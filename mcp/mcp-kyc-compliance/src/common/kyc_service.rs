use std::sync::Arc;

use serde::Serialize;
use sqlx::{FromRow, PgPool};

use rmcp::{
    handler::server::common::Extension,
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};

use super::auth::{extract_caller, Caller};
use super::regulatory_corpus::{RegulatoryCorpus, RegulatoryMatch};

/// Concentration threshold from `data/corpus/03-suitability.md`: a product
/// is not suitable if it would push (or keep) the client's exposure to its
/// own sector above this percentage of the client's total portfolio value.
const MAX_SECTOR_CONCENTRATION_PCT: f64 = 25.0;

// =================== RESPONSE ENVELOPE (AUDIT TRAIL) ===================

#[derive(Debug, Serialize)]
pub struct ToolResponse<T: Serialize> {
    pub output: T,
    pub called_by: String,
    pub roles: Vec<String>,
}

impl<T: Serialize> ToolResponse<T> {
    fn new(output: T, caller: &Caller) -> Self {
        Self {
            output,
            called_by: caller.banker_id.clone(),
            roles: caller.roles.clone(),
        }
    }

    fn into_call_tool_result(self) -> Result<CallToolResult, McpError> {
        let json = serde_json::to_string_pretty(&self).map_err(|e| {
            McpError::internal_error(format!("error serializing response: {e}"), None)
        })?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }
}

// =================== DATA TYPES ===================

#[derive(Debug, Serialize, FromRow)]
pub struct RiskProfile {
    pub risk_profile: String,
    pub kyc_status: String,
    pub pep_flag: bool,
}

#[derive(Debug, FromRow)]
struct Product {
    risk_rating: String,
    sector: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SuitabilityResult {
    pub potentially_suitable: bool,
    pub risk_ok: bool,
    pub sector_concentration_pct: f64,
    pub explanation: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetRiskProfileParams {
    #[schemars(description = "Identifier of the client whose risk profile is being consulted")]
    pub client_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CheckSuitabilityParams {
    #[schemars(description = "Identifier of the client considering the product")]
    pub client_id: String,
    #[schemars(description = "Identifier of the product being evaluated for suitability")]
    pub product_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchRegulatoryGuidanceParams {
    #[schemars(
        description = "Natural-language question about the (fictional) regulatory corpus, e.g. 'when to escalate an unusual transaction'"
    )]
    pub query: String,
}

// =================== ISOLATION RULE ===================

/// Checks that `client_id` belongs to `banker_id`. The error message is
/// deliberately ambiguous: it must not reveal whether `client_id` exists but
/// belongs to another banker, or doesn't exist at all. Same rule and same
/// rationale as `mcp-portfolio`'s `assert_owns_client` — copied from there.
/// `product_id` and `query` are never checked this way: products are a
/// shared catalog with no owner, and a free-text query has no client to own.
async fn assert_owns_client(
    pool: &PgPool,
    banker_id: &str,
    client_id: &str,
) -> Result<(), McpError> {
    let owner: Option<String> = sqlx::query_scalar("SELECT banker_id FROM clients WHERE id = $1")
        .bind(client_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| McpError::internal_error(format!("database error: {e}"), None))?;

    match owner {
        Some(o) if o == banker_id => Ok(()),
        _ => {
            tracing::warn!(
                target: "tenant_violation",
                banker_id,
                client_id,
                "access denied to client outside the caller's book"
            );
            Err(McpError::invalid_params(
                "client_id not found for the authenticated caller",
                None,
            ))
        }
    }
}

/// Ordinal used to compare a client's declared risk profile against a
/// product's risk rating. Both columns share the same
/// `CHECK (... IN ('conservative', 'moderate', 'aggressive'))` constraint
/// in the schema, so an unrecognized value here would indicate a schema
/// drift, not normal input.
fn risk_ordinal(risk: &str) -> Result<u8, McpError> {
    match risk {
        "conservative" => Ok(0),
        "moderate" => Ok(1),
        "aggressive" => Ok(2),
        other => Err(McpError::internal_error(
            format!("unrecognized risk rating '{other}' — schema/data drift"),
            None,
        )),
    }
}

// =================== MCP SERVICE ===================

#[derive(Clone)]
pub struct KycComplianceService {
    pool: PgPool,
    corpus: Arc<RegulatoryCorpus>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl KycComplianceService {
    pub fn new(pool: PgPool, corpus: Arc<RegulatoryCorpus>) -> Self {
        Self {
            pool,
            corpus,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Returns the declared risk profile, KYC status, and PEP flag for a client of the authenticated banker. Fails if the client does not belong to the caller."
    )]
    pub async fn get_risk_profile(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetRiskProfileParams>,
    ) -> Result<CallToolResult, McpError> {
        let caller = extract_caller(&parts);
        assert_owns_client(&self.pool, &caller.banker_id, &params.client_id).await?;

        let profile: Option<RiskProfile> =
            sqlx::query_as("SELECT risk_profile, kyc_status, pep_flag FROM clients WHERE id = $1")
                .bind(&params.client_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| McpError::internal_error(format!("database error: {e}"), None))?;

        let Some(profile) = profile else {
            return Err(McpError::invalid_params(
                "client_id not found for the authenticated caller",
                None,
            ));
        };

        ToolResponse::new(profile, &caller).into_call_tool_result()
    }

    #[tool(
        description = "Evaluates whether a product is potentially suitable for a client, per the (fictional, simplified) rules in data/corpus/03-suitability.md: the product's risk rating must not exceed the client's declared risk profile, and the product's sector must not already represent more than 25% of the client's portfolio. Ownership is validated only on client_id — product_id refers to a shared catalog, not client data."
    )]
    pub async fn check_suitability(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CheckSuitabilityParams>,
    ) -> Result<CallToolResult, McpError> {
        let caller = extract_caller(&parts);
        assert_owns_client(&self.pool, &caller.banker_id, &params.client_id).await?;

        let client_risk: String =
            sqlx::query_scalar("SELECT risk_profile FROM clients WHERE id = $1")
                .bind(&params.client_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| McpError::internal_error(format!("database error: {e}"), None))?;

        let product: Option<Product> =
            sqlx::query_as("SELECT risk_rating, sector FROM products WHERE id = $1")
                .bind(&params.product_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| McpError::internal_error(format!("database error: {e}"), None))?;
        let Some(product) = product else {
            return Err(McpError::invalid_params(
                format!("product_id '{}' not found", params.product_id),
                None,
            ));
        };

        let risk_ok = risk_ordinal(&product.risk_rating)? <= risk_ordinal(&client_risk)?;

        // Current sector breakdown of the client's portfolio. This is a
        // proxy for "would this product further concentrate an
        // already-concentrated sector", not a simulation of a specific
        // trade amount — check_suitability takes no investment amount per
        // the spec, so it can only reason about the client's *existing*
        // exposure to the product's sector.
        let sector_totals: Vec<(String, f64)> = sqlx::query_as(
            "SELECT sector, SUM(market_value)::float8 FROM positions WHERE client_id = $1 GROUP BY sector",
        )
        .bind(&params.client_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| McpError::internal_error(format!("database error: {e}"), None))?;

        let total_value: f64 = sector_totals.iter().map(|(_, v)| v).sum();
        let sector_concentration_pct = match &product.sector {
            Some(sector) if total_value > 0.0 => {
                let sector_value: f64 = sector_totals
                    .iter()
                    .filter(|(s, _)| s == sector)
                    .map(|(_, v)| v)
                    .sum();
                sector_value / total_value * 100.0
            }
            _ => 0.0,
        };

        let concentration_ok = sector_concentration_pct <= MAX_SECTOR_CONCENTRATION_PCT;
        let potentially_suitable = risk_ok && concentration_ok;

        let explanation = if !risk_ok {
            format!(
                "Product risk rating '{}' exceeds the client's declared risk profile '{}'.",
                product.risk_rating, client_risk
            )
        } else if !concentration_ok {
            format!(
                "The client's existing exposure to sector '{}' is already {:.1}% of their portfolio, above the {:.0}% concentration limit in data/corpus/03-suitability.md — this product would concentrate that exposure further.",
                product.sector.as_deref().unwrap_or("unknown"),
                sector_concentration_pct,
                MAX_SECTOR_CONCENTRATION_PCT
            )
        } else {
            "Risk rating is within the client's declared profile and sector concentration remains within the 25% limit.".to_string()
        };

        ToolResponse::new(
            SuitabilityResult {
                potentially_suitable,
                risk_ok,
                sector_concentration_pct,
                explanation,
            },
            &caller,
        )
        .into_call_tool_result()
    }

    #[tool(
        description = "Semantic search over the (fictional, simplified) regulatory guidance corpus. Returns the top matching text fragments plus their source document, so the caller can cite the clause. No client-ownership check applies — this is regulatory data, not client data."
    )]
    pub async fn search_regulatory_guidance(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<SearchRegulatoryGuidanceParams>,
    ) -> Result<CallToolResult, McpError> {
        let caller = extract_caller(&parts);

        let matches: Vec<RegulatoryMatch> = self
            .corpus
            .search(&params.query, 2)
            .await
            .map_err(|e| McpError::internal_error(format!("corpus search error: {e}"), None))?;

        ToolResponse::new(matches, &caller).into_call_tool_result()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for KycComplianceService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "KYC/Compliance MCP server (fictional, simplified demo corpus — see README \
                 disclaimer). Each banker can only consult clients in their own book: identity \
                 comes from the JWT validated by the Envoy sidecar, never from a model \
                 parameter. Tools: get_risk_profile(client_id), \
                 check_suitability(client_id, product_id), \
                 search_regulatory_guidance(query).",
            )
            .with_server_info(Implementation::new(
                "mcp-kyc-compliance".to_string(),
                env!("CARGO_PKG_VERSION").to_string(),
            ))
    }
}

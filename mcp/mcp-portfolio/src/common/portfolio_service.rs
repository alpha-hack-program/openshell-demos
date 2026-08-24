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

// =================== ENVOLTORIO DE RESPUESTA (RASTRO DE AUDITORÍA) ===================

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
            McpError::internal_error(format!("error serializando la respuesta: {e}"), None)
        })?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }
}

// =================== TIPOS DE DATOS ===================

#[derive(Debug, Serialize, FromRow)]
pub struct ClientSummary {
    pub id: String,
    pub name: String,
    pub risk_profile: String,
    pub kyc_status: String,
    pub pep_flag: bool,
    pub sector_focus: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Position {
    pub id: String,
    pub client_id: String,
    pub ticker: String,
    pub isin: Option<String>,
    pub sector: String,
    pub quantity: f64,
    pub price: f64,
    pub market_value: f64,
    pub currency: String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct PerformanceSnapshot {
    pub twr: f64,
    pub benchmark_twr: f64,
}

#[derive(Debug, Serialize, FromRow)]
pub struct TopClientAum {
    pub client_id: String,
    pub name: String,
    pub aum_total: f64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetPositionsParams {
    #[schemars(description = "Identificador del cliente cuyas posiciones se quieren consultar")]
    pub client_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetPerformanceParams {
    #[schemars(description = "Identificador del cliente cuyo rendimiento se quiere consultar")]
    pub client_id: String,
    #[schemars(description = "Periodo a consultar. VALORES VÁLIDOS: 'MTD', 'QTD', 'YTD'")]
    pub period: String,
}

// =================== REGLA DE AISLAMIENTO ===================

/// Comprueba que `client_id` pertenece a `banker_id`. El mensaje de error es
/// deliberadamente ambiguo: no debe revelar si el `client_id` existe pero
/// pertenece a otro banquero, o si no existe en absoluto.
async fn assert_owns_client(
    pool: &PgPool,
    banker_id: &str,
    client_id: &str,
) -> Result<(), McpError> {
    let owner: Option<String> = sqlx::query_scalar("SELECT banker_id FROM clients WHERE id = $1")
        .bind(client_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| McpError::internal_error(format!("error de base de datos: {e}"), None))?;

    match owner {
        Some(o) if o == banker_id => Ok(()),
        _ => {
            tracing::warn!(
                target: "tenant_violation",
                banker_id,
                client_id,
                "acceso denegado a cliente fuera del libro"
            );
            Err(McpError::invalid_params(
                "client_id no encontrado para el llamante autenticado",
                None,
            ))
        }
    }
}

// =================== SERVICIO MCP ===================

#[derive(Clone)]
pub struct PortfolioService {
    pool: PgPool,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl PortfolioService {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Lista los clientes del banquero autenticado, incluido su perfil de riesgo y estado KYC/PEP. No admite parámetros: el banker_id se toma siempre del JWT, nunca de un argumento del modelo."
    )]
    pub async fn list_my_clients(
        &self,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Result<CallToolResult, McpError> {
        let caller = extract_caller(&parts);

        let clients: Vec<ClientSummary> = sqlx::query_as(
            "SELECT id, name, risk_profile, kyc_status, pep_flag, sector_focus \
             FROM clients WHERE banker_id = $1",
        )
        .bind(&caller.banker_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| McpError::internal_error(format!("error de base de datos: {e}"), None))?;

        ToolResponse::new(clients, &caller).into_call_tool_result()
    }

    #[tool(
        description = "Devuelve las posiciones de un cliente del banquero autenticado. Falla si el cliente no pertenece al llamante."
    )]
    pub async fn get_positions(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetPositionsParams>,
    ) -> Result<CallToolResult, McpError> {
        let caller = extract_caller(&parts);
        assert_owns_client(&self.pool, &caller.banker_id, &params.client_id).await?;

        let positions: Vec<Position> = sqlx::query_as(
            "SELECT id, client_id, ticker, isin, sector, \
             quantity::float8 AS quantity, price::float8 AS price, \
             market_value::float8 AS market_value, currency \
             FROM positions WHERE client_id = $1",
        )
        .bind(&params.client_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| McpError::internal_error(format!("error de base de datos: {e}"), None))?;

        ToolResponse::new(positions, &caller).into_call_tool_result()
    }

    #[tool(
        description = "Devuelve el rendimiento (TWR) de un cliente del banquero autenticado para un periodo dado ('MTD', 'QTD' o 'YTD'). Falla si el cliente no pertenece al llamante."
    )]
    pub async fn get_performance(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetPerformanceParams>,
    ) -> Result<CallToolResult, McpError> {
        let caller = extract_caller(&parts);
        assert_owns_client(&self.pool, &caller.banker_id, &params.client_id).await?;

        let snapshot: Option<PerformanceSnapshot> = sqlx::query_as(
            "SELECT twr::float8 AS twr, benchmark_twr::float8 AS benchmark_twr \
             FROM performance_snapshots WHERE client_id = $1 AND period = $2",
        )
        .bind(&params.client_id)
        .bind(&params.period)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| McpError::internal_error(format!("error de base de datos: {e}"), None))?;

        let Some(snapshot) = snapshot else {
            return Err(McpError::invalid_params(
                format!(
                    "no hay datos de rendimiento para el periodo '{}' de este cliente",
                    params.period
                ),
                None,
            ));
        };

        ToolResponse::new(snapshot, &caller).into_call_tool_result()
    }

    #[tool(
        description = "Calcula, entre los clientes del banquero autenticado, cuál tiene mayor patrimonio bajo gestión (AUM), sumando el valor de mercado de sus posiciones. No admite parámetros."
    )]
    pub async fn get_top_client_by_aum(
        &self,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Result<CallToolResult, McpError> {
        let caller = extract_caller(&parts);

        let top_client: Option<TopClientAum> = sqlx::query_as(
            "SELECT c.id AS client_id, c.name, SUM(p.market_value)::float8 AS aum_total \
             FROM clients c \
             JOIN positions p ON p.client_id = c.id \
             WHERE c.banker_id = $1 \
             GROUP BY c.id, c.name \
             ORDER BY aum_total DESC \
             LIMIT 1",
        )
        .bind(&caller.banker_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| McpError::internal_error(format!("error de base de datos: {e}"), None))?;

        let Some(top_client) = top_client else {
            return Err(McpError::invalid_params(
                "el banquero autenticado no tiene clientes con posiciones",
                None,
            ));
        };

        ToolResponse::new(top_client, &caller).into_call_tool_result()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for PortfolioService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Servidor MCP de cartera para banqueros. Cada banquero solo puede consultar \
                 clientes de su propio libro: la identidad se toma del JWT validado por el \
                 sidecar Envoy, nunca de un parámetro del modelo. Herramientas disponibles: \
                 list_my_clients (sin parámetros), get_positions(client_id), \
                 get_performance(client_id, period: 'MTD'|'QTD'|'YTD'), \
                 get_top_client_by_aum (sin parámetros).",
            )
            .with_server_info(Implementation::new(
                "mcp-portfolio".to_string(),
                env!("CARGO_PKG_VERSION").to_string(),
            ))
    }
}

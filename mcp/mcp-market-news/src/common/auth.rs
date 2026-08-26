//! Caller identity extraction from a bearer JWT.
//!
//! This mirrors the pattern used across the sibling MCP servers in this demo
//! family (`mcp-portfolio`, `mcp-kyc-compliance`, `mcp-crm-calendar`): the
//! gateway's Envoy sidecar validates the Keycloak-issued JWT's signature
//! *before* the request reaches this process, so here we only base64-decode
//! the JWT payload to read claims — we never verify a signature ourselves.
//! Do not reuse this module's decoding logic anywhere the JWT signature
//! has not already been validated upstream.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use http::HeaderMap;
use serde::Serialize;

/// Identity of whoever called a tool, as read from their bearer token.
#[derive(Debug, Clone, Serialize)]
pub struct Caller {
    pub banker_id: String,
    pub roles: Vec<String>,
}

impl Caller {
    /// Identity used when no `Authorization` header is present, or it
    /// cannot be parsed as a JWT-shaped bearer token.
    pub fn unknown() -> Self {
        Self {
            banker_id: "unknown".to_string(),
            roles: Vec::new(),
        }
    }
}

/// Extracts the caller's identity from the `Authorization: Bearer <jwt>`
/// header. The JWT signature is NOT verified here — that already happened
/// upstream, in the Envoy sidecar sitting in front of this service. This
/// function only reads claims out of the (already-trusted) payload.
pub fn extract_caller(headers: &HeaderMap) -> Caller {
    let token = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));

    let Some(token) = token else {
        return Caller::unknown();
    };

    let payload_b64 = token.split('.').nth(1).unwrap_or_default();
    let payload = URL_SAFE_NO_PAD.decode(payload_b64).unwrap_or_default();
    let claims: serde_json::Value = serde_json::from_slice(&payload).unwrap_or_default();

    Caller {
        banker_id: claims["preferred_username"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
        roles: claims["realm_access"]["roles"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Convenience wrapper for callers holding an `http::request::Parts`
/// (the shape rmcp's `Extension` handler-argument hands us).
pub fn extract_caller_from_parts(parts: &http::request::Parts) -> Caller {
    extract_caller(&parts.headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_for(claims: &serde_json::Value) -> String {
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
        format!("eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.{payload}.")
    }

    #[test]
    fn extracts_username_and_roles() {
        let claims = serde_json::json!({
            "preferred_username": "bob",
            "realm_access": { "roles": ["banker", "uma_authorization"] }
        });
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            format!("Bearer {}", token_for(&claims)).parse().unwrap(),
        );

        let caller = extract_caller(&headers);
        assert_eq!(caller.banker_id, "bob");
        assert_eq!(caller.roles, vec!["banker", "uma_authorization"]);
    }

    #[test]
    fn missing_header_yields_unknown() {
        let headers = HeaderMap::new();
        let caller = extract_caller(&headers);
        assert_eq!(caller.banker_id, "unknown");
        assert!(caller.roles.is_empty());
    }

    #[test]
    fn malformed_bearer_yields_unknown_fields() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer not-a-jwt".parse().unwrap());
        let caller = extract_caller(&headers);
        // No valid JWT payload to decode from "not-a-jwt".split('.').nth(1)
        assert_eq!(caller.banker_id, "unknown");
        assert!(caller.roles.is_empty());
    }
}

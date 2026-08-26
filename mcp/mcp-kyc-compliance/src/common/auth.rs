//! Caller identity extracted from the JWT.
//!
//! The Envoy sidecar in front of this service has already validated the
//! token's signature against Keycloak before the request reaches here. This
//! module does NOT validate a signature: it only base64-decodes the payload
//! to read `preferred_username` and `realm_access.roles`. Same pattern as
//! every other MCP server in this demo family (see `mcp-portfolio`'s
//! `src/common/auth.rs`, which this file is copied from).

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Caller {
    pub banker_id: String,
    pub roles: Vec<String>,
}

impl Caller {
    pub fn unknown() -> Self {
        Self {
            banker_id: "unknown".to_string(),
            roles: Vec::new(),
        }
    }
}

pub fn extract_caller(parts: &http::request::Parts) -> Caller {
    let Some(token) = parts
        .headers
        .get(http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
    else {
        return Caller::unknown();
    };

    let Some(payload_b64) = token.split('.').nth(1).filter(|s| !s.is_empty()) else {
        return Caller::unknown();
    };

    let Ok(payload) = URL_SAFE_NO_PAD.decode(payload_b64) else {
        return Caller::unknown();
    };

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

#[cfg(test)]
mod tests {
    use super::*;

    fn parts_with_auth(header: Option<&str>) -> http::request::Parts {
        let mut builder = http::Request::builder();
        if let Some(h) = header {
            builder = builder.header(http::header::AUTHORIZATION, h);
        }
        let (parts, _) = builder.body(()).unwrap().into_parts();
        parts
    }

    fn fake_token(payload_json: &str) -> String {
        let payload = URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        format!("eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.{payload}.")
    }

    #[test]
    fn no_authorization_header_is_unknown() {
        let caller = extract_caller(&parts_with_auth(None));
        assert_eq!(caller.banker_id, "unknown");
        assert!(caller.roles.is_empty());
    }

    #[test]
    fn extracts_username_and_roles_from_bearer_token() {
        let token = fake_token(
            r#"{"preferred_username":"bob","realm_access":{"roles":["banker","offline_access"]}}"#,
        );
        let header = format!("Bearer {token}");
        let caller = extract_caller(&parts_with_auth(Some(&header)));
        assert_eq!(caller.banker_id, "bob");
        assert_eq!(caller.roles, vec!["banker", "offline_access"]);
    }

    #[test]
    fn malformed_token_falls_back_to_unknown() {
        let caller = extract_caller(&parts_with_auth(Some("Bearer not-a-jwt")));
        assert_eq!(caller.banker_id, "unknown");
        assert!(caller.roles.is_empty());
    }

    #[test]
    fn missing_bearer_prefix_falls_back_to_unknown() {
        let caller = extract_caller(&parts_with_auth(Some("Basic dXNlcjpwYXNz")));
        assert_eq!(caller.banker_id, "unknown");
    }
}

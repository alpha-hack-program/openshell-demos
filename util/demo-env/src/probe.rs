// SPDX-License-Identifier: Apache-2.0

//! Reading the content-bearing files — gateway registration, OIDC bundle,
//! PEM material — leniently.
//!
//! Lenient is the point. These files are written by the openshell CLI, not
//! by us, and their schemas belong to a dependency this repo pins by tag.
//! A field appearing, moving or changing type between chart versions must
//! degrade to "unknown" in one row of output, never to a hard failure that
//! makes `demo-env status` useless exactly when something is already
//! wrong.

use std::path::Path;

use base64::Engine as _;
use serde_json::Value;

/// `gateways/<name>/metadata.json`.
pub struct GatewayMetadata {
    /// The cluster this identity is registered against. `use_identity`
    /// compares this to the endpoint derived from `.env`.
    pub endpoint: Option<String>,
    /// `oidc` for this demo; mTLS-only gateways report something else.
    pub auth_mode: Option<String>,
}

impl GatewayMetadata {
    pub fn read(path: &Path) -> Result<GatewayMetadata, String> {
        let value = read_json(path)?;
        Ok(GatewayMetadata {
            endpoint: string_field(&value, &["gateway_endpoint", "endpoint", "url"]),
            auth_mode: string_field(&value, &["auth_mode", "authMode"]),
        })
    }
}

/// `gateways/<name>/oidc_token.json` — what `openshell gateway login`
/// writes and `parrot` rotates in place.
pub struct OidcBundle {
    pub issuer: Option<String>,
    pub client_id: Option<String>,
    pub has_refresh_token: bool,
    /// Access-token expiry as a unix timestamp, from the bundle's own
    /// field when present, else from the JWT's `exp`.
    pub expires_at: Option<i64>,
    /// Who the access token actually belongs to. The single most useful
    /// fact in the whole report: an `oc-bob` tree holding alice's token is
    /// invisible to every file-existence check, and produces failures
    /// downstream ("not a member of this workspace") that point nowhere
    /// near the cause.
    pub subject: Option<String>,
}

impl OidcBundle {
    pub fn read(path: &Path) -> Result<OidcBundle, String> {
        let value = read_json(path)?;
        let access_token = string_field(&value, &["access_token", "accessToken"]);
        let claims = access_token.as_deref().and_then(jwt_claims);

        let expires_at = value
            .get("expires_at")
            .or_else(|| value.get("expiresAt"))
            .and_then(as_unix_timestamp)
            .or_else(|| {
                claims
                    .as_ref()
                    .and_then(|c| c.get("exp"))
                    .and_then(as_unix_timestamp)
            });

        let subject = claims
            .as_ref()
            .and_then(|c| string_field(c, &["preferred_username", "email", "name", "sub"]));

        Ok(OidcBundle {
            issuer: string_field(&value, &["issuer", "iss"]),
            client_id: string_field(&value, &["client_id", "clientId"]),
            has_refresh_token: string_field(&value, &["refresh_token", "refreshToken"]).is_some(),
            expires_at,
            subject,
        })
    }
}

/// What a PEM file turned out to contain.
pub struct PemInfo {
    pub certificates: usize,
    /// `notAfter` of the first certificate, as a unix timestamp.
    pub first_not_after: Option<i64>,
    /// Subject CN of the first certificate, for telling a client cert
    /// apart from a CA at a glance.
    pub first_subject_cn: Option<String>,
}

pub fn read_pem(path: &Path) -> Result<PemInfo, String> {
    use x509_parser::prelude::*;

    let bytes = std::fs::read(path).map_err(|e| format!("cannot read: {e}"))?;
    let mut certificates = 0usize;
    let mut first_not_after = None;
    let mut first_subject_cn = None;

    for pem in Pem::iter_from_buffer(&bytes).flatten() {
        if pem.label != "CERTIFICATE" {
            continue;
        }
        certificates += 1;
        let Ok(cert) = pem.parse_x509() else { continue };
        if first_not_after.is_none() {
            first_not_after = Some(cert.validity().not_after.timestamp());
            first_subject_cn = cert
                .subject()
                .iter_common_name()
                .next()
                .and_then(|cn| cn.as_str().ok())
                .map(str::to_string);
        }
    }

    Ok(PemInfo {
        certificates,
        first_not_after,
        first_subject_cn,
    })
}

/// True if the file looks like a PEM private key. Only a shape check — we
/// never parse, and certainly never log, key material.
pub fn looks_like_private_key(path: &Path) -> Result<bool, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read: {e}"))?;
    Ok(text.contains("PRIVATE KEY-----"))
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("not valid JSON: {e}"))
}

/// First of `keys` present as a non-empty string.
fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|k| value.get(*k))
        .filter_map(Value::as_str)
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

/// Accept a unix timestamp as a JSON number or as an RFC3339 string —
/// both shapes have been seen in these files, and guessing wrong would
/// silently report every token as "expiry unknown".
fn as_unix_timestamp(value: &Value) -> Option<i64> {
    if let Some(n) = value.as_i64() {
        return Some(n);
    }
    if let Some(f) = value.as_f64() {
        return Some(f as i64);
    }
    let text = value.as_str()?;
    if let Ok(n) = text.parse::<i64>() {
        return Some(n);
    }
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|dt| dt.timestamp())
}

/// Decode a JWT's claims without verifying it. Verification would need the
/// issuer's JWKS and a network call; everything read here is advisory
/// display, and the gateway is the only party whose opinion of the
/// signature matters.
fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn timestamps_parse_from_either_shape() {
        assert_eq!(
            as_unix_timestamp(&json!(1_760_000_000)),
            Some(1_760_000_000)
        );
        assert_eq!(as_unix_timestamp(&json!("1760000000")), Some(1_760_000_000));
        assert_eq!(
            as_unix_timestamp(&json!("2026-10-08T12:00:00Z")),
            Some(1_791_460_800)
        );
        assert_eq!(as_unix_timestamp(&json!(null)), None);
    }

    #[test]
    fn jwt_payload_decodes_without_verification() {
        // {"preferred_username":"alice","exp":1760000000}
        let token = "ignored.eyJwcmVmZXJyZWRfdXNlcm5hbWUiOiJhbGljZSIsImV4cCI6MTc2MDAwMDAwMH0.sig";
        let claims = jwt_claims(token).expect("claims decode");
        assert_eq!(claims["preferred_username"], "alice");
        assert_eq!(as_unix_timestamp(&claims["exp"]), Some(1_760_000_000));
    }

    #[test]
    fn missing_and_empty_fields_read_as_absent() {
        let value = json!({ "issuer": "", "client_id": "openshell-cli" });
        assert_eq!(string_field(&value, &["issuer"]), None);
        assert_eq!(
            string_field(&value, &["client_id"]).as_deref(),
            Some("openshell-cli")
        );
    }
}

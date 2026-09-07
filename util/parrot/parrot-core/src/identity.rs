// SPDX-License-Identifier: Apache-2.0

//! Best-effort identity resolution from the active gateway's OIDC token, for
//! greeting the user by name (e.g. on the TUI splash screen).

use base64::Engine as _;
use serde_json::Value;

/// Who parrot thinks is logged in, resolved from the OIDC access token's
/// claims. Falls back to a generic identity when nothing can be resolved
/// (unauthenticated local gateway, unparseable token, etc.) rather than
/// erroring — greeting the user is a nicety, not a requirement.
#[derive(Debug, Clone, Default)]
pub struct Identity {
    pub display_name: Option<String>,
    pub subject: Option<String>,
}

impl Identity {
    pub fn generic() -> Self {
        Self::default()
    }

    /// A greeting suitable for the splash screen.
    pub fn greeting(&self) -> String {
        match &self.display_name {
            Some(name) => format!("Welcome back, {name}."),
            None => "Welcome.".to_string(),
        }
    }
}

/// Extract a display name from an OIDC access token's JWT claims.
///
/// Does not verify the token's signature — parrot trusts its own on-disk
/// token store the same way `openshell whoami` does. Not a security
/// boundary: this is display-only.
pub fn identity_from_access_token(token: &str) -> Identity {
    let Some(claims) = decode_jwt_claims(token) else {
        return Identity::generic();
    };

    let display_name = ["name", "preferred_username", "email"]
        .iter()
        .find_map(|key| claims.get(*key).and_then(Value::as_str))
        .map(str::to_string);
    let subject = claims
        .get("sub")
        .and_then(Value::as_str)
        .map(str::to_string);

    Identity {
        display_name,
        subject,
    }
}

fn decode_jwt_claims(token: &str) -> Option<serde_json::Map<String, Value>> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value.as_object().cloned()
}

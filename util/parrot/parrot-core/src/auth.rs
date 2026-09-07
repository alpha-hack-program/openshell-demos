// SPDX-License-Identifier: Apache-2.0

//! OIDC auth wiring on top of the CLI's on-disk token store.
//!
//! `openshell-sdk` refreshes tokens through a caller-supplied [`Refresh`]
//! implementation; it never runs a browser flow itself. Parrot doesn't
//! either — it only rotates an already-issued refresh token, the same way
//! `openshell-cli`'s `apply_auth_with_status` does. Log in with
//! `openshell gateway login <name>` first.
//!
//! v1 is OIDC-only: `openshell-sdk`'s transport does not speak mTLS at
//! all, so mTLS-authenticated gateways are out of scope until the SDK
//! grows that support upstream.

use std::sync::Arc;

use async_trait::async_trait;
use openshell_bootstrap::oidc_token::{self, OidcTokenBundle};
use openshell_sdk::{oidc, AuthConfig, Refresh, RefreshError, RefreshedToken};

use crate::error::{ParrotError, Result};
use crate::gateway::GatewayContext;

/// Implements the SDK's [`Refresh`] contract against
/// `$XDG_CONFIG_HOME/openshell/gateways/<name>/oidc_token.json`, the same
/// file `openshell-cli` reads and writes.
struct StoredOidcRefresher {
    gateway_name: String,
    scopes: Vec<String>,
}

#[async_trait]
impl Refresh for StoredOidcRefresher {
    async fn refresh(&self) -> std::result::Result<RefreshedToken, RefreshError> {
        let bundle = oidc_token::load_oidc_token(&self.gateway_name).ok_or_else(|| {
            RefreshError::Terminal(format!(
                "no stored OIDC credentials for gateway '{name}'; run `openshell gateway login {name}`",
                name = self.gateway_name
            ))
        })?;
        let refresh_token = bundle.refresh_token.clone().ok_or_else(|| {
            RefreshError::Terminal("stored OIDC bundle has no refresh token".to_string())
        })?;

        let input =
            oidc::RefreshTokenInput::new(refresh_token.clone(), &bundle.issuer, &bundle.client_id)
                .with_scopes(self.scopes.clone());

        let output = oidc::refresh_token(&input)
            .await
            .map_err(|e| RefreshError::Transient(e.to_string()))?;

        let updated = OidcTokenBundle {
            access_token: output.access_token.clone(),
            refresh_token: output.refresh_token.clone().or(Some(refresh_token)),
            expires_at: output.expires_at,
            issuer: bundle.issuer,
            client_id: bundle.client_id,
        };
        // Best-effort: a failed write just means the next refresh redoes
        // the round trip, not a reason to fail this one.
        let _ = oidc_token::store_oidc_token(&self.gateway_name, &updated);

        Ok(match output.expires_at {
            Some(expires_at) => {
                RefreshedToken::new(output.access_token).with_expires_at(expires_at)
            }
            None => RefreshedToken::new(output.access_token),
        })
    }
}

/// Build an [`AuthConfig`] for `gateway`, wired for live OIDC rotation via
/// the stored refresh token.
pub fn resolve_auth(gateway: &GatewayContext) -> Result<AuthConfig> {
    match gateway.metadata.auth_mode.as_deref() {
        Some("oidc") => {
            let bundle = oidc_token::load_oidc_token(&gateway.name).ok_or_else(|| {
                ParrotError::MissingOidcCredentials {
                    gateway: gateway.name.clone(),
                }
            })?;
            let scopes = gateway
                .metadata
                .oidc_scopes
                .as_deref()
                .map(|s| s.split_whitespace().map(str::to_string).collect())
                .unwrap_or_default();
            let refresher: Arc<dyn Refresh> = Arc::new(StoredOidcRefresher {
                gateway_name: gateway.name.clone(),
                scopes,
            });
            Ok(AuthConfig::Oidc {
                token: bundle.access_token,
                expires_at: bundle.expires_at,
                refresh: Some(refresher),
            })
        }
        other => Err(ParrotError::UnsupportedAuthMode {
            gateway: gateway.name.clone(),
            mode: other.unwrap_or("mtls").to_string(),
        }),
    }
}

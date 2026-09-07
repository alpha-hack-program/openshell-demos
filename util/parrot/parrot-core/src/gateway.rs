// SPDX-License-Identifier: Apache-2.0

//! Gateway resolution, mirroring `openshell` CLI's precedence.
//!
//! `openshell-sdk` deliberately does no filesystem access or gateway-name
//! resolution — callers hand it a fully-formed `ClientConfig`. That
//! resolution logic lives inline in `openshell-cli`'s binary (private to
//! the `main.rs` bin target, not exported from its lib), so parrot
//! reimplements the same precedence here against `openshell-bootstrap`'s
//! public, stable functions rather than shelling out to the CLI.

use openshell_bootstrap::{
    list_gateways, load_active_gateway, load_gateway_metadata, GatewayMetadata,
};

use crate::error::{ParrotError, Result};

/// A resolved gateway: name, endpoint, and its stored metadata (auth mode,
/// OIDC issuer/client, etc.).
#[derive(Debug, Clone)]
pub struct GatewayContext {
    pub name: String,
    pub endpoint: String,
    pub metadata: GatewayMetadata,
}

fn normalize_endpoint(endpoint: &str) -> &str {
    endpoint.trim_end_matches('/')
}

fn find_gateway_by_endpoint(endpoint: &str) -> Option<String> {
    let endpoint = normalize_endpoint(endpoint);

    if let Some(active) = load_active_gateway() {
        if let Ok(metadata) = load_gateway_metadata(&active) {
            if normalize_endpoint(&metadata.gateway_endpoint) == endpoint {
                return Some(metadata.name);
            }
        }
    }

    list_gateways()
        .ok()?
        .into_iter()
        .find_map(|m| (normalize_endpoint(&m.gateway_endpoint) == endpoint).then_some(m.name))
}

/// Resolve which gateway to talk to.
///
/// Precedence, matching `openshell`:
/// 1. explicit `gateway_endpoint` (direct URL; `gateway_name`, if also
///    given, pins which stored metadata to use for auth/TLS material)
/// 2. explicit `gateway_name`
/// 3. `OPENSHELL_GATEWAY` environment variable
/// 4. the active gateway pointer (`$XDG_CONFIG_HOME/openshell/active_gateway`)
///
/// Unlike the CLI, parrot always requires stored metadata to exist — it
/// reads OIDC issuer/client/scopes from it and never runs the interactive
/// login flow itself. Register and log in via `openshell gateway add` /
/// `openshell gateway login` first.
pub fn resolve_gateway(
    gateway_name: Option<&str>,
    gateway_endpoint: Option<&str>,
) -> Result<GatewayContext> {
    let name = if let Some(endpoint) = gateway_endpoint {
        // Trust an explicit endpoint directly; only consult stored metadata
        // to recover the gateway *name* (needed for auth/TLS material), not
        // to decide whether to use the endpoint at all.
        gateway_name
            .map(str::to_string)
            .or_else(|| find_gateway_by_endpoint(endpoint))
            .unwrap_or_else(|| endpoint.to_string())
    } else {
        gateway_name
            .map(str::to_string)
            .or_else(|| {
                std::env::var("OPENSHELL_GATEWAY")
                    .ok()
                    .filter(|v| !v.trim().is_empty())
            })
            .or_else(load_active_gateway)
            .ok_or(ParrotError::NoActiveGateway)?
    };

    let metadata =
        load_gateway_metadata(&name).map_err(|_| ParrotError::UnknownGateway(name.clone()))?;

    let endpoint = gateway_endpoint
        .map(str::to_string)
        .unwrap_or_else(|| metadata.gateway_endpoint.clone());

    Ok(GatewayContext {
        name: metadata.name.clone(),
        endpoint,
        metadata,
    })
}

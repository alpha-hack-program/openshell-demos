// SPDX-License-Identifier: Apache-2.0

use openshell_sdk::{AuthConfig, ClientConfig, ListOptions, OpenShellClient};

use crate::auth::resolve_auth;
use crate::error::{ParrotError, Result};
use crate::gateway::{resolve_gateway, GatewayContext};
use crate::identity::{identity_from_access_token, Identity};

/// A connected OpenShell client plus the gateway and identity context it was
/// resolved from — the thing both `parrot-tui` and `parrot-gpui` build
/// around.
pub struct ParrotClient {
    pub gateway: GatewayContext,
    pub identity: Identity,
    inner: OpenShellClient,
}

impl ParrotClient {
    /// Resolve the target gateway (explicit name/endpoint, then
    /// `OPENSHELL_GATEWAY`, then the active gateway pointer), load its
    /// stored OIDC credentials, and connect.
    pub async fn connect(
        gateway_name: Option<&str>,
        gateway_endpoint: Option<&str>,
    ) -> Result<Self> {
        let gateway = resolve_gateway(gateway_name, gateway_endpoint)?;
        let auth = resolve_auth(&gateway)?;

        let identity = match &auth {
            AuthConfig::Oidc { token, .. } => identity_from_access_token(token),
            _ => Identity::generic(),
        };

        let mut config = ClientConfig::new(gateway.endpoint.clone());
        config.auth = Some(auth);
        config.ca_cert = load_gateway_ca_cert(&gateway.name);
        let inner = OpenShellClient::connect(config).await?;

        Ok(Self {
            gateway,
            identity,
            inner,
        })
    }

    /// Escape hatch to the underlying SDK client, for callers that need
    /// curated RPCs (`create_sandbox`, `list_sandboxes`, ...) beyond what
    /// `parrot-core` wraps.
    pub fn sdk(&self) -> &OpenShellClient {
        &self.inner
    }

    /// Resolve which workspace to target for [`crate::exec::ExecStream`].
    ///
    /// Returns `explicit` verbatim when given. Otherwise lists the
    /// workspaces this identity can see: exactly one means "obviously
    /// that one" (the common case — a banker who's only a member of their
    /// own workspace); zero falls back to `None` (the unscoped, "default
    /// workspace" resolution); more than one is ambiguous and requires
    /// `--workspace` to disambiguate.
    pub async fn resolve_workspace(&self, explicit: Option<&str>) -> Result<Option<String>> {
        if let Some(name) = explicit {
            return Ok(Some(name.to_string()));
        }
        let workspaces = self.inner.list_workspaces(ListOptions::default()).await?;
        match workspaces.as_slice() {
            [] => Ok(None),
            [only] => Ok(Some(only.name.clone())),
            many => Err(ParrotError::AmbiguousWorkspace(
                many.iter().map(|w| w.name.clone()).collect(),
            )),
        }
    }
}

/// Load a gateway's own CA (the same self-signed CA that signs its server
/// cert, written under `mtls/` by `openshell gateway add`) so the SDK
/// trusts it instead of falling back to public web PKI roots.
///
/// Without this, every gateway with a self-signed or private-CA server
/// cert fails to connect at all (`connect error: transport error`) — a
/// generic message that gives no hint the cause is an untrusted cert, not
/// a network or auth problem. Confirmed live against a gateway requiring
/// both mTLS and OIDC: loading only this CA (no client certificate is
/// presented — `openshell-sdk` has no such field) is sufficient for an
/// OIDC-authenticated connection to succeed.
///
/// Reconstructed manually rather than via `openshell-bootstrap`, whose
/// path-resolution helpers are private to that crate — matches the
/// documented `$XDG_CONFIG_HOME/openshell/gateways/<name>/mtls/`
/// convention. Returns `None` (falling back to system roots, the correct
/// behavior for a publicly-trusted gateway cert) if `XDG_CONFIG_HOME`/
/// `HOME` can't be resolved or no `ca.crt` exists for this gateway.
fn load_gateway_ca_cert(gateway_name: &str) -> Option<Vec<u8>> {
    let config_home = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .ok()?;
    let ca_path = config_home
        .join("openshell/gateways")
        .join(gateway_name)
        .join("mtls/ca.crt");
    std::fs::read(&ca_path).ok()
}

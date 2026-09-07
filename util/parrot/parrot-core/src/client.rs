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

// SPDX-License-Identifier: Apache-2.0

use thiserror::Error;

pub type Result<T> = std::result::Result<T, ParrotError>;

#[derive(Debug, Error)]
pub enum ParrotError {
    #[error(
        "no active OpenShell gateway.\nSet one with: openshell gateway select <name>\nOr register one with: openshell gateway add <endpoint>"
    )]
    NoActiveGateway,

    #[error(
        "unknown gateway '{0}'.\nRegister it first: openshell gateway add <endpoint> --name {0}"
    )]
    UnknownGateway(String),

    #[error(
        "gateway '{gateway}' has no stored OIDC credentials; run `openshell gateway login {gateway}`"
    )]
    MissingOidcCredentials { gateway: String },

    #[error(
        "gateway '{gateway}' uses auth mode '{mode}', which parrot does not support yet (OIDC only)"
    )]
    UnsupportedAuthMode { gateway: String, mode: String },

    #[error(
        "multiple workspaces available ({}); pass --workspace to pick one",
        .0.join(", ")
    )]
    AmbiguousWorkspace(Vec<String>),

    #[error("OpenShell SDK error: {0}")]
    Sdk(#[from] openshell_sdk::SdkError),

    #[error("gateway bootstrap error: {0}")]
    Bootstrap(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

// SPDX-License-Identifier: Apache-2.0

//! Backend-agnostic core for parrot: OpenShell SDK integration, gateway/auth
//! resolution, live-streaming exec, JSONL classification, and the shared
//! theme/state types both `parrot-tui` and `parrot-gpui` render from.
//!
//! No ratatui or gpui dependency here on purpose — this crate is usable
//! headlessly.

pub mod agent;
pub mod auth;
pub mod client;
pub mod error;
pub mod exec;
pub mod gateway;
pub mod identity;
pub mod jsonl;
pub mod state;
pub mod theme;

pub use agent::{detect_passthrough_environment, Agent, TurnOptions};
pub use client::ParrotClient;
pub use error::{ParrotError, Result};
pub use exec::{ExecOutcome, ExecStream, StreamedExecOptions};
pub use gateway::GatewayContext;
pub use identity::Identity;
pub use jsonl::AgentEvent;
pub use state::{LogEntry, RunStatus, SessionState, StreamSource};
pub use theme::{Palette, Rgb, ThemeMode};

// Re-export so frontends don't need a direct `openshell-sdk` dependency
// for common types (e.g. constructing `StreamedExecOptions`/error matching).
pub use openshell_sdk::SdkError;

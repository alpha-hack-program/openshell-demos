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
pub mod markdown;
pub mod mcp_auth;
pub mod mcp_client;
pub mod state;
pub mod theme;

pub use agent::{detect_passthrough_environment, Agent, TurnOptions};
pub use client::ParrotClient;
pub use error::{ParrotError, Result};
pub use exec::{ExecOutcome, ExecStream, StreamedExecOptions};
pub use gateway::GatewayContext;
pub use identity::{identity_from_access_token, Identity};
pub use jsonl::{tool_result_text, AgentEvent};
pub use markdown::{render_inline_markdown, InlineRun, InlineStyle};
pub use mcp_auth::{start_login, McpAuthConfig, McpToken, PendingLogin};
pub use mcp_client::{unwrap_tool_output, McpGatewayClient, McpServerName};
pub use state::{LogEntry, RunStatus, SessionState, StreamSource};
pub use theme::{Palette, Rgb, ThemeMode};

// Re-exported so frontends can name `rmcp::model::{Tool, CallToolResult}`
// (sent over Tauri IPC as-is; both already derive `Serialize`) without
// their own direct `rmcp` dependency.
pub use rmcp::model::{CallToolResult, Tool as McpTool};

// Re-export so frontends don't need a direct `openshell-sdk` dependency
// for common types (e.g. constructing `StreamedExecOptions`/error matching,
// or rendering `ParrotClient::sandbox_phase`'s result).
pub use openshell_sdk::{SandboxPhase, SdkError};

// SPDX-License-Identifier: Apache-2.0

//! Live-streaming exec on top of `openshell-sdk`'s raw `exec_sandbox` RPC.
//!
//! `OpenShellClient::exec` (the curated surface) buffers stdout/stderr to
//! completion and returns once — the opposite of what parrot needs to relay
//! an agent CLI's output as it's emitted. This module drives the streaming
//! RPC directly via `raw_grpc()` and demuxes it into two line-oriented
//! channels (stdout/stderr kept separate, per the JSONL-on-stdout /
//! logs-on-stderr contract) plus a distinct outcome channel, so callers can
//! tell "the turn completed" apart from "the stream died".

use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;
use openshell_sdk::raw::proto::exec_sandbox_event::Payload;
use openshell_sdk::raw::ExecSandboxRequest;
use openshell_sdk::SdkError;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;

use crate::client::ParrotClient;
use crate::error::Result;

/// Options for a streaming exec. Mirrors [`openshell_sdk::ExecOptions`]
/// minus `stdin`, which the streaming path doesn't wire up yet.
#[derive(Clone, Debug, Default)]
pub struct StreamedExecOptions {
    pub workdir: Option<String>,
    pub environment: HashMap<String, String>,
    pub timeout: Option<Duration>,
}

/// How an exec stream ended.
#[derive(Clone, Debug)]
pub struct ExecOutcome {
    /// Process exit code, when the gateway reported one before the stream
    /// closed.
    pub exit_code: Option<i32>,
    /// `true` when the stream ended because the transport/gRPC call failed
    /// (network drop, gateway restart) rather than the process exiting
    /// normally.
    pub crashed: bool,
    pub crash_reason: Option<String>,
}

/// A live exec: separate line streams for stdout/stderr, plus the final
/// outcome once the process (or the stream) ends.
pub struct ExecStream {
    pub stdout: ReceiverStream<String>,
    pub stderr: ReceiverStream<String>,
    pub outcome: oneshot::Receiver<ExecOutcome>,
}

impl ParrotClient {
    /// Run `cmd` inside sandbox `name` in `workspace`, streaming
    /// stdout/stderr line-by-line as they arrive instead of buffering
    /// until the command finishes.
    ///
    /// `workspace` matters: the unscoped `GetSandboxRequest` the SDK sends
    /// when no workspace is given resolves against the `"default"`
    /// workspace server-side, not "any workspace the caller can see" — a
    /// banker whose sandbox lives in their own workspace (the common case
    /// in the keycloak-oidc demo) gets `not a member of workspace
    /// 'default'` if `workspace` is omitted. Pass `None` only when the
    /// sandbox genuinely lives in the default workspace (e.g. admin).
    pub async fn exec_streamed(
        &self,
        name: &str,
        workspace: Option<&str>,
        cmd: &[String],
        opts: StreamedExecOptions,
    ) -> Result<ExecStream> {
        let sandbox = match workspace {
            Some(workspace) => self.sdk().workspace(workspace).get_sandbox(name).await?,
            None => self.sdk().get_sandbox(name).await?,
        };
        let request = ExecSandboxRequest {
            sandbox_id: sandbox.id,
            command: cmd.to_vec(),
            workdir: opts.workdir.unwrap_or_default(),
            environment: opts.environment,
            timeout_seconds: opts
                .timeout
                .map_or(0, |d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX)),
            stdin: Vec::new(),
            tty: false,
            cols: 0,
            rows: 0,
        };

        // Open the stream under the same retry-once-on-`Unauthenticated`
        // policy the curated `exec()` uses for opening its stream. Mid-stream
        // token rotation is out of scope, same as upstream.
        let mut grpc = self.sdk().raw_grpc_fresh().await?;
        let mut response = grpc.exec_sandbox(request.clone()).await;
        if let Err(status) = &response {
            if status.code() == tonic::Code::Unauthenticated
                && self.sdk().force_refresh().await.unwrap_or(false)
            {
                let mut grpc = self.sdk().raw_grpc();
                response = grpc.exec_sandbox(request).await;
            }
        }
        let mut stream = response
            .map_err(|status| SdkError::connect(status.to_string()))?
            .into_inner();

        let (stdout_tx, stdout_rx) = mpsc::channel::<String>(64);
        let (stderr_tx, stderr_rx) = mpsc::channel::<String>(64);
        let (outcome_tx, outcome_rx) = oneshot::channel();

        tokio::spawn(async move {
            let mut stdout_buf: Vec<u8> = Vec::new();
            let mut stderr_buf: Vec<u8> = Vec::new();
            let mut exit_code = None;
            let mut crashed = false;
            let mut crash_reason = None;

            loop {
                match stream.next().await {
                    Some(Ok(event)) => match event.payload {
                        Some(Payload::Stdout(chunk)) => {
                            drain_lines(&mut stdout_buf, &chunk.data, &stdout_tx).await;
                        }
                        Some(Payload::Stderr(chunk)) => {
                            drain_lines(&mut stderr_buf, &chunk.data, &stderr_tx).await;
                        }
                        Some(Payload::Exit(exit)) => {
                            exit_code = Some(exit.exit_code);
                        }
                        None => {}
                    },
                    Some(Err(status)) => {
                        crashed = true;
                        crash_reason = Some(status.to_string());
                        break;
                    }
                    None => break,
                }
            }

            flush_remainder(stdout_buf, &stdout_tx).await;
            flush_remainder(stderr_buf, &stderr_tx).await;

            let _ = outcome_tx.send(ExecOutcome {
                exit_code,
                crashed,
                crash_reason,
            });
        });

        Ok(ExecStream {
            stdout: ReceiverStream::new(stdout_rx),
            stderr: ReceiverStream::new(stderr_rx),
            outcome: outcome_rx,
        })
    }
}

async fn drain_lines(buf: &mut Vec<u8>, chunk: &[u8], tx: &mpsc::Sender<String>) {
    buf.extend_from_slice(chunk);
    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
        let line: Vec<u8> = buf.drain(..=pos).collect();
        let line = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
        let _ = tx.send(line).await;
    }
}

async fn flush_remainder(buf: Vec<u8>, tx: &mpsc::Sender<String>) {
    if !buf.is_empty() {
        let _ = tx.send(String::from_utf8_lossy(&buf).into_owned()).await;
    }
}

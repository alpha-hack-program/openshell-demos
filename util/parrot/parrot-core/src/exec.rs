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
use openshell_sdk::raw::proto;
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
        // Fail fast with a clear "not found" / "not a member" error before
        // opening the exec stream — same existence/membership check as
        // before, even though the RPC itself now addresses the sandbox by
        // name rather than by the id this used to return.
        match workspace {
            Some(workspace) => {
                self.sdk().workspace(workspace).get_sandbox(name).await?;
            }
            None => {
                self.sdk().get_sandbox(name).await?;
            }
        };
        let request = ExecSandboxRequest {
            sandbox: name.to_string(),
            workspace_scope: Some(proto::workspace_selector(workspace.unwrap_or("default"))),
            command: cmd.to_vec(),
            workdir: opts.workdir.unwrap_or_default(),
            environment: opts.environment,
            execution_timeout: opts.timeout.map(|d| prost_types::Duration {
                seconds: i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
                nanos: i32::try_from(d.subsec_nanos()).unwrap_or(0),
            }),
            stdin: Vec::new(),
            tty: false,
            cols: 0,
            rows: 0,
            ..Default::default()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, TurnOptions};
    use crate::jsonl::extract_session_id;

    /// Drains one exec to completion, returning (session_id, exit_code) --
    /// the same sequential, blocking pattern `parrot-tui`'s `run_turn`
    /// uses (await exec_streamed, then drain the whole stream before
    /// returning), as opposed to `parrot-tauri`'s `submit_turn`, which
    /// spawns this same loop into the background and returns immediately.
    async fn drain_turn(exec: &mut ExecStream) -> (Option<String>, Option<i32>) {
        let mut session_id = None;
        // Must drain stderr too, not just stdout: it's a bounded channel
        // (capacity 64, see `exec_streamed`), so leaving it unread risks
        // the producer task blocking on a full channel if the process
        // writes enough to stderr, hanging this whole function.
        let mut stdout_done = false;
        let mut stderr_done = false;
        let mut outcome_done = false;
        let mut exit_code = None;
        loop {
            tokio::select! {
                biased;
                line = exec.stdout.next(), if !stdout_done => match line {
                    Some(line) => {
                        if let Some(id) = extract_session_id(&line) {
                            session_id = Some(id);
                        }
                    }
                    None => stdout_done = true,
                },
                line = exec.stderr.next(), if !stderr_done => match line {
                    Some(_) => {}
                    None => stderr_done = true,
                },
                outcome = &mut exec.outcome, if !outcome_done => {
                    if let Ok(outcome) = outcome {
                        exit_code = outcome.exit_code;
                    }
                    outcome_done = true;
                }
            }
            if stdout_done && stderr_done && outcome_done {
                break;
            }
        }
        (session_id, exit_code)
    }

    /// Reproduces `parrot-tui`'s exact sequential pattern against a real
    /// Codex sandbox: turn 1 with no `--resume`, capture the `thread_id`
    /// Codex reports, then turn 2 with `exec resume <thread_id>` --
    /// confirming whether the *shared* `agent.rs`/`exec.rs` resume
    /// mechanism itself threads sessions correctly when driven
    /// sequentially (one turn fully awaited before the next starts),
    /// independent of either frontend's own UI-layer code. Run manually:
    ///
    /// ```sh
    /// SANDBOX_NAME=aud-codex-bob WORKSPACE=bob \
    /// cargo test -p parrot-core exec::tests::live -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore]
    async fn live_codex_resume_threads_one_session_when_sequential() {
        let sandbox = std::env::var("SANDBOX_NAME").unwrap_or_else(|_| "aud-codex-bob".to_string());
        let workspace = std::env::var("WORKSPACE").unwrap_or_else(|_| "bob".to_string());
        let client = ParrotClient::connect(None, None)
            .await
            .expect("connect to OpenShell gateway");
        let environment = crate::agent::detect_passthrough_environment();

        let turn1_opts = TurnOptions {
            prompt: "Remember the word 'parrot-resume-test'. Reply with just 'ok'.",
            mcp_config: None,
            session_id: None,
            no_builtin_tools: true,
        };
        let command1 = Agent::Codex.build_turn(&turn1_opts);
        let mut exec1 = client
            .exec_streamed(&sandbox, Some(workspace.as_str()), &command1, StreamedExecOptions {
                environment: environment.clone(),
                ..Default::default()
            })
            .await
            .expect("exec_streamed turn 1");
        let (session_id_1, exit_code_1) = drain_turn(&mut exec1).await;
        println!("turn 1: session_id={session_id_1:?} exit_code={exit_code_1:?}");
        let session_id_1 = session_id_1.expect("turn 1 should report a thread_id");

        let turn2_opts = TurnOptions {
            prompt: "What word did I just ask you to remember?",
            mcp_config: None,
            session_id: Some(&session_id_1),
            no_builtin_tools: true,
        };
        let command2 = Agent::Codex.build_turn(&turn2_opts);
        let mut exec2 = client
            .exec_streamed(&sandbox, Some(workspace.as_str()), &command2, StreamedExecOptions {
                environment,
                ..Default::default()
            })
            .await
            .expect("exec_streamed turn 2");
        let (session_id_2, exit_code_2) = drain_turn(&mut exec2).await;
        println!("turn 2: session_id={session_id_2:?} exit_code={exit_code_2:?}");
        let session_id_2 = session_id_2.expect("turn 2 should report a thread_id");

        assert_eq!(
            session_id_1, session_id_2,
            "resuming with turn 1's thread_id should keep the same session, not start a new one"
        );
    }
}

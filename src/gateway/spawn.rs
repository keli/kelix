use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, Command};
use tokio::sync::{oneshot, Mutex};
use uuid::Uuid;

use crate::protocol::adapter_msg::{AdapterMessage, AdapterOutboundMessage};

use super::{GatewayCtx, GatewayOutbound, SessionHandle};

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

pub fn spawn_core_session(
    ctx: Arc<GatewayCtx>,
    session_id: String,
    core_session_id: String,
    resume: bool,
    config: Option<&Path>,
    working_dir: Option<&Path>,
    enabled_subagents: &[String],
    initial_prompt: Option<&str>,
) -> anyhow::Result<Arc<SessionHandle>> {
    spawn_core_session_force(
        ctx,
        session_id,
        core_session_id,
        resume,
        false,
        config,
        working_dir,
        enabled_subagents,
        initial_prompt,
    )
}

pub fn spawn_core_session_force(
    ctx: Arc<GatewayCtx>,
    session_id: String,
    core_session_id: String,
    resume: bool,
    force: bool,
    config: Option<&Path>,
    working_dir: Option<&Path>,
    enabled_subagents: &[String],
    initial_prompt: Option<&str>,
) -> anyhow::Result<Arc<SessionHandle>> {
    let mut command = Command::new(&ctx.core_bin);
    command.arg("core");
    if resume {
        command.arg("resume").arg(&core_session_id);
        if force {
            command.arg("--force");
        }
    } else {
        let config = config.ok_or_else(|| {
            anyhow::anyhow!("config path required to start a new session for {session_id}")
        })?;
        command
            .arg("start")
            .arg(config)
            .arg("--session-id")
            .arg(&core_session_id);
        if !enabled_subagents.is_empty() {
            command
                .arg("--enabled-subagents")
                .arg(enabled_subagents.join(","));
        }
        if let Some(prompt) = initial_prompt {
            command.arg("--prompt").arg(prompt);
        }
    }

    if let Some(dir) = working_dir {
        command.current_dir(dir);
    }

    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = command.spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("missing core stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("missing core stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("missing core stderr"))?;

    tokio::spawn(read_core_stdout(
        Arc::clone(&ctx),
        session_id,
        stdout,
        stderr,
    ));
    let (kill_tx, mut kill_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        tokio::select! {
            _ = child.wait() => {}
            _ = &mut kill_rx => {
                let _ = child.start_kill();
                if tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await.is_err() {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
            }
        }
    });

    Ok(Arc::new(SessionHandle {
        stdin: Mutex::new(stdin),
        kill_tx: Mutex::new(Some(kill_tx)),
    }))
}

pub async fn send_core_message(
    session: &SessionHandle,
    msg: &AdapterMessage,
) -> anyhow::Result<()> {
    let mut stdin = session.stdin.lock().await;
    let mut line = serde_json::to_vec(msg)?;
    line.push(b'\n');
    stdin.write_all(&line).await?;
    stdin.flush().await?;
    Ok(())
}

pub async fn force_kill_core_session(session: &SessionHandle) {
    let kill_tx = {
        let mut guard = session.kill_tx.lock().await;
        guard.take()
    };
    if let Some(tx) = kill_tx {
        let _ = tx.send(());
    }
}

pub async fn read_core_stdout(
    ctx: Arc<GatewayCtx>,
    session_id: String,
    stdout: tokio::process::ChildStdout,
    stderr: ChildStderr,
) {
    // @chunk gateway/core-stderr-stream
    // Stream core stderr lines in real time so debug diagnostics are visible
    // before core exits, while still collecting them for the final exit detail.
    let stderr_ctx = Arc::clone(&ctx);
    let stderr_session_id = session_id.clone();
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        let mut collected = Vec::new();

        while let Ok(Some(line)) = reader.next_line().await {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let line_text = trimmed.to_string();
            collected.push(line_text.clone());
            emit_gateway_info_detail(
                &stderr_ctx,
                format!("core stderr [{stderr_session_id}]"),
                Some(line_text),
                Some(new_id("info")),
            );
        }

        collected.join("\n")
    });
    // @end-chunk

    let mut lines = BufReader::new(stdout).lines();

    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let message = match serde_json::from_str::<AdapterOutboundMessage>(trimmed) {
                    Ok(v) => v,
                    Err(err) => {
                        emit_gateway_error(
                            &ctx,
                            format!("failed to parse core output for session {session_id}: {err}"),
                            Some(new_id("err")),
                        );
                        continue;
                    }
                };

                handle_core_message(&ctx, &session_id, message).await;
            }
            Ok(None) => {
                let was_active = {
                    let mut state = ctx.state.lock().await;
                    state.active_sessions.remove(&session_id).is_some()
                };
                let detail = stderr_task.await.ok().filter(|s| !s.is_empty());
                if was_active {
                    emit_gateway_error_detail(
                        &ctx,
                        format!("core process exited for session {session_id}"),
                        detail,
                        Some(new_id("err")),
                    );
                } else {
                    emit_gateway_info_detail(
                        &ctx,
                        format!("core process exited for session {session_id}"),
                        detail,
                        Some(new_id("info")),
                    );
                }
                break;
            }
            Err(err) => {
                emit_gateway_error(
                    &ctx,
                    format!("core read error for session {session_id}: {err}"),
                    Some(new_id("err")),
                );
                break;
            }
        }
    }
}

async fn handle_core_message(
    ctx: &Arc<GatewayCtx>,
    session_id: &str,
    message: AdapterOutboundMessage,
) {
    match &message {
        AdapterOutboundMessage::ApprovalRequired { request_id, .. } => {
            let mut state = ctx.state.lock().await;
            state
                .pending_approvals
                .insert(request_id.clone(), session_id.to_string());
        }
        AdapterOutboundMessage::ApprovalResponseAck { .. } => {
            let mut state = ctx.state.lock().await;
            state.pending_approvals.retain(|_, sid| sid != session_id);
        }
        AdapterOutboundMessage::SessionComplete { .. } => {
            let mut state = ctx.state.lock().await;
            state.active_sessions.remove(session_id);
            state.pending_approvals.retain(|_, sid| sid != session_id);
        }
        _ => {}
    }

    let _ = ctx.events_tx.send(GatewayOutbound::CoreEvent {
        session_id: session_id.to_string(),
        event: message,
    });
}

pub fn emit_gateway_error(ctx: &GatewayCtx, message: String, id: Option<String>) {
    emit_gateway_error_detail(ctx, message, None, id);
}

pub fn emit_gateway_error_detail(
    ctx: &GatewayCtx,
    message: String,
    detail: Option<String>,
    id: Option<String>,
) {
    let _ = ctx.events_tx.send(GatewayOutbound::GatewayError {
        id: id.unwrap_or_else(|| new_id("err")),
        message,
        detail,
    });
}

pub fn emit_gateway_info_detail(
    ctx: &GatewayCtx,
    message: String,
    detail: Option<String>,
    id: Option<String>,
) {
    let _ = ctx.events_tx.send(GatewayOutbound::GatewayInfo {
        id: id.unwrap_or_else(|| new_id("info")),
        message,
        detail,
    });
}

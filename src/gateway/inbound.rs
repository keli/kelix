use std::path::PathBuf;
use std::sync::Arc;

use crate::protocol::adapter_msg::AdapterMessage;

use super::spawn::{
    force_kill_core_session, new_id, send_core_message, spawn_core_session,
    spawn_core_session_force,
};
use super::{GatewayCtx, GatewayInbound, GatewayOutbound, SessionHandle};

// @chunk gateway/handle-inbound
pub async fn handle_inbound(ctx: &Arc<GatewayCtx>, inbound: GatewayInbound) -> anyhow::Result<()> {
    match inbound {
        GatewayInbound::SessionInit {
            id,
            session_id,
            config,
            working_dir,
            enabled_subagents,
            initial_prompt,
        } => {
            let _session = start_session(
                ctx,
                &session_id,
                config,
                working_dir,
                enabled_subagents,
                initial_prompt,
            )
            .await?;
            let _ = ctx.events_tx.send(GatewayOutbound::SessionReady {
                id,
                session_id,
                status: "started".to_string(),
            });
        }
        GatewayInbound::UserMessage {
            id,
            text,
            session_id,
            sender_id,
        } => {
            // @chunk gateway/user-message-no-session
            // If no active session exists for this session_id, refuse gracefully.
            // Clients must send SessionInit before sending messages.
            let session = {
                let state = ctx.state.lock().await;
                state.active_sessions.get(&session_id).map(Arc::clone)
            };
            // @end-chunk

            match session {
                Some(s) => {
                    let relay_id = id.clone();
                    let relay_text = text.clone();
                    let relay_sender_id = sender_id.clone();
                    send_core_message(
                        &s,
                        &AdapterMessage::UserMessage {
                            id,
                            text,
                            sender_id,
                            channel_id: Some(session_id.clone()),
                        },
                    )
                    .await?;
                    let _ = ctx.events_tx.send(GatewayOutbound::UserMessageRelay {
                        id: relay_id,
                        session_id,
                        text: relay_text,
                        sender_id: relay_sender_id,
                    });
                }
                None => {
                    let _ = ctx.events_tx.send(GatewayOutbound::NoSession {
                        id,
                        session_id,
                        message: "No active session. Start one with `kelix start <config>`."
                            .to_string(),
                    });
                }
            }
        }
        GatewayInbound::ApprovalResponse {
            id,
            request_id,
            choice,
            session_id,
        } => {
            let session_id = match session_id {
                Some(v) => v,
                None => {
                    let state = ctx.state.lock().await;
                    state.pending_approvals.get(&request_id).cloned().ok_or_else(|| {
                        anyhow::anyhow!(
                            "unknown approval request_id: {request_id}; provide session_id explicitly"
                        )
                    })?
                }
            };
            let session = require_active_session(ctx, &session_id).await?;
            send_core_message(
                &session,
                &AdapterMessage::ApprovalResponse {
                    id,
                    request_id,
                    choice,
                },
            )
            .await?;
        }
        GatewayInbound::DebugMode {
            id,
            enabled,
            session_id,
        } => {
            let session = require_active_session(ctx, &session_id).await?;
            send_core_message(&session, &AdapterMessage::DebugMode { id, enabled }).await?;
        }
        GatewayInbound::SessionResume {
            id,
            session_id,
            force,
        } => {
            let mut state = ctx.state.lock().await;
            if state.active_sessions.contains_key(&session_id) {
                let _ = ctx.events_tx.send(GatewayOutbound::SessionReady {
                    id,
                    session_id,
                    status: "already_active".to_string(),
                });
            } else {
                let session = spawn_core_session_force(
                    Arc::clone(ctx),
                    session_id.clone(),
                    session_id.clone(),
                    true,
                    force,
                    None,
                    None,
                    &[],
                    None,
                )?;
                state
                    .active_sessions
                    .insert(session_id.clone(), Arc::clone(&session));
                let _ = ctx.events_tx.send(GatewayOutbound::SessionReady {
                    id,
                    session_id,
                    status: "resumed".to_string(),
                });
            }
        }
        GatewayInbound::Shutdown { .. } => {
            eprintln!("kelix-gateway: shutdown requested via WebSocket");
            shutdown_all_sessions(ctx).await;
            let _ = ctx.shutdown_tx.send(true);
        }
        GatewayInbound::SessionEnd { _id: _, session_id } => {
            let removed = {
                let mut state = ctx.state.lock().await;
                state.active_sessions.remove(&session_id)
            };
            if let Some(session) = removed {
                let _ =
                    send_core_message(&session, &AdapterMessage::SessionEnd { id: new_id("end") })
                        .await;
            }
        }
    }

    Ok(())
}
// @end-chunk

// @chunk gateway/start-session
// Start a fresh session for the given session_id using the supplied config.
// Always creates a new core process; resume is handled by `kelix resume`.
pub async fn start_session(
    ctx: &Arc<GatewayCtx>,
    session_id: &str,
    config: PathBuf,
    working_dir: PathBuf,
    enabled_subagents: Vec<String>,
    initial_prompt: Option<String>,
) -> anyhow::Result<Arc<SessionHandle>> {
    let mut state = ctx.state.lock().await;

    // If already active (e.g. duplicate SessionInit), return existing handle.
    if let Some(session) = state.active_sessions.get(session_id) {
        return Ok(Arc::clone(session));
    }

    let session = spawn_core_session(
        Arc::clone(ctx),
        session_id.to_string(),
        session_id.to_string(),
        false,
        Some(&config),
        Some(&working_dir),
        &enabled_subagents,
        initial_prompt.as_deref(),
    )?;

    state
        .active_sessions
        .insert(session_id.to_string(), Arc::clone(&session));

    Ok(session)
}
// @end-chunk

/// Return the active session handle for a session_id, error if none is running.
pub async fn require_active_session(
    ctx: &Arc<GatewayCtx>,
    session_id: &str,
) -> anyhow::Result<Arc<SessionHandle>> {
    let state = ctx.state.lock().await;
    state
        .active_sessions
        .get(session_id)
        .map(Arc::clone)
        .ok_or_else(|| anyhow::anyhow!("no active session for session_id {session_id}"))
}

// @chunk gateway/shutdown-all-sessions
// Send session_end to every active core process so each can suspend cleanly
// before the gateway exits. If the stdin write fails (pipe already broken),
// the process is already gone so nothing further is needed.
pub async fn shutdown_all_sessions(ctx: &Arc<GatewayCtx>) {
    let sessions: Vec<Arc<SessionHandle>> = {
        let state = ctx.state.lock().await;
        state.active_sessions.values().map(Arc::clone).collect()
    };
    for session in sessions {
        let msg = AdapterMessage::SessionEnd {
            id: new_id("shutdown"),
        };
        let _ = send_core_message(&session, &msg).await;
    }

    // Give core processes a short grace period to suspend cleanly.
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(15);
    loop {
        let remaining: Vec<Arc<SessionHandle>> = {
            let state = ctx.state.lock().await;
            state.active_sessions.values().map(Arc::clone).collect()
        };
        if remaining.is_empty() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            for session in remaining {
                force_kill_core_session(&session).await;
            }
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}
// @end-chunk

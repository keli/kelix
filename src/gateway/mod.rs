mod inbound;
mod spawn;

use anyhow::Context;
use clap::Args;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::process::ChildStdin;
use tokio::sync::{broadcast, oneshot, watch, Mutex};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use crate::protocol::adapter_msg::AdapterOutboundMessage;

use inbound::{handle_inbound, shutdown_all_sessions};
use spawn::{emit_gateway_error, new_id};

// @chunk gateway/options
// Gateway is stateless between restarts. Sessions are started fresh on each
// SessionInit; resuming a suspended session is done via `kelix resume`.
#[derive(Debug, Clone, Args)]
pub struct GatewayOptions {
    /// Path to the kelix core executable.
    #[arg(long, default_value = "kelix")]
    pub core_bin: String,

    /// Bind address for the WebSocket server.
    #[arg(long, default_value = "127.0.0.1:9000")]
    pub listen_addr: String,
}
// @end-chunk

pub(crate) struct SessionHandle {
    pub(crate) stdin: Mutex<ChildStdin>,
    pub(crate) kill_tx: Mutex<Option<oneshot::Sender<()>>>,
}

// @chunk gateway/state
// active_sessions:  session_id -> running process handle (in-memory only)
// pending_approvals: request_id -> session_id
pub(crate) struct GatewayState {
    pub(crate) active_sessions: HashMap<String, Arc<SessionHandle>>,
    pub(crate) pending_approvals: HashMap<String, String>,
}
// @end-chunk

pub(crate) struct GatewayCtx {
    pub(crate) core_bin: String,
    pub(crate) state: Arc<Mutex<GatewayState>>,
    pub(crate) events_tx: broadcast::Sender<GatewayOutbound>,
    pub(crate) shutdown_tx: watch::Sender<bool>,
}

// @chunk gateway/inbound-types
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum GatewayInbound {
    /// Sent by a client when it wants to start a new session.
    /// Always starts a fresh session; use kelix resume for resuming.
    SessionInit {
        id: String,
        session_id: String,
        config: PathBuf,
        working_dir: PathBuf,
        #[serde(default)]
        enabled_subagents: Vec<String>,
        #[serde(default)]
        initial_prompt: Option<String>,
    },
    UserMessage {
        id: String,
        text: String,
        session_id: String,
        #[serde(default)]
        sender_id: Option<String>,
    },
    ApprovalResponse {
        id: String,
        request_id: String,
        choice: String,
        #[serde(default)]
        session_id: Option<String>,
    },
    DebugMode {
        id: String,
        #[serde(default)]
        enabled: Option<bool>,
        session_id: String,
    },
    SessionEnd {
        #[serde(rename = "id")]
        _id: String,
        session_id: String,
    },
    /// Resume a suspended session by session_id.
    SessionResume {
        id: String,
        session_id: String,
        #[serde(default)]
        force: bool,
    },
    /// Gracefully shut down the gateway process.
    Shutdown { id: String },
}
// @end-chunk

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum GatewayOutbound {
    CoreEvent {
        session_id: String,
        event: AdapterOutboundMessage,
    },
    UserMessageRelay {
        id: String,
        session_id: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sender_id: Option<String>,
    },
    SessionReady {
        id: String,
        session_id: String,
        status: String,
    },
    GatewayError {
        id: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    GatewayInfo {
        id: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// Sent when a UserMessage arrives for a session_id with no active session.
    NoSession {
        id: String,
        session_id: String,
        message: String,
    },
}

// @chunk gateway/run
pub async fn run(options: GatewayOptions) -> anyhow::Result<()> {
    let (events_tx, _) = broadcast::channel::<GatewayOutbound>(512);
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let ctx = Arc::new(GatewayCtx {
        core_bin: options.core_bin,
        state: Arc::new(Mutex::new(GatewayState {
            active_sessions: HashMap::new(),
            pending_approvals: HashMap::new(),
        })),
        events_tx,
        shutdown_tx,
    });

    let listener = TcpListener::bind(&options.listen_addr)
        .await
        .with_context(|| format!("failed to bind {}", options.listen_addr))?;
    eprintln!("kelix-gateway: listening on ws://{}", options.listen_addr);

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, peer) = result?;
                let ctx = Arc::clone(&ctx);
                tokio::spawn(async move {
                    if let Err(err) = handle_connection(ctx, stream).await {
                        eprintln!("kelix-gateway: connection {peer:?} failed: {err}");
                    }
                });
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    eprintln!("kelix-gateway: shutting down");
                    break;
                }
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("kelix-gateway: received Ctrl-C, shutting down");
                shutdown_all_sessions(&ctx).await;
                break;
            }
        }
    }

    Ok(())
}
// @end-chunk

async fn handle_connection(ctx: Arc<GatewayCtx>, stream: TcpStream) -> anyhow::Result<()> {
    let ws = accept_async(stream).await?;
    let (mut ws_sink, mut ws_stream) = ws.split();
    let mut events_rx = ctx.events_tx.subscribe();

    let writer = tokio::spawn(async move {
        while let Ok(event) = events_rx.recv().await {
            let payload = match serde_json::to_string(&event) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if ws_sink.send(Message::Text(payload)).await.is_err() {
                break;
            }
        }
    });

    while let Some(frame) = ws_stream.next().await {
        let frame = match frame {
            Ok(v) => v,
            Err(err) => {
                emit_gateway_error(
                    &ctx,
                    format!("websocket read error: {err}"),
                    Some(new_id("err")),
                );
                break;
            }
        };

        if frame.is_close() {
            break;
        }
        let Message::Text(text) = frame else {
            continue;
        };

        let inbound = match serde_json::from_str::<GatewayInbound>(&text) {
            Ok(v) => v,
            Err(err) => {
                emit_gateway_error(
                    &ctx,
                    format!("invalid inbound message: {err}"),
                    Some(new_id("err")),
                );
                continue;
            }
        };

        if let Err(err) = handle_inbound(&ctx, inbound).await {
            emit_gateway_error(&ctx, err.to_string(), Some(new_id("err")));
        }
    }

    writer.abort();
    Ok(())
}

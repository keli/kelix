use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::protocol::adapter_msg::AdapterOutboundMessage;

pub mod api;
pub mod gateway;
pub mod handlers;

use api::{load_state, telegram_get_me, TelegramUpdate};
use gateway::poll_telegram_loop;
use handlers::{generate_claim_code, handle_gateway_event, handle_telegram_update};

macro_rules! logln {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let mut stderr = std::io::stderr();
        let _ = write!(stderr, "\r");
        let _ = writeln!(stderr, $($arg)*);
    }};
}
pub(super) use logln;

// @chunk telegram-adapter/options
// Telegram provider options are grouped under the unified `kelix adapter`
// command so other providers can be added without creating more top-level
// subcommands.
#[derive(Debug, Clone, Args)]
pub struct TelegramOptions {
    /// Telegram bot token. If omitted, TELEGRAM_BOT_TOKEN is used.
    #[arg(long)]
    pub bot_token: Option<String>,

    /// Gateway WebSocket URL.
    #[arg(long, default_value = "ws://127.0.0.1:9000")]
    pub gateway_url: String,

    /// Long-poll timeout for Telegram getUpdates.
    #[arg(long, default_value_t = 30)]
    pub poll_timeout_secs: u64,

    /// Adapter state path. Default: ~/.kelix/adapters/telegram-state.json
    #[arg(long)]
    pub state_path: Option<PathBuf>,
}
// @end-chunk

#[derive(Debug)]
pub enum RuntimeEvent {
    Telegram(TelegramUpdate),
    Gateway(GatewayOutbound),
}

#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub session_id: String,
    pub chat_id: i64,
    pub options: Vec<String>,
}

// @chunk telegram-adapter/whitelist
// Whitelist is seeded by /claim on first run. Until at least one user has
// claimed, the adapter prints a one-time passcode at startup. Only whitelisted
// users can drive sessions; the /claim command itself is always accepted.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelegramState {
    pub chat_bindings: HashMap<i64, String>,
    /// Telegram user IDs that are allowed to send session messages.
    #[serde(default)]
    pub whitelist: HashSet<i64>,
}
// @end-chunk

#[derive(Debug, Clone)]
pub struct BotProfile {
    pub id: i64,
    pub username: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GatewayOutbound {
    CoreEvent {
        session_id: String,
        event: AdapterOutboundMessage,
    },
    UserMessageRelay {
        #[serde(rename = "id")]
        _id: String,
        session_id: String,
        text: String,
        #[serde(default)]
        sender_id: Option<String>,
    },
    SessionReady {
        #[serde(rename = "id")]
        _id: String,
        #[serde(rename = "session_id")]
        _session_id: String,
        #[serde(rename = "status")]
        _status: String,
    },
    GatewayError {
        #[serde(rename = "id")]
        _id: String,
        message: String,
        #[serde(default)]
        detail: Option<String>,
    },
    GatewayInfo {
        #[serde(rename = "id")]
        _id: String,
        message: String,
        #[serde(default)]
        detail: Option<String>,
    },
    NoSession {
        #[serde(rename = "id")]
        _id: String,
        session_id: String,
        message: String,
    },
}

// @chunk telegram-adapter/runtime
pub async fn run(options: TelegramOptions, reset: bool, ready_file: Option<PathBuf>) -> Result<()> {
    let state_path = resolve_state_path(options.state_path)?;

    if reset {
        api::save_state(&state_path, &TelegramState::default()).await?;
        logln!("kelix-adapter: state reset: {}", state_path.display());
        return Ok(());
    }

    let (token, token_source) = resolve_bot_token(options.bot_token)?;
    logln!("kelix-adapter: provider=telegram");
    logln!("kelix-adapter: gateway={}", options.gateway_url);
    logln!("kelix-adapter: state_path={}", state_path.display());
    logln!("kelix-adapter: token_source={token_source}");

    let bot_profile = telegram_get_me(&token).await?;
    logln!(
        "kelix-adapter: telegram_bot=@{} id={}",
        bot_profile.username,
        bot_profile.id
    );

    let mut state = load_state(&state_path).await?;
    let mut pending_approvals: HashMap<String, PendingApproval> = HashMap::new();

    // Generate a one-time claim code if no user has been whitelisted yet.
    // The code is intentionally ephemeral: it lives only for this process
    // lifetime and is never written to disk.
    let claim_code: Option<String> = if state.whitelist.is_empty() {
        let code = generate_claim_code();
        logln!("kelix-adapter: whitelist is empty");
        logln!("kelix-adapter: send '/claim {code}' to the bot to claim admin access");
        Some(code)
    } else {
        None
    };

    let (ws_stream, _) = connect_async(&options.gateway_url)
        .await
        .with_context(|| format!("failed to connect gateway at {}", options.gateway_url))?;
    let (mut ws_sink, mut ws_stream) = ws_stream.split();

    let (event_tx, mut event_rx) = mpsc::channel::<RuntimeEvent>(1024);
    let (ws_tx, mut ws_rx) = mpsc::unbounded_channel::<String>();

    tokio::spawn(async move {
        while let Some(payload) = ws_rx.recv().await {
            if ws_sink.send(Message::Text(payload)).await.is_err() {
                break;
            }
        }
    });

    let gateway_events_tx = event_tx.clone();
    tokio::spawn(async move {
        while let Some(frame) = ws_stream.next().await {
            let Ok(frame) = frame else {
                break;
            };
            if !frame.is_text() {
                continue;
            }
            let Message::Text(text) = frame else {
                continue;
            };
            if let Ok(event) = serde_json::from_str::<GatewayOutbound>(&text) {
                let _ = gateway_events_tx.send(RuntimeEvent::Gateway(event)).await;
            }
        }
    });

    let telegram_events_tx = event_tx.clone();
    let token_for_poll = token.clone();
    let poll_timeout = options.poll_timeout_secs;
    tokio::spawn(async move {
        if let Err(err) = poll_telegram_loop(token_for_poll, poll_timeout, telegram_events_tx).await
        {
            logln!("telegram-adapter: polling stopped: {err}");
        }
    });

    if let Some(path) = ready_file.as_deref() {
        write_ready_file(path)?;
        logln!("kelix-adapter: ready");
    }

    while let Some(event) = event_rx.recv().await {
        match event {
            RuntimeEvent::Telegram(update) => {
                handle_telegram_update(
                    &token,
                    &bot_profile,
                    &ws_tx,
                    &state_path,
                    &mut state,
                    &mut pending_approvals,
                    claim_code.as_deref(),
                    update,
                )
                .await?;
            }
            RuntimeEvent::Gateway(outbound) => {
                handle_gateway_event(&token, &state, &mut pending_approvals, outbound).await?;
            }
        }
    }

    Ok(())
}
// @end-chunk

fn write_ready_file(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, b"ready\n")
        .with_context(|| format!("failed to write ready file {}", path.display()))?;
    Ok(())
}

fn resolve_bot_token(input: Option<String>) -> Result<(String, &'static str)> {
    if let Some(token) = input.filter(|v| !v.trim().is_empty()) {
        return Ok((token, "cli_flag"));
    }
    if let Ok(token) = std::env::var("TELEGRAM_BOT_TOKEN") {
        if !token.trim().is_empty() {
            return Ok((token, "env:TELEGRAM_BOT_TOKEN"));
        }
    }
    Err(anyhow::anyhow!(
        "telegram bot token missing; pass --bot-token or set TELEGRAM_BOT_TOKEN"
    ))
}

fn resolve_state_path(input: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = input {
        return Ok(path);
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot resolve home directory"))?;
    Ok(home
        .join(".kelix")
        .join("adapters")
        .join("telegram-state.json"))
}

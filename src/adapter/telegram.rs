use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use crate::protocol::adapter_msg::AdapterOutboundMessage;

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

    /// Path to kelix executable used for `kelix list --json`.
    #[arg(long, default_value = "kelix")]
    pub core_bin: String,

    /// Long-poll timeout for Telegram getUpdates.
    #[arg(long, default_value_t = 30)]
    pub poll_timeout_secs: u64,

    /// Adapter state path. Default: ~/.kelix/adapters/telegram-state.json
    #[arg(long)]
    pub state_path: Option<PathBuf>,
}
// @end-chunk

#[derive(Debug)]
enum RuntimeEvent {
    Telegram(TelegramUpdate),
    Gateway(GatewayOutbound),
}

#[derive(Debug, Clone)]
struct PendingApproval {
    session_id: String,
    chat_id: i64,
    options: Vec<String>,
}

// @chunk telegram-adapter/whitelist
// Whitelist is seeded by /claim on first run. Until at least one user has
// claimed, the adapter prints a one-time passcode at startup. Only whitelisted
// users can drive sessions; the /claim command itself is always accepted.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TelegramState {
    chat_bindings: HashMap<i64, String>,
    /// Telegram user IDs that are allowed to send session messages.
    #[serde(default)]
    whitelist: HashSet<i64>,
}
// @end-chunk

#[derive(Debug, Deserialize)]
struct SessionList {
    sessions: Vec<SessionEntry>,
}

#[derive(Debug, Deserialize)]
struct SessionEntry {
    id: String,
}

#[derive(Debug, Clone)]
struct BotProfile {
    id: i64,
    username: String,
}

// @chunk telegram-adapter/runtime
pub async fn run(options: TelegramOptions, reset: bool) -> Result<()> {
    let state_path = resolve_state_path(options.state_path)?;

    if reset {
        save_state(&state_path, &TelegramState::default()).await?;
        eprintln!("kelix-adapter: state reset: {}", state_path.display());
        return Ok(());
    }

    let (token, token_source) = resolve_bot_token(options.bot_token)?;
    eprintln!("kelix-adapter: provider=telegram");
    eprintln!("kelix-adapter: gateway={}", options.gateway_url);
    eprintln!("kelix-adapter: core_bin={}", options.core_bin);
    eprintln!("kelix-adapter: state_path={}", state_path.display());
    eprintln!("kelix-adapter: token_source={token_source}");

    let bot_profile = telegram_get_me(&token).await?;
    eprintln!(
        "kelix-adapter: telegram_bot=@{} id={}",
        bot_profile.username, bot_profile.id
    );

    let mut state = load_state(&state_path).await?;
    let mut pending_approvals: HashMap<String, PendingApproval> = HashMap::new();

    // Generate a one-time claim code if no user has been whitelisted yet.
    // The code is intentionally ephemeral: it lives only for this process
    // lifetime and is never written to disk.
    let claim_code: Option<String> = if state.whitelist.is_empty() {
        let code = generate_claim_code();
        eprintln!("kelix-adapter: whitelist is empty");
        eprintln!("kelix-adapter: send '/claim {code}' to the bot to claim admin access");
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
            eprintln!("telegram-adapter: polling stopped: {err}");
        }
    });

    while let Some(event) = event_rx.recv().await {
        match event {
            RuntimeEvent::Telegram(update) => {
                handle_telegram_update(
                    &token,
                    &bot_profile,
                    &options.core_bin,
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

// @chunk telegram-adapter/event-handlers
async fn handle_telegram_update(
    token: &str,
    bot_profile: &BotProfile,
    core_bin: &str,
    ws_tx: &mpsc::UnboundedSender<String>,
    state_path: &Path,
    state: &mut TelegramState,
    pending_approvals: &mut HashMap<String, PendingApproval>,
    claim_code: Option<&str>,
    update: TelegramUpdate,
) -> Result<()> {
    let Some(message) = update.message.or(update.edited_message) else {
        return Ok(());
    };
    // Ignore messages from bots (including self).
    if message.from.as_ref().map(|u| u.is_bot).unwrap_or(false) {
        return Ok(());
    }
    if message
        .from
        .as_ref()
        .map(|u| u.id == bot_profile.id)
        .unwrap_or(false)
    {
        return Ok(());
    }

    let Some(raw_text) = message.text.as_deref() else {
        return Ok(());
    };
    let chat_id = message.chat.id;
    let user_id = message.from.as_ref().map(|u| u.id);

    // Handle /claim before the group-only and whitelist checks so it works
    // in both private chats and groups, regardless of whitelist status.
    if let Some((cmd, args)) = parse_command(raw_text) {
        if cmd == "claim" {
            return handle_claim(token, state, state_path, chat_id, user_id, claim_code, args)
                .await;
        }
    }

    // Session-driving messages are group/supergroup only.
    if message.chat.kind != "group" && message.chat.kind != "supergroup" {
        return Ok(());
    }

    // Enforce whitelist. If no users are whitelisted the bot is unclaimed and
    // all session messages are silently dropped until someone claims it.
    if let Some(uid) = user_id {
        if !state.whitelist.is_empty() && !state.whitelist.contains(&uid) {
            return Ok(());
        }
    } else {
        // No sender identity — drop.
        return Ok(());
    }

    let sender_id = user_id
        .map(|uid| format!("tg:{uid}"))
        .unwrap_or_else(|| "tg:unknown".to_string());

    if let Some((cmd, args)) = parse_command(raw_text) {
        match cmd.as_str() {
            "status" => {
                let body = match state.chat_bindings.get(&chat_id) {
                    Some(session_id) => format!("bound session: {session_id}"),
                    None => {
                        "not bound. rename group to an existing session and talk, or use /rebind"
                            .to_string()
                    }
                };
                telegram_send_message(token, chat_id, &body).await?;
                return Ok(());
            }
            "rebind" => {
                if let Some(session_id) = auto_bind_chat(
                    core_bin,
                    ws_tx,
                    state_path,
                    state,
                    chat_id,
                    message.chat.title.as_deref(),
                )
                .await?
                {
                    telegram_send_message(
                        token,
                        chat_id,
                        &format!("rebound to session: {session_id}"),
                    )
                    .await?;
                } else {
                    telegram_send_message(
                        token,
                        chat_id,
                        "no matching session found for this group title",
                    )
                    .await?;
                }
                return Ok(());
            }
            "approve" => {
                let mut parts = args.split_whitespace();
                let request_id = parts.next().unwrap_or_default();
                let raw_choice = parts.next().unwrap_or_default();
                if request_id.is_empty() || raw_choice.is_empty() {
                    telegram_send_message(
                        token,
                        chat_id,
                        "usage: /approve <request_id> <choice|index>",
                    )
                    .await?;
                    return Ok(());
                }
                let pending = match pending_approvals.get(request_id) {
                    Some(v) if v.chat_id == chat_id => v.clone(),
                    _ => {
                        telegram_send_message(token, chat_id, "unknown approval request_id")
                            .await?;
                        return Ok(());
                    }
                };
                let Some(choice) = select_choice(raw_choice, &pending.options) else {
                    telegram_send_message(token, chat_id, "invalid choice").await?;
                    return Ok(());
                };
                send_gateway_approval_response(ws_tx, &pending.session_id, request_id, &choice)?;
                pending_approvals.remove(request_id);
                telegram_send_message(token, chat_id, "approval sent").await?;
                return Ok(());
            }
            "run" | "ask" => {
                let Some(prompt_text) = normalize_prompt_text(args, &bot_profile.username) else {
                    telegram_send_message(token, chat_id, "usage: /ask <message>").await?;
                    return Ok(());
                };
                let session_id = ensure_bound_session(
                    token,
                    core_bin,
                    ws_tx,
                    state_path,
                    state,
                    chat_id,
                    message.chat.title.as_deref(),
                )
                .await?;
                send_gateway_session_resume(ws_tx, &session_id)?;
                send_gateway_user_message(ws_tx, &session_id, &sender_id, &prompt_text)?;
                return Ok(());
            }
            "help" => {
                telegram_send_message(
                    token,
                    chat_id,
                    "commands: /ask <text>, /run <text>, /status, /rebind, /approve <request_id> <choice|index>, /claim <code>",
                )
                .await?;
                return Ok(());
            }
            _ => {}
        }
    }

    let Some(prompt_text) = normalize_prompt_text(raw_text, &bot_profile.username) else {
        return Ok(());
    };
    let session_id = ensure_bound_session(
        token,
        core_bin,
        ws_tx,
        state_path,
        state,
        chat_id,
        message.chat.title.as_deref(),
    )
    .await?;

    // Keep Telegram behavior close to TUI: opportunistically resume before
    // each message so suspended sessions can receive input immediately.
    send_gateway_session_resume(ws_tx, &session_id)?;
    send_gateway_user_message(ws_tx, &session_id, &sender_id, &prompt_text)?;
    Ok(())
}

async fn ensure_bound_session(
    token: &str,
    core_bin: &str,
    ws_tx: &mpsc::UnboundedSender<String>,
    state_path: &Path,
    state: &mut TelegramState,
    chat_id: i64,
    chat_title: Option<&str>,
) -> Result<String> {
    if let Some(existing) = state.chat_bindings.get(&chat_id).cloned() {
        return Ok(existing);
    }

    let Some(bound) =
        auto_bind_chat(core_bin, ws_tx, state_path, state, chat_id, chat_title).await?
    else {
        telegram_send_message(
            token,
            chat_id,
            "no matching session for group title. start locally with --session <group_title> then send /rebind",
        )
        .await?;
        anyhow::bail!("session not bound for chat {chat_id}");
    };

    telegram_send_message(token, chat_id, &format!("auto-bound to session: {bound}")).await?;
    Ok(bound)
}

async fn handle_gateway_event(
    token: &str,
    state: &TelegramState,
    pending_approvals: &mut HashMap<String, PendingApproval>,
    outbound: GatewayOutbound,
) -> Result<()> {
    match outbound {
        GatewayOutbound::CoreEvent { session_id, event } => {
            let Some(chat_id) = chat_for_session(state, &session_id) else {
                return Ok(());
            };
            match event {
                AdapterOutboundMessage::AgentMessage { text, .. } => {
                    telegram_send_message(token, chat_id, &text).await?;
                }
                AdapterOutboundMessage::Notify { text, level, .. } => {
                    telegram_send_message(token, chat_id, &format!("[{level}] {text}")).await?;
                }
                AdapterOutboundMessage::ApprovalRequired {
                    request_id,
                    message,
                    options,
                    ..
                } => {
                    pending_approvals.insert(
                        request_id.clone(),
                        PendingApproval {
                            session_id: session_id.clone(),
                            chat_id,
                            options: options.clone(),
                        },
                    );
                    let choices = options
                        .iter()
                        .enumerate()
                        .map(|(idx, option)| format!("{}. {}", idx + 1, option))
                        .collect::<Vec<_>>()
                        .join("\n");
                    telegram_send_message(
                        token,
                        chat_id,
                        &format!(
                            "approval required:\n{message}\nrequest_id: {request_id}\n{choices}\nreply with /approve {request_id} <choice|index>"
                        ),
                    )
                    .await?;
                }
                AdapterOutboundMessage::SessionComplete { summary, .. } => {
                    telegram_send_message(token, chat_id, &format!("session complete: {summary}"))
                        .await?;
                }
                AdapterOutboundMessage::SessionError { reason, .. } => {
                    telegram_send_message(token, chat_id, &format!("session error: {reason}"))
                        .await?;
                }
                AdapterOutboundMessage::Error { code, message, .. } => {
                    telegram_send_message(
                        token,
                        chat_id,
                        &format!("core error [{code}]: {message}"),
                    )
                    .await?;
                }
                AdapterOutboundMessage::UserMessageAck { .. }
                | AdapterOutboundMessage::ApprovalResponseAck { .. } => {}
            }
        }
        GatewayOutbound::UserMessageRelay {
            session_id,
            text,
            sender_id,
            ..
        } => {
            // Skip Telegram-originated inputs to avoid duplicate echo loops.
            if sender_id
                .as_deref()
                .map(|v| v.starts_with("tg:"))
                .unwrap_or(false)
            {
                return Ok(());
            }
            if let Some(chat_id) = chat_for_session(state, &session_id) {
                let prefix = sender_id.unwrap_or_else(|| "user".to_string());
                telegram_send_message(token, chat_id, &format!("[{prefix}] {text}")).await?;
            }
        }
        GatewayOutbound::NoSession {
            session_id,
            message,
            ..
        } => {
            if let Some(chat_id) = chat_for_session(state, &session_id) {
                telegram_send_message(
                    token,
                    chat_id,
                    &format!("{message}; run local start/resume for session: {session_id}"),
                )
                .await?;
            }
        }
        GatewayOutbound::GatewayError {
            message, detail, ..
        } => {
            if let Some(extra) = detail {
                eprintln!("telegram-adapter: gateway error: {message} ({extra})");
            } else {
                eprintln!("telegram-adapter: gateway error: {message}");
            }
        }
        GatewayOutbound::GatewayInfo {
            message, detail, ..
        } => {
            if let Some(extra) = detail {
                eprintln!("telegram-adapter: gateway info: {message} ({extra})");
            } else {
                eprintln!("telegram-adapter: gateway info: {message}");
            }
        }
        GatewayOutbound::SessionReady { .. } => {}
    }

    Ok(())
}
// @end-chunk

// @chunk telegram-adapter/transport-and-storage
async fn poll_telegram_loop(
    token: String,
    poll_timeout_secs: u64,
    event_tx: mpsc::Sender<RuntimeEvent>,
) -> Result<()> {
    let mut offset: i64 = 0;
    loop {
        let payload = json!({
            "offset": offset,
            "timeout": poll_timeout_secs,
            "allowed_updates": ["message", "edited_message"]
        });
        match telegram_api::<Vec<TelegramUpdate>>(&token, "getUpdates", &payload).await {
            Ok(updates) => {
                for update in updates {
                    offset = offset.max(update.update_id + 1);
                    if event_tx.send(RuntimeEvent::Telegram(update)).await.is_err() {
                        return Ok(());
                    }
                }
            }
            Err(err) => {
                eprintln!("telegram-adapter: getUpdates failed: {err}");
                sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

async fn auto_bind_chat(
    core_bin: &str,
    ws_tx: &mpsc::UnboundedSender<String>,
    state_path: &Path,
    state: &mut TelegramState,
    chat_id: i64,
    chat_title: Option<&str>,
) -> Result<Option<String>> {
    let Some(title) = chat_title.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if !session_exists(core_bin, title).await? {
        return Ok(None);
    }

    state.chat_bindings.insert(chat_id, title.to_string());
    save_state(state_path, state).await?;
    send_gateway_session_resume(ws_tx, title)?;
    Ok(Some(title.to_string()))
}

async fn session_exists(core_bin: &str, session_id: &str) -> Result<bool> {
    let output = Command::new(core_bin)
        .arg("list")
        .arg("--json")
        .output()
        .await
        .with_context(|| format!("failed to run `{core_bin} list --json`"))?;
    if !output.status.success() {
        anyhow::bail!(
            "`{core_bin} list --json` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let parsed: SessionList = serde_json::from_slice(&output.stdout)
        .context("failed to parse `kelix list --json` output")?;
    Ok(parsed.sessions.iter().any(|entry| entry.id == session_id))
}

async fn load_state(path: &Path) -> Result<TelegramState> {
    if !path.exists() {
        return Ok(TelegramState::default());
    }
    let body = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read adapter state {}", path.display()))?;
    let state: TelegramState = serde_json::from_str(&body)
        .with_context(|| format!("failed to parse adapter state {}", path.display()))?;
    Ok(state)
}

async fn save_state(path: &Path, state: &TelegramState) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let data = serde_json::to_vec_pretty(state)?;
    tokio::fs::write(path, data)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}
// @end-chunk

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

fn normalize_prompt_text(raw: &str, bot_username: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lowered = trimmed.to_ascii_lowercase();
    let mention = format!("@{}", bot_username.to_ascii_lowercase());
    if lowered.starts_with(&mention) {
        let mut rest = trimmed[mention.len()..].trim_start();
        while let Some(ch) = rest.chars().next() {
            if ch == ':' || ch == ',' || ch == '，' {
                rest = rest[ch.len_utf8()..].trim_start();
            } else {
                break;
            }
        }
        if rest.is_empty() {
            return None;
        }
        return Some(rest.to_string());
    }

    Some(trimmed.to_string())
}

fn chat_for_session(state: &TelegramState, session_id: &str) -> Option<i64> {
    state
        .chat_bindings
        .iter()
        .find_map(|(chat_id, bound)| (bound == session_id).then_some(*chat_id))
}

// @chunk telegram-adapter/whitelist-claim
// generate_claim_code produces a short alphanumeric one-time passcode.
// It uses the first 12 hex characters of a UUID, which gives ~48 bits of
// entropy — sufficient for an interactive first-run flow.
fn generate_claim_code() -> String {
    Uuid::new_v4().simple().to_string()[..12].to_string()
}

async fn handle_claim(
    token: &str,
    state: &mut TelegramState,
    state_path: &Path,
    chat_id: i64,
    user_id: Option<i64>,
    claim_code: Option<&str>,
    args: &str,
) -> Result<()> {
    let Some(uid) = user_id else {
        telegram_send_message(token, chat_id, "cannot identify sender").await?;
        return Ok(());
    };

    // Already whitelisted — no-op.
    if state.whitelist.contains(&uid) {
        telegram_send_message(token, chat_id, "you are already whitelisted").await?;
        return Ok(());
    }

    let submitted = args.trim();
    match claim_code {
        None => {
            // Whitelist already has at least one user; claiming is closed.
            telegram_send_message(token, chat_id, "whitelist is already claimed").await?;
        }
        Some(code) if submitted == code => {
            state.whitelist.insert(uid);
            save_state(state_path, state).await?;
            eprintln!("kelix-adapter: whitelist claimed by user_id={uid}");
            telegram_send_message(token, chat_id, "you have been added to the whitelist").await?;
        }
        Some(_) => {
            telegram_send_message(token, chat_id, "invalid claim code").await?;
        }
    }
    Ok(())
}
// @end-chunk

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

fn parse_command(text: &str) -> Option<(String, &str)> {
    let trimmed = text.trim();
    let rest = trimmed.strip_prefix('/')?;
    let mut parts = rest.splitn(2, char::is_whitespace);
    let command = parts.next()?.split('@').next()?.trim().to_ascii_lowercase();
    let args = parts.next().unwrap_or("").trim();
    Some((command, args))
}

fn select_choice(input: &str, options: &[String]) -> Option<String> {
    if let Ok(idx) = input.parse::<usize>() {
        if (1..=options.len()).contains(&idx) {
            return Some(options[idx - 1].clone());
        }
    }
    options
        .iter()
        .find(|opt| opt.eq_ignore_ascii_case(input))
        .cloned()
}

fn send_gateway_user_message(
    ws_tx: &mpsc::UnboundedSender<String>,
    session_id: &str,
    sender_id: &str,
    text: &str,
) -> Result<()> {
    let payload = json!({
        "id": new_id("msg"),
        "type": "user_message",
        "text": text,
        "session_id": session_id,
        "sender_id": sender_id,
    });
    ws_tx
        .send(payload.to_string())
        .context("gateway sender closed")?;
    Ok(())
}

fn send_gateway_approval_response(
    ws_tx: &mpsc::UnboundedSender<String>,
    session_id: &str,
    request_id: &str,
    choice: &str,
) -> Result<()> {
    let payload = json!({
        "id": new_id("approve"),
        "type": "approval_response",
        "request_id": request_id,
        "choice": choice,
        "session_id": session_id,
    });
    ws_tx
        .send(payload.to_string())
        .context("gateway sender closed")?;
    Ok(())
}

fn send_gateway_session_resume(
    ws_tx: &mpsc::UnboundedSender<String>,
    session_id: &str,
) -> Result<()> {
    let payload = json!({
        "id": new_id("resume"),
        "type": "session_resume",
        "session_id": session_id,
        "force": false,
    });
    ws_tx
        .send(payload.to_string())
        .context("gateway sender closed")?;
    Ok(())
}

async fn telegram_send_message(token: &str, chat_id: i64, text: &str) -> Result<()> {
    let payload = json!({
        "chat_id": chat_id,
        "text": text,
    });
    let _ = telegram_api::<serde_json::Value>(token, "sendMessage", &payload).await?;
    Ok(())
}

async fn telegram_get_me(token: &str) -> Result<BotProfile> {
    let payload = json!({});
    let user = telegram_api::<TelegramUser>(token, "getMe", &payload).await?;
    let username = user
        .username
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("telegram bot username is required"))?;
    Ok(BotProfile {
        id: user.id,
        username,
    })
}

async fn telegram_api<T: for<'de> Deserialize<'de>>(
    token: &str,
    method: &str,
    payload: &serde_json::Value,
) -> Result<T> {
    let url = format!("https://api.telegram.org/bot{token}/{method}");
    let output = Command::new("curl")
        .arg("--silent")
        .arg("--show-error")
        .arg("--fail")
        .arg("-X")
        .arg("POST")
        .arg(&url)
        .arg("-H")
        .arg("Content-Type: application/json")
        .arg("-d")
        .arg(payload.to_string())
        .output()
        .await
        .with_context(|| format!("failed to execute curl for Telegram method {method}"))?;

    if !output.status.success() {
        anyhow::bail!(
            "Telegram API call {} failed: {}",
            method,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let response: TelegramResponse<T> = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("failed to parse Telegram response for {method}"))?;
    if !response.ok {
        let desc = response
            .description
            .unwrap_or_else(|| "unknown Telegram API error".to_string());
        anyhow::bail!("Telegram API {} returned error: {}", method, desc);
    }
    Ok(response.result)
}

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: T,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    #[serde(default)]
    message: Option<TelegramMessage>,
    #[serde(default)]
    edited_message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    chat: TelegramChat,
    #[serde(default)]
    from: Option<TelegramUser>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramChat {
    id: i64,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramUser {
    id: i64,
    #[serde(default)]
    is_bot: bool,
    #[serde(default)]
    username: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum GatewayOutbound {
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

#[cfg(test)]
mod tests {
    use super::{parse_command, select_choice};

    #[test]
    fn parse_command_strips_bot_suffix() {
        let parsed = parse_command("/rebind@my_bot").unwrap();
        assert_eq!(parsed.0, "rebind");
        assert_eq!(parsed.1, "");
    }

    #[test]
    fn parse_command_keeps_args() {
        let parsed = parse_command("/approve req-1 yes").unwrap();
        assert_eq!(parsed.0, "approve");
        assert_eq!(parsed.1, "req-1 yes");
    }

    #[test]
    fn select_choice_supports_index() {
        let options = vec!["yes".to_string(), "no".to_string()];
        assert_eq!(select_choice("2", &options), Some("no".to_string()));
    }

    #[test]
    fn select_choice_supports_text_case_insensitive() {
        let options = vec!["Skip".to_string(), "Approve".to_string()];
        assert_eq!(
            select_choice("approve", &options),
            Some("Approve".to_string())
        );
    }

    #[test]
    fn generate_claim_code_has_expected_length() {
        let code = super::generate_claim_code();
        assert_eq!(code.len(), 12);
        assert!(code.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn generate_claim_code_is_unique() {
        let a = super::generate_claim_code();
        let b = super::generate_claim_code();
        assert_ne!(a, b);
    }
}

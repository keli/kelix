use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::protocol::adapter_msg::AdapterOutboundMessage;
use crate::session::index::SessionIndex;

use super::api::{save_state, telegram_send_message, TelegramUpdate};
use super::gateway::{
    send_gateway_approval_response, send_gateway_session_resume, send_gateway_user_message,
};
use super::{BotProfile, GatewayOutbound, PendingApproval, TelegramState};

// @chunk telegram-adapter/update-handler
// Entry point for all inbound Telegram updates. Filters bots and anonymous
// senders, dispatches slash commands, then falls through to session message
// routing.
pub async fn handle_telegram_update(
    token: &str,
    bot_profile: &BotProfile,
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
// @end-chunk

// @chunk telegram-adapter/gateway-event-handler
// Translates gateway outbound events into Telegram messages for the bound chat.
pub async fn handle_gateway_event(
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
                super::logln!("telegram-adapter: gateway error: {message} ({extra})");
            } else {
                super::logln!("telegram-adapter: gateway error: {message}");
            }
        }
        GatewayOutbound::GatewayInfo {
            message, detail, ..
        } => {
            // Avoid duplicating "core process exited" info lines on shared terminals.
            // The interactive TUI already prints and redraws prompt for this event.
            if message.starts_with("core process exited for session ") {
                return Ok(());
            }
            if let Some(extra) = detail {
                super::logln!("telegram-adapter: gateway info: {message} ({extra})");
            } else {
                super::logln!("telegram-adapter: gateway info: {message}");
            }
        }
        GatewayOutbound::SessionReady { .. } => {}
    }

    Ok(())
}
// @end-chunk

async fn ensure_bound_session(
    token: &str,
    ws_tx: &mpsc::UnboundedSender<String>,
    state_path: &Path,
    state: &mut TelegramState,
    chat_id: i64,
    chat_title: Option<&str>,
) -> Result<String> {
    if let Some(existing) = state.chat_bindings.get(&chat_id).cloned() {
        return Ok(existing);
    }

    let Some(bound) = auto_bind_chat(ws_tx, state_path, state, chat_id, chat_title).await? else {
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

async fn auto_bind_chat(
    ws_tx: &mpsc::UnboundedSender<String>,
    state_path: &Path,
    state: &mut TelegramState,
    chat_id: i64,
    chat_title: Option<&str>,
) -> Result<Option<String>> {
    let Some(title) = chat_title.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if !session_exists(title).await? {
        return Ok(None);
    }

    state.chat_bindings.insert(chat_id, title.to_string());
    save_state(state_path, state).await?;
    send_gateway_session_resume(ws_tx, title)?;
    Ok(Some(title.to_string()))
}

async fn session_exists(session_id: &str) -> Result<bool> {
    let index = SessionIndex::load()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("failed to load session index")?;
    Ok(index.sessions.iter().any(|entry| entry.id == session_id))
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
pub fn generate_claim_code() -> String {
    Uuid::new_v4().simple().to_string()[..12].to_string()
}

pub async fn handle_claim(
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
            super::logln!("kelix-adapter: whitelist claimed by user_id={uid}");
            telegram_send_message(token, chat_id, "you have been added to the whitelist").await?;
        }
        Some(_) => {
            telegram_send_message(token, chat_id, "invalid claim code").await?;
        }
    }
    Ok(())
}
// @end-chunk

pub fn parse_command(text: &str) -> Option<(String, &str)> {
    let trimmed = text.trim();
    let rest = trimmed.strip_prefix('/')?;
    let mut parts = rest.splitn(2, char::is_whitespace);
    let command = parts.next()?.split('@').next()?.trim().to_ascii_lowercase();
    let args = parts.next().unwrap_or("").trim();
    Some((command, args))
}

pub fn normalize_prompt_text(raw: &str, bot_username: &str) -> Option<String> {
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

pub fn select_choice(input: &str, options: &[String]) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::{generate_claim_code, normalize_prompt_text, parse_command, select_choice};

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
        let code = generate_claim_code();
        assert_eq!(code.len(), 12);
        assert!(code.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn generate_claim_code_is_unique() {
        let a = generate_claim_code();
        let b = generate_claim_code();
        assert_ne!(a, b);
    }

    #[test]
    fn normalize_prompt_text_strips_mention() {
        assert_eq!(
            normalize_prompt_text("@mybot: hello", "mybot"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn normalize_prompt_text_returns_none_for_mention_only() {
        assert_eq!(normalize_prompt_text("@mybot", "mybot"), None);
    }
}

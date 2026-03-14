mod input;
mod messages;
mod terminal;

use clap::Args;
use crossterm::event::{self, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::enable_raw_mode;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{mpsc, watch, Mutex};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::protocol::adapter_msg::AdapterOutboundMessage;

use input::{backspace_char_at_cursor, delete_char_at_cursor, insert_char_at_cursor};
use messages::{
    approval_response_msg, debug_mode_msg, parse_debug_arg, select_approval_choice,
    session_end_msg, session_init_msg, session_resume_msg, shutdown_msg, user_message_msg,
};
use terminal::{clear_screen, print_help, print_line, print_scoped, redraw_prompt, RawModeGuard};

// @chunk tui-client/options
#[derive(Debug, Clone, Args)]
pub struct TuiOptions {
    /// WebSocket URL for the gateway.
    #[arg(long, default_value = "ws://127.0.0.1:9000")]
    pub url: String,

    /// Session id for this session.
    #[arg(long)]
    pub session: String,

    /// Optional sender_id attached to user_message.
    #[arg(long, default_value = "kelix-tui")]
    pub sender_id: String,

    /// Path to kelix.toml for this session (sent as SessionInit on connect).
    /// Not required when attaching to an existing session.
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,

    /// Working directory used by gateway when spawning core for SessionInit.
    pub working_dir: std::path::PathBuf,

    /// Comma-separated subagent allowlist (used only when starting a new session).
    #[arg(long, value_delimiter = ',')]
    pub enabled_subagents: Vec<String>,

    /// Optional first message sent automatically after connecting.
    #[arg(long)]
    pub initial_message: Option<String>,

    /// Resume a suspended session instead of starting a new one.
    #[arg(long)]
    pub resume: bool,

    /// Force resume even if the crash limit has been hit (used with --resume).
    #[arg(long)]
    pub force: bool,
}
// @end-chunk

#[derive(Debug, Clone)]
struct PendingApproval {
    request_id: String,
    options: Vec<String>,
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
        session_id: String,
        status: String,
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

// @chunk tui-client/run
pub async fn run(options: TuiOptions) -> anyhow::Result<()> {
    let (ws_stream, _) = connect_async(&options.url).await?;
    let (mut ws_sink, mut ws_stream) = ws_stream.split();
    let (key_tx, mut key_rx) = mpsc::channel::<KeyEvent>(128);
    let output_lock = Arc::new(Mutex::new(()));

    enable_raw_mode()?;
    let _raw_guard = RawModeGuard;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let shutdown_rx_pump = shutdown_rx.clone();
    tokio::task::spawn_blocking(move || loop {
        if *shutdown_rx_pump.borrow() {
            break;
        }
        if event::poll(std::time::Duration::from_millis(50)).unwrap_or(false) {
            match event::read() {
                Ok(event::Event::Key(key)) => {
                    if key_tx.blocking_send(key).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }
    });

    let mut input_buf = String::new();
    let mut cursor_char_pos = 0usize;

    print_line(
        &output_lock,
        &format!("connected: {}", options.url),
        &input_buf,
        cursor_char_pos,
    )
    .await;
    print_line(
        &output_lock,
        &format!("active session: {}", options.session),
        &input_buf,
        cursor_char_pos,
    )
    .await;
    print_line(
        &output_lock,
        "type /help for commands",
        &input_buf,
        cursor_char_pos,
    )
    .await;

    let mut active_session = options.session.clone();
    let sender_id = options.sender_id.clone();
    let mut pending_approvals: HashMap<String, PendingApproval> = HashMap::new();
    let mut pending_inputs: HashSet<String> = HashSet::new();

    // Send SessionResume or SessionInit depending on mode.
    if options.resume {
        ws_sink
            .send(Message::Text(session_resume_msg(
                &active_session,
                options.force,
            )))
            .await?;
        if let Some(initial) = options.initial_message.clone() {
            ws_sink
                .send(Message::Text(user_message_msg(
                    &active_session,
                    &sender_id,
                    &initial,
                )))
                .await?;
        }
    } else if let Some(ref config) = options.config {
        ws_sink
            .send(Message::Text(session_init_msg(
                &active_session,
                config,
                &options.working_dir,
                &options.enabled_subagents,
                options.initial_message.as_deref(),
            )))
            .await?;
        if let Some(ref initial) = options.initial_message {
            print_line(
                &output_lock,
                &format!("[you] {initial}"),
                &input_buf,
                cursor_char_pos,
            )
            .await;
        }
    }

    redraw_prompt(&output_lock, &input_buf, cursor_char_pos).await;

    'main: loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                print_line(
                    &output_lock,
                    "[info] leaving local tui client",
                    &input_buf,
                    cursor_char_pos,
                )
                .await;
                break 'main;
            }
            key = key_rx.recv() => {
                let Some(key) = key else {
                    break;
                };

                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    print_line(
                        &output_lock,
                        "[info] leaving local tui client",
                        &input_buf,
                        cursor_char_pos,
                    )
                    .await;
                    break 'main;
                }

                match key.code {
                    KeyCode::Enter => {
                        let mut text = input_buf.trim().to_string();
                        input_buf.clear();
                        cursor_char_pos = 0;
                        print_line(&output_lock, "", &input_buf, cursor_char_pos).await;

                        if text.is_empty() {
                            continue;
                        }

                        if let Some(rest) = text.strip_prefix("//") {
                            text = format!("/{rest}");
                        } else if let Some(cmd) = text.strip_prefix('/') {
                            let normalized = cmd.trim();
                            match normalized {
                                "help" => {
                                    print_help(&output_lock, &input_buf, cursor_char_pos).await;
                                }
                                "quit" | "exit" => {
                                    print_line(
                                        &output_lock,
                                        "[info] leaving local tui client",
                                        &input_buf,
                                        cursor_char_pos,
                                    )
                                    .await;
                                    break 'main;
                                }
                                "clear" => {
                                    clear_screen(&output_lock, &input_buf, cursor_char_pos).await;
                                }
                                "resume" => {
                                    ws_sink
                                        .send(Message::Text(session_resume_msg(&active_session, false)))
                                        .await?;
                                    pending_approvals.remove(&active_session);
                                    pending_inputs.remove(&active_session);
                                    print_line(
                                        &output_lock,
                                        &format!("[info] session resume requested: {active_session}"),
                                        &input_buf,
                                        cursor_char_pos,
                                    )
                                    .await;
                                }
                                _ if normalized.starts_with("resume ") => {
                                    let next = normalized["resume ".len()..].trim();
                                    if next.is_empty() {
                                        print_line(
                                            &output_lock,
                                            "[warning] usage: /resume [session_id]",
                                            &input_buf,
                                            cursor_char_pos,
                                        )
                                        .await;
                                    } else {
                                        active_session = next.to_string();
                                        ws_sink
                                            .send(Message::Text(session_resume_msg(&active_session, false)))
                                            .await?;
                                        pending_approvals.remove(&active_session);
                                        pending_inputs.remove(&active_session);
                                        print_line(
                                            &output_lock,
                                            &format!("[info] session resume requested: {active_session}"),
                                            &input_buf,
                                            cursor_char_pos,
                                        )
                                        .await;
                                    }
                                }
                                "debug" => {
                                    ws_sink
                                        .send(Message::Text(debug_mode_msg(&active_session, None)))
                                        .await?;
                                }
                                _ if normalized.starts_with("debug ") => {
                                    let arg = normalized["debug ".len()..].trim();
                                    if let Some(enabled) = parse_debug_arg(arg) {
                                        ws_sink
                                            .send(Message::Text(debug_mode_msg(
                                                &active_session,
                                                Some(enabled),
                                            )))
                                            .await?;
                                    } else {
                                        print_line(
                                            &output_lock,
                                            "[warning] usage: /debug [on|off]",
                                            &input_buf,
                                            cursor_char_pos,
                                        )
                                        .await;
                                    }
                                }
                                _ if normalized.starts_with("session ") => {
                                    let next = normalized["session ".len()..].trim();
                                    if next.is_empty() {
                                        print_line(
                                            &output_lock,
                                            "[warning] usage: /session <session_id>",
                                            &input_buf,
                                            cursor_char_pos,
                                        )
                                        .await;
                                    } else {
                                        active_session = next.to_string();
                                        print_line(
                                            &output_lock,
                                            &format!("active session: {}", active_session),
                                            &input_buf,
                                            cursor_char_pos,
                                        )
                                        .await;
                                    }
                                }
                                "shutdown" => {
                                    ws_sink
                                        .send(Message::Text(shutdown_msg()))
                                        .await?;
                                    print_line(
                                        &output_lock,
                                        "[info] shutdown sent to gateway",
                                        &input_buf,
                                        cursor_char_pos,
                                    )
                                    .await;
                                    break 'main;
                                }
                                "suspend" | "end" | "done" => {
                                    ws_sink
                                        .send(Message::Text(session_end_msg(&active_session)))
                                        .await?;
                                    pending_approvals.remove(&active_session);
                                    pending_inputs.remove(&active_session);
                                    print_line(
                                        &output_lock,
                                        &format!(
                                            "[info] session end requested (suspend): {active_session}"
                                        ),
                                        &input_buf,
                                        cursor_char_pos,
                                    )
                                    .await;
                                }
                                _ if normalized.starts_with("approve ") => {
                                    let rest = normalized["approve ".len()..].trim();
                                    let mut parts = rest.split_whitespace();
                                    if let (Some(request_id), Some(choice)) = (parts.next(), parts.next()) {
                                        ws_sink
                                            .send(Message::Text(approval_response_msg(
                                                &active_session,
                                                request_id,
                                                choice,
                                            )))
                                            .await?;
                                    } else {
                                        print_line(
                                            &output_lock,
                                            "[warning] usage: /approve <request_id> <choice>",
                                            &input_buf,
                                            cursor_char_pos,
                                        )
                                        .await;
                                    }
                                }
                                _ => {
                                    print_line(
                                        &output_lock,
                                        &format!("[warning] unknown command '/{normalized}'"),
                                        &input_buf,
                                        cursor_char_pos,
                                    )
                                    .await;
                                    print_line(
                                        &output_lock,
                                        "[info] type /help to see built-in commands",
                                        &input_buf,
                                        cursor_char_pos,
                                    )
                                    .await;
                                }
                            }
                            continue;
                        }

                        if let Some(pending) = pending_approvals.get(&active_session).cloned() {
                            if let Some(choice) = select_approval_choice(&text, &pending.options) {
                                pending_approvals.remove(&active_session);
                                ws_sink
                                    .send(Message::Text(approval_response_msg(
                                        &active_session,
                                        &pending.request_id,
                                        &choice,
                                    )))
                                    .await?;
                            } else {
                                print_line(
                                    &output_lock,
                                    "[warning] invalid approval choice; type option number or exact option text",
                                    &input_buf,
                                    cursor_char_pos,
                                )
                                .await;
                            }
                            continue;
                        }

                        ws_sink
                            .send(Message::Text(user_message_msg(
                                &active_session,
                                &sender_id,
                                &text,
                            )))
                            .await?;
                        print_line(&output_lock, &format!("[you] {text}"), &input_buf, cursor_char_pos).await;
                        pending_inputs.remove(&active_session);
                    }
                    KeyCode::Backspace => {
                        backspace_char_at_cursor(&mut input_buf, &mut cursor_char_pos);
                        redraw_prompt(&output_lock, &input_buf, cursor_char_pos).await;
                    }
                    KeyCode::Delete => {
                        delete_char_at_cursor(&mut input_buf, cursor_char_pos);
                        redraw_prompt(&output_lock, &input_buf, cursor_char_pos).await;
                    }
                    KeyCode::Left => {
                        if cursor_char_pos > 0 {
                            cursor_char_pos -= 1;
                            redraw_prompt(&output_lock, &input_buf, cursor_char_pos).await;
                        }
                    }
                    KeyCode::Right => {
                        let len = input_buf.chars().count();
                        if cursor_char_pos < len {
                            cursor_char_pos += 1;
                            redraw_prompt(&output_lock, &input_buf, cursor_char_pos).await;
                        }
                    }
                    KeyCode::Home => {
                        cursor_char_pos = 0;
                        redraw_prompt(&output_lock, &input_buf, cursor_char_pos).await;
                    }
                    KeyCode::End => {
                        cursor_char_pos = input_buf.chars().count();
                        redraw_prompt(&output_lock, &input_buf, cursor_char_pos).await;
                    }
                    KeyCode::Char(c) => {
                        insert_char_at_cursor(&mut input_buf, &mut cursor_char_pos, c);
                        redraw_prompt(&output_lock, &input_buf, cursor_char_pos).await;
                    }
                    _ => {}
                }
            }
            frame = ws_stream.next() => {
                let Some(frame) = frame else {
                    break;
                };
                match frame {
                    Ok(Message::Text(text)) => {
                        match serde_json::from_str::<GatewayOutbound>(&text) {
                            Ok(GatewayOutbound::CoreEvent { session_id, event }) => {
                                match event {
                                    AdapterOutboundMessage::UserMessageAck { .. }
                                    | AdapterOutboundMessage::ApprovalResponseAck { .. } => {}
                                    AdapterOutboundMessage::AgentMessage { text, .. } => {
                                        pending_inputs.insert(session_id.clone());
                                        print_scoped(
                                            &output_lock,
                                            &session_id,
                                            &active_session,
                                            &format!("[input] {text}"),
                                            &input_buf,
                                            cursor_char_pos,
                                        )
                                        .await;
                                    }
                                    AdapterOutboundMessage::Notify { text, level, .. } => {
                                        print_scoped(
                                            &output_lock,
                                            &session_id,
                                            &active_session,
                                            &format!("[{level}] {text}"),
                                            &input_buf,
                                            cursor_char_pos,
                                        )
                                        .await;
                                    }
                                    AdapterOutboundMessage::ApprovalRequired {
                                        request_id,
                                        message,
                                        options,
                                        ..
                                    } => {
                                        pending_approvals.insert(
                                            session_id.clone(),
                                            PendingApproval {
                                                request_id,
                                                options: options.clone(),
                                            },
                                        );
                                        print_scoped(
                                            &output_lock,
                                            &session_id,
                                            &active_session,
                                            &format!("[approval] {message}"),
                                            &input_buf,
                                            cursor_char_pos,
                                        )
                                        .await;
                                        for (idx, option) in options.iter().enumerate() {
                                            print_scoped(
                                                &output_lock,
                                                &session_id,
                                                &active_session,
                                                &format!("[approval] {}. {}", idx + 1, option),
                                                &input_buf,
                                                cursor_char_pos,
                                            )
                                            .await;
                                        }
                                    }
                                    AdapterOutboundMessage::SessionComplete { summary, .. } => {
                                        pending_approvals.remove(&session_id);
                                        pending_inputs.remove(&session_id);
                                        print_scoped(
                                            &output_lock,
                                            &session_id,
                                            &active_session,
                                            &format!("[complete] {summary}"),
                                            &input_buf,
                                            cursor_char_pos,
                                        )
                                        .await;
                                    }
                                    AdapterOutboundMessage::SessionError { reason, .. } => {
                                        pending_approvals.remove(&session_id);
                                        pending_inputs.remove(&session_id);
                                        print_scoped(
                                            &output_lock,
                                            &session_id,
                                            &active_session,
                                            &format!("[error] {reason}"),
                                            &input_buf,
                                            cursor_char_pos,
                                        )
                                        .await;
                                    }
                                    AdapterOutboundMessage::Error { code, message, .. } => {
                                        print_scoped(
                                            &output_lock,
                                            &session_id,
                                            &active_session,
                                            &format!("[error] {code}: {message}"),
                                            &input_buf,
                                            cursor_char_pos,
                                        )
                                        .await;
                                    }
                                }
                            }
                            Ok(GatewayOutbound::UserMessageRelay {
                                session_id,
                                text,
                                sender_id,
                                ..
                            }) => {
                                let sender = sender_id.unwrap_or_else(|| "user".to_string());
                                print_scoped(
                                    &output_lock,
                                    &session_id,
                                    &active_session,
                                    &format!("[relay:{sender}] {text}"),
                                    &input_buf,
                                    cursor_char_pos,
                                )
                                .await;
                            }
                            Ok(GatewayOutbound::SessionReady { session_id, status, .. }) => {
                                print_scoped(
                                    &output_lock,
                                    &session_id,
                                    &active_session,
                                    &format!("[info] session {status}: {session_id}"),
                                    &input_buf,
                                    cursor_char_pos,
                                )
                                .await;
                            }
                            Ok(GatewayOutbound::GatewayError { message, detail, .. }) => {
                                print_line(
                                    &output_lock,
                                    &format!("[error] {message}"),
                                    &input_buf,
                                    cursor_char_pos,
                                )
                                .await;
                                if let Some(detail) = detail {
                                    for line in detail.lines() {
                                        print_line(
                                            &output_lock,
                                            &format!("[error] {line}"),
                                            &input_buf,
                                            cursor_char_pos,
                                        )
                                        .await;
                                    }
                                }
                            }
                            Ok(GatewayOutbound::GatewayInfo { message, detail, .. }) => {
                                print_line(
                                    &output_lock,
                                    &format!("[info] {message}"),
                                    &input_buf,
                                    cursor_char_pos,
                                )
                                .await;
                                if let Some(detail) = detail {
                                    for line in detail.lines() {
                                        print_line(
                                            &output_lock,
                                            &format!("[info] {line}"),
                                            &input_buf,
                                            cursor_char_pos,
                                        )
                                        .await;
                                    }
                                }
                            }
                            Ok(GatewayOutbound::NoSession { session_id, message, .. }) => {
                                print_scoped(
                                    &output_lock,
                                    &session_id,
                                    &active_session,
                                    &format!("[no-session] {message}"),
                                    &input_buf,
                                    cursor_char_pos,
                                )
                                .await;
                            }
                            Err(_) => match serde_json::from_str::<serde_json::Value>(&text) {
                                Ok(v) => {
                                    print_line(
                                        &output_lock,
                                        &serde_json::to_string_pretty(&v)?,
                                        &input_buf,
                                        cursor_char_pos,
                                    )
                                    .await;
                                }
                                Err(_) => {
                                    print_line(&output_lock, &text, &input_buf, cursor_char_pos).await;
                                }
                            },
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(_) => {}
                    Err(err) => {
                        print_line(
                            &output_lock,
                            &format!("websocket error: {err}"),
                            &input_buf,
                            cursor_char_pos,
                        )
                        .await;
                        break;
                    }
                }
            }
        }
    }

    let _ = shutdown_tx.send(true);
    Ok(())
}
// @end-chunk

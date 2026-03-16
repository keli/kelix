use anyhow::{Context, Result};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};
use uuid::Uuid;

use super::api::{telegram_api, TelegramUpdate};
use super::RuntimeEvent;

// @chunk telegram-adapter/gateway-send
// Helpers for sending inbound messages to the gateway over the WebSocket
// write channel. Each function constructs the JSON payload and enqueues it.
pub fn send_gateway_user_message(
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

pub fn send_gateway_approval_response(
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

pub fn send_gateway_session_resume(
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
// @end-chunk

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

// @chunk telegram-adapter/poll-loop
// Long-poll loop: calls getUpdates with a configurable timeout and forwards
// each update to the event channel. Retries with a 2 s back-off on failure.
pub async fn poll_telegram_loop(
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
                super::logln!("telegram-adapter: getUpdates failed: {err}");
                sleep(Duration::from_secs(2)).await;
            }
        }
    }
}
// @end-chunk

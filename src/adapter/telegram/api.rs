use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;

use super::{BotProfile, TelegramState};

// @chunk telegram-adapter/api-client
// Thin wrapper around the Telegram Bot API. All HTTP is done here; callers
// work with typed structs only.
pub async fn telegram_send_message(token: &str, chat_id: i64, text: &str) -> Result<()> {
    let payload = json!({ "chat_id": chat_id, "text": text });
    let _ = telegram_api::<serde_json::Value>(token, "sendMessage", &payload).await?;
    Ok(())
}

pub async fn telegram_get_me(token: &str) -> Result<BotProfile> {
    let user = telegram_api::<TelegramUser>(token, "getMe", &json!({})).await?;
    let username = user
        .username
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("telegram bot username is required"))?;
    Ok(BotProfile {
        id: user.id,
        username,
    })
}

pub async fn telegram_api<T: for<'de> Deserialize<'de>>(
    token: &str,
    method: &str,
    payload: &serde_json::Value,
) -> Result<T> {
    let url = format!("https://api.telegram.org/bot{token}/{method}");
    let response: TelegramApiResponse<T> = reqwest::Client::new()
        .post(&url)
        .json(payload)
        .send()
        .await
        .with_context(|| format!("failed to send request for Telegram method {method}"))?
        .json()
        .await
        .with_context(|| format!("failed to parse Telegram response for {method}"))?;
    if !response.ok {
        let desc = response
            .description
            .unwrap_or_else(|| "unknown Telegram API error".to_string());
        anyhow::bail!("Telegram API {} returned error: {}", method, desc);
    }
    Ok(response.result)
}
// @end-chunk

// @chunk telegram-adapter/state-storage
// Adapter state (whitelist + chat bindings) is persisted as JSON.
// Returns a default on missing file; writes are non-atomic (single path).
pub async fn load_state(path: &Path) -> Result<TelegramState> {
    if !path.exists() {
        return Ok(TelegramState::default());
    }
    let body = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read adapter state {}", path.display()))?;
    serde_json::from_str(&body)
        .with_context(|| format!("failed to parse adapter state {}", path.display()))
}

pub async fn save_state(path: &Path, state: &TelegramState) -> Result<()> {
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

#[derive(Debug, Deserialize)]
struct TelegramApiResponse<T> {
    ok: bool,
    result: T,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<TelegramMessage>,
    #[serde(default)]
    pub edited_message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramMessage {
    pub chat: TelegramChat,
    #[serde(default)]
    pub from: Option<TelegramUser>,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramUser {
    pub id: i64,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub username: Option<String>,
}

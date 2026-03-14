/// JSONL audit log for session turns.
/// Path: ~/.kelix/sessions/<session-id>.jsonl
use crate::error::CoreError;
use crate::session::state::Turn;
use std::path::PathBuf;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn data_dir() -> Result<PathBuf, CoreError> {
    if let Some(override_dir) = std::env::var_os("KELIX_DATA_DIR") {
        return Ok(PathBuf::from(override_dir));
    }
    // @chunk session-log/default-data-dir
    // Match session index default: keep all persistent runtime data under
    // ~/.kelix unless KELIX_DATA_DIR explicitly overrides it.
    dirs::home_dir()
        .map(|home| home.join(".kelix"))
        .ok_or_else(|| CoreError::Config("cannot determine home directory".to_string()))
    // @end-chunk
}

pub fn log_path(session_id: &str) -> Result<PathBuf, CoreError> {
    let base = data_dir()?;
    Ok(log_path_in(&base, session_id))
}

fn log_path_in(base: &std::path::Path, session_id: &str) -> PathBuf {
    base.join("sessions").join(format!("{session_id}.jsonl"))
}

/// Append a single turn to the session JSONL file.
pub async fn append_turn(session_id: &str, turn: &Turn) -> Result<(), CoreError> {
    let path = log_path(session_id)?;
    append_turn_at(&path, turn).await
}

async fn append_turn_at(path: &std::path::Path, turn: &Turn) -> Result<(), CoreError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await?;
    let mut line = serde_json::to_vec(turn)?;
    line.push(b'\n');
    file.write_all(&line).await?;
    file.flush().await?;
    Ok(())
}

/// Load all turns from an existing JSONL file.
/// Lines that fail to parse are silently skipped (log corruption resilience).
pub async fn load_turns(session_id: &str) -> Result<Vec<Turn>, CoreError> {
    let path = log_path(session_id)?;
    load_turns_at(&path).await
}

async fn load_turns_at(path: &std::path::Path) -> Result<Vec<Turn>, CoreError> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let file = tokio::fs::File::open(&path).await?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();
    let mut turns = Vec::new();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(turn) = serde_json::from_str::<Turn>(&line) {
            turns.push(turn);
        }
    }
    Ok(turns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn test_append_and_load_roundtrip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let session_id = format!("test-{}", uuid::Uuid::new_v4());
        let path = log_path_in(temp_dir.path(), &session_id);
        let turn1 = Turn {
            timestamp: Utc::now(),
            prompt: Some("test prompt".to_string()),
            subagent_cmd: None,
            output: None,
        };
        let turn2 = Turn {
            timestamp: Utc::now(),
            prompt: None,
            subagent_cmd: Some("echo hello".to_string()),
            output: Some("hello".to_string()),
        };
        append_turn_at(&path, &turn1).await.unwrap();
        append_turn_at(&path, &turn2).await.unwrap();

        let turns = load_turns_at(&path).await.unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].prompt.as_deref(), Some("test prompt"));
        assert_eq!(turns[1].subagent_cmd.as_deref(), Some("echo hello"));

        // Cleanup
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn test_load_nonexistent_returns_empty() {
        let turns = load_turns("nonexistent-session-id-xyz").await.unwrap();
        assert!(turns.is_empty());
    }
}

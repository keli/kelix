/// Session index: ~/.kelix/sessions/index.json
/// Maintains a list of all known sessions with their state and metadata.
/// Writes are atomic (temp file + rename).
use crate::error::CoreError;
use crate::session::state::{SessionId, SessionState};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

fn data_dir() -> Result<PathBuf, CoreError> {
    if let Some(override_dir) = std::env::var_os("KELIX_DATA_DIR") {
        return Ok(PathBuf::from(override_dir));
    }
    // @chunk session-index/default-data-dir
    // Default persistent data root is ~/.kelix for a single cross-platform
    // location. Keep KELIX_DATA_DIR as an explicit override for operators/CI.
    dirs::home_dir()
        .map(|home| home.join(".kelix"))
        .ok_or_else(|| CoreError::Config("cannot determine home directory".to_string()))
    // @end-chunk
}

fn index_path() -> Result<PathBuf, CoreError> {
    let base = data_dir()?;
    Ok(index_path_in(&base))
}

fn index_path_in(base: &Path) -> PathBuf {
    base.join("sessions").join("index.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub id: SessionId,
    pub config_path: PathBuf,
    pub state: SessionState,
    pub last_active: DateTime<Utc>,
    pub enabled_subagents: Vec<String>,
    /// Consecutive unclean orchestrator exits since last successful completion.
    #[serde(default)]
    pub crash_counter: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionIndex {
    pub sessions: Vec<SessionEntry>,
}

impl SessionIndex {
    pub async fn load() -> Result<Self, CoreError> {
        let path = index_path()?;
        Self::load_from_path(&path).await
    }

    pub async fn save(&self) -> Result<(), CoreError> {
        let path = index_path()?;
        self.save_to_path(&path).await
    }

    async fn load_from_path(path: &Path) -> Result<Self, CoreError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = tokio::fs::read_to_string(path).await?;
        let index: Self = serde_json::from_str(&contents)?;
        Ok(index)
    }

    async fn save_to_path(&self, path: &Path) -> Result<(), CoreError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = path.with_extension("json.tmp");
        let contents = serde_json::to_vec_pretty(self)?;
        tokio::fs::write(&tmp, &contents).await?;
        tokio::fs::rename(&tmp, &path).await?;
        Ok(())
    }

    /// Insert or update a session entry.
    pub fn upsert(&mut self, entry: SessionEntry) {
        if let Some(existing) = self.sessions.iter_mut().find(|e| e.id == entry.id) {
            *existing = entry;
        } else {
            self.sessions.push(entry);
        }
    }

    pub fn get(&self, id: &str) -> Option<&SessionEntry> {
        self.sessions.iter().find(|e| e.id == id)
    }

    // @chunk session-index/purge
    // Remove sessions that have been inactive for longer than `max_age_days`.
    // Active sessions are never purged; only suspended ones are eligible.
    // Returns the number of entries removed.
    pub fn purge_old(&mut self, max_age_days: u64) -> usize {
        let cutoff = Utc::now() - chrono::Duration::days(max_age_days as i64);
        let before = self.sessions.len();
        self.sessions
            .retain(|e| e.state == SessionState::Active || e.last_active >= cutoff);
        before - self.sessions.len()
    }
    // @end-chunk
}

/// Load index, apply `f`, save atomically.
pub async fn update<F>(f: F) -> Result<(), CoreError>
where
    F: FnOnce(&mut SessionIndex),
{
    let mut index = SessionIndex::load().await?;
    f(&mut index);
    index.save().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_upsert_and_get() {
        let mut index = SessionIndex::default();
        let entry = SessionEntry {
            id: "sess-test-001".to_string(),
            config_path: PathBuf::from("/tmp/test.toml"),
            state: SessionState::Active,
            last_active: Utc::now(),
            enabled_subagents: vec!["orchestrator".to_string()],
            crash_counter: 0,
        };
        index.upsert(entry.clone());
        assert_eq!(index.sessions.len(), 1);

        // Update state
        let updated = SessionEntry {
            state: SessionState::Suspended,
            ..entry
        };
        index.upsert(updated);
        assert_eq!(index.sessions.len(), 1);
        assert_eq!(
            index.get("sess-test-001").unwrap().state,
            SessionState::Suspended
        );
    }

    #[tokio::test]
    async fn test_save_and_load_roundtrip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = index_path_in(temp_dir.path());

        // Use a unique ID to avoid colliding with real sessions
        let session_id = format!("sess-test-{}", uuid::Uuid::new_v4());
        let mut index = SessionIndex::load_from_path(&path).await.unwrap();
        let original_len = index.sessions.len();

        index.upsert(SessionEntry {
            id: session_id.clone(),
            config_path: PathBuf::from("/tmp/test.toml"),
            state: SessionState::Suspended,
            last_active: Utc::now(),
            enabled_subagents: vec![],
            crash_counter: 0,
        });
        index.save_to_path(&path).await.unwrap();

        let reloaded = SessionIndex::load_from_path(&path).await.unwrap();
        assert_eq!(reloaded.sessions.len(), original_len + 1);
        assert_eq!(
            reloaded.get(&session_id).unwrap().state,
            SessionState::Suspended
        );

        // Cleanup: remove the test entry
        let mut cleaned = reloaded;
        cleaned.sessions.retain(|e| e.id != session_id);
        cleaned.save_to_path(&path).await.unwrap();
    }
}

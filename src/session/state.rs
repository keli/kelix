/// Session state: in-memory representation of a session.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type SessionId = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Active,
    Suspended,
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Suspended => write!(f, "suspended"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_cmd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Debug)]
pub struct Session {
    pub id: SessionId,
    pub config_path: PathBuf,
    pub state: SessionState,
    pub turns: Vec<Turn>,
    pub total_turns: u64,
    /// Consecutive unclean orchestrator exits (crashes, not planned handovers).
    pub crash_counter: u32,
    /// Total acknowledged spawns in this session.
    pub spawn_count: u64,
    /// Cumulative tokens tracked from worker usage fields.
    pub cumulative_tokens: u64,
    pub started_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
    /// Subset of subagents enabled for this session (from --enabled-subagents).
    pub enabled_subagents: Vec<String>,
    /// Infra manifest from the orchestrator's bootstrap phase.
    pub infra_manifest: Option<serde_json::Value>,
}

impl Session {
    pub fn new(id: SessionId, config_path: PathBuf, enabled_subagents: Vec<String>) -> Self {
        let now = Utc::now();
        Self {
            id,
            config_path,
            state: SessionState::Active,
            turns: vec![],
            total_turns: 0,
            crash_counter: 0,
            spawn_count: 0,
            cumulative_tokens: 0,
            started_at: now,
            last_active: now,
            enabled_subagents,
            infra_manifest: None,
        }
    }

    pub fn append_turn(&mut self, turn: Turn) {
        self.total_turns += 1;
        self.last_active = Utc::now();
        self.turns.push(turn);
    }

    pub fn mark_suspended(&mut self) {
        self.state = SessionState::Suspended;
        self.last_active = Utc::now();
    }

    pub fn increment_crash(&mut self) {
        self.crash_counter += 1;
        self.last_active = Utc::now();
    }

    pub fn reset_crash_counter(&mut self) {
        self.crash_counter = 0;
    }
}

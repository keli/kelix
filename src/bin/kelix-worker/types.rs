use serde::{Deserialize, Serialize};
use serde_json::Value;

// @chunk kelix-worker/types
/// Spawn input written by the orchestrator (ORCHESTRATOR_PROTOCOL.md §4).
#[derive(Debug, Deserialize)]
pub struct SpawnInput {
    pub prompt: String,
    #[serde(default)]
    pub context: SpawnContext,
}

#[derive(Debug, Default, Deserialize)]
pub struct SpawnContext {
    #[serde(default)]
    pub task_id: String,
    #[serde(default)]
    pub branch: String,
}

/// Worker result written to stdout (ORCHESTRATOR_PROTOCOL.md §5).
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerResult {
    pub task_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub branch: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base_revision: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub blocked_reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handover: Option<Value>,
}

impl Default for WorkerResult {
    fn default() -> Self {
        Self {
            task_id: String::new(),
            status: String::new(),
            branch: String::new(),
            base_revision: String::new(),
            summary: String::new(),
            error: String::new(),
            failure_kind: String::new(),
            blocked_reason: String::new(),
            handover: None,
        }
    }
}
// @end-chunk

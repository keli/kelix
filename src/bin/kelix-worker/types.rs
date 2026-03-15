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

/// Worker exit status (ORCHESTRATOR_PROTOCOL.md §5).
/// Determines the exit code: success=0, failure=1, blocked=2, handover=3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    Success,
    Failure,
    Blocked,
    Handover,
}

impl WorkerStatus {
    /// Exhaustive list of valid serialized values. Used to generate runtime contract text.
    pub fn all_values() -> &'static [&'static str] {
        &["success", "failure", "blocked", "handover"]
    }

    /// Exit code per ORCHESTRATOR_PROTOCOL.md §5.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Success => 0,
            Self::Failure => 1,
            Self::Blocked => 2,
            Self::Handover => 3,
        }
    }

    /// Try to parse from a string, accepting common LLM synonyms.
    pub fn from_str_lenient(s: &str) -> Option<Self> {
        match s {
            "success" => Some(Self::Success),
            "failure" | "failed" | "rejected" => Some(Self::Failure),
            "blocked" | "needs_input" => Some(Self::Blocked),
            "handover" => Some(Self::Handover),
            _ => None,
        }
    }
}

/// Failure sub-category (ORCHESTRATOR_PROTOCOL.md §5).
/// Required when status is `failure`; omitted otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// Worker could not produce a correct implementation.
    Implementation,
    /// Implementation exists but BUILD_CMD failed.
    BuildFailed,
    /// Build passed but TEST_CMD failed.
    TestFailed,
    /// Output persisted locally but the extra publication step (e.g. git push) failed.
    /// Does NOT count toward MAX_FIX_ATTEMPTS; orchestrator re-invokes for publication retry.
    PushFailed,
}

impl FailureKind {
    pub fn all_values() -> &'static [&'static str] {
        &["implementation", "build_failed", "test_failed", "push_failed"]
    }
}

/// Blocked sub-category (ORCHESTRATOR_PROTOCOL.md §5).
/// Required when status is `blocked`; omitted otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockedReason {
    /// Worker needs human approval; orchestrator surfaces via `approve` or `blocked`.
    ApprovalRequired,
    /// An external service is unavailable; orchestrator retries (counts toward MAX_FIX_ATTEMPTS).
    ServiceUnavailable,
    /// Task prompt lacks necessary context; orchestrator revises and retries once.
    InsufficientContext,
}

impl BlockedReason {
    pub fn all_values() -> &'static [&'static str] {
        &["approval_required", "service_unavailable", "insufficient_context"]
    }
}

/// Worker result written to stdout (ORCHESTRATOR_PROTOCOL.md §5).
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerResult {
    pub task_id: String,
    pub status: WorkerStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub branch: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base_revision: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// Required when status is `failure`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<FailureKind>,
    /// Required when status is `blocked`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<BlockedReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handover: Option<Value>,
}

impl Default for WorkerResult {
    fn default() -> Self {
        Self {
            task_id: String::new(),
            status: WorkerStatus::Failure,
            branch: String::new(),
            base_revision: String::new(),
            summary: String::new(),
            error: String::new(),
            failure_kind: None,
            blocked_reason: None,
            handover: None,
        }
    }
}
// @end-chunk

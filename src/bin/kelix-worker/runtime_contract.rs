// @chunk worker/runtime-contract
// Canonical worker JSON output contract injected by kelix-worker.
// The schema is generated from the typed enums in types.rs so that the
// runtime contract stays in sync with the code regardless of prompt content.
use super::types::{BlockedReason, FailureKind, WorkerStatus};

/// Exit-code semantics per ORCHESTRATOR_PROTOCOL.md §5.
/// Derived from WorkerStatus::exit_code() — the single source of truth.
pub fn exit_code_table() -> String {
    let rows: Vec<String> = [
        (WorkerStatus::Success, "success — changes committed to task branch"),
        (WorkerStatus::Failure, "failure — no changes committed, error in output"),
        (WorkerStatus::Blocked, "blocked — worker cannot proceed, see blocked_reason"),
        (WorkerStatus::Handover, "handover — context limit reached, partial progress committed"),
    ]
    .iter()
    .map(|(s, desc)| format!("  {} = {}", s.exit_code(), desc))
    .collect();
    rows.join("\n")
}

/// Worker runtime contract string injected into every worker prompt.
/// All valid enum values are derived from the typed enums; do not edit manually.
pub fn worker_runtime_contract() -> String {
    let status_values = WorkerStatus::all_values().join("|");
    let failure_kind_values = FailureKind::all_values().join("|");
    let blocked_reason_values = BlockedReason::all_values().join("|");
    let exit_table = exit_code_table();

    format!(
        "# Worker Runtime Contract\n\
You must respond with exactly one compact JSON object on a single line.\n\
Do not output Markdown, prose, bullets, code fences, or any text before/after the JSON object.\n\
\n\
Output schema:\n\
{{\"task_id\":\"string\",\"status\":\"{status_values}\",\"branch\":\"string\",\"base_revision\":\"string\",\
\"summary\":\"string\",\"error\":\"string\",\
\"failure_kind\":\"{failure_kind_values}\",\
\"blocked_reason\":\"{blocked_reason_values}\",\
\"handover\":object}}\n\
\n\
Field rules:\n\
- `base_revision` must be the VCS revision (e.g. git commit hash) your changes are based on; omit only if no VCS is in use.\n\
- `failure_kind` is required when status is `failure`; omit otherwise.\n\
- `blocked_reason` is required when status is `blocked`; omit otherwise.\n\
- `handover` is required when status is `handover`; include `next_prompt`, `progress`, `remaining`.\n\
- If unsure, return status=\"failure\" with a clear `error` message.\n\
\n\
Exit codes (set automatically by the worker process — for your information only):\n\
{exit_table}"
    )
}

/// Pre-computed constant used at binary startup. Calling worker_runtime_contract()
/// once at startup is equivalent; this avoids repeated string allocation in the hot path.
pub static WORKER_RUNTIME_CONTRACT: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(worker_runtime_contract);
// @end-chunk

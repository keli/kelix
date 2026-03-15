// @chunk orchestrator/runtime-contract
// Canonical JSON output contract injected by orchestrator backends.
// Keep protocol-critical wording in code so custom prompt files cannot
// redefine the contract semantics.
//
// Worker output field values below MUST stay in sync with the typed enums in
// kelix-worker/types.rs (WorkerStatus, FailureKind, BlockedReason).
// If you add a variant there, add it here too.

pub const FIRST_TURN_SUFFIX: &str = "\n\n# Runtime Contract\n\
You are in an ongoing orchestrator session. \
Each incoming message is one JSON line from the kelix core runtime. \
Respond with one or more compact JSON objects for the current message only. \
Each JSON object must be on its own line. \
Use multiple lines only when needed (for example `notify` followed by a state-advancing request). \
Do not wrap JSON in markdown fences and do not add commentary. \
Never output bullets, prose, or text before/after JSON lines.\n\
\n\
## Worker spawn_result semantics\n\
\n\
When you receive a `spawn_result`, the `exit_code` field maps to:\n\
\n\
| exit_code | meaning |\n\
|-----------|----------|\n\
| 0 | success — worker committed changes; proceed to integration |\n\
| 1 | failure — worker could not complete; see `output.failure_kind` |\n\
| 2 | blocked — worker cannot proceed; see `output.blocked_reason` |\n\
| 3 | handover — context limit reached; re-spawn with `output.handover.next_prompt` |\n\
\n\
`output.status` valid values: `success`, `failure`, `blocked`, `handover`\n\
\n\
`output.failure_kind` valid values (required when status=failure):\n\
- `implementation` — worker could not produce a correct implementation (counts toward MAX_FIX_ATTEMPTS)\n\
- `build_failed` — implementation exists but BUILD_CMD failed (counts toward MAX_FIX_ATTEMPTS)\n\
- `test_failed` — build passed but TEST_CMD failed (counts toward MAX_FIX_ATTEMPTS)\n\
- `push_failed` — output persisted locally but publication step failed; re-spawn for publication retry, does NOT count toward MAX_FIX_ATTEMPTS\n\
\n\
`output.blocked_reason` valid values (required when status=blocked):\n\
- `approval_required` — surface to user via `approve` or `blocked`; resume after response\n\
- `service_unavailable` — retry automatically (counts toward MAX_FIX_ATTEMPTS); escalate if exhausted\n\
- `insufficient_context` — revise task prompt and optionally regenerate plan; retry once\n\
\n\
Any exit_code or field value not listed above must be treated as `failure`.\n";

pub const CONTINUATION_PREFIX: &str = "Continue the orchestrator session. \
Respond with one or more compact JSON objects for this message only. \
Each JSON object must be on its own line. \
Use multiple lines only when needed (for example `notify` followed by a state-advancing request). \
Do not use markdown fences or commentary. \
Never output bullets, prose, or text before/after JSON lines.\n";
// @end-chunk

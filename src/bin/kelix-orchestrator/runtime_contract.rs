// @chunk orchestrator/runtime-contract
// Canonical JSON output contract injected by orchestrator backends.
// Keep protocol-critical wording in code so custom prompt files cannot
// redefine the contract semantics.
pub const FIRST_TURN_SUFFIX: &str = "\n\n# Runtime Contract\n\
You are in an ongoing orchestrator session. \
Each incoming message is one JSON line from the kelix core runtime. \
Respond with one or more compact JSON objects for the current message only. \
Each JSON object must be on its own line. \
Use multiple lines only when needed (for example `notify` followed by a state-advancing request). \
Do not wrap JSON in markdown fences and do not add commentary. \
Never output bullets, prose, or text before/after JSON lines.\n";

pub const CONTINUATION_PREFIX: &str = "Continue the orchestrator session. \
Respond with one or more compact JSON objects for this message only. \
Each JSON object must be on its own line. \
Use multiple lines only when needed (for example `notify` followed by a state-advancing request). \
Do not use markdown fences or commentary. \
Never output bullets, prose, or text before/after JSON lines.\n";
// @end-chunk

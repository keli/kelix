// @chunk worker/runtime-contract
// Canonical worker JSON output contract injected by kelix-worker.
pub const WORKER_RUNTIME_CONTRACT: &str = "# Worker Runtime Contract\n\
You must respond with exactly one compact JSON object on a single line.\n\
Do not output Markdown, prose, bullets, code fences, or any text before/after the JSON object.\n\
Output schema:\n\
{\"task_id\":\"string\",\"status\":\"success|failure|blocked|handover\",\"branch\":\"string\",\"base_revision\":\"string\",\"summary\":\"string\",\"error\":\"string\",\"failure_kind\":\"implementation|push_failed\",\"blocked_reason\":\"approval_required|service_unavailable|insufficient_context\",\"handover\":object}\n\
If unsure, return a JSON object with status=\"failure\" and a clear error message.";
// @end-chunk

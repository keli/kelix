use serde_json::Value;
use std::process::{Command, Stdio};

use super::helpers::parse_worker_result_text;
use super::types::WorkerResult;

// @chunk kelix-worker/claude-backend
// Invokes `claude -p` with stream-json output and extracts the result event.
// Claude emits a final `{"type":"result","subtype":"success"|"error_*",...}` line.
// On success the LLM output must be a JSON worker result.
pub fn run_claude(prompt: &str, task_id: &str, branch: &str) -> Result<WorkerResult, String> {
    let output = Command::new("claude")
        .args([
            "-p",
            prompt,
            "--output-format",
            "stream-json",
            "--verbose",
            "--dangerously-skip-permissions",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("failed to spawn claude: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Find the last `type: result` event.
    let result_event = stdout
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("result"))
        .ok_or_else(|| {
            format!(
                "claude produced no result event (exit {})",
                output.status.code().unwrap_or(-1)
            )
        })?;

    let subtype = result_event
        .get("subtype")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let result_text = result_event
        .get("result")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if subtype != "success" {
        return Err(format!("claude failed ({subtype}): {result_text}"));
    }

    parse_worker_result_text(result_text, task_id, branch)
}
// @end-chunk

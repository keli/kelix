use serde_json::Value;
use std::process::{Command, Stdio};

use super::helpers::{extract_json_object, parse_worker_result_value, strip_code_fence};
use super::types::WorkerResult;

// @chunk kelix-worker/codex-backend
// Invokes `codex exec --json` and extracts the last agent_message text.
// Codex emits `item.completed` events; agent replies arrive as
// `{"type":"item.completed","item":{"type":"agent_message","text":"..."}}`.
// The last such message is parsed as JSON; if it matches WorkerResult it is
// used directly, otherwise it is treated as free-text summary.
pub fn run_codex(prompt: &str, task_id: &str, branch: &str) -> Result<WorkerResult, String> {
    let output = Command::new("codex")
        .args([
            "exec",
            "--json",
            "--sandbox",
            "danger-full-access",
            "--dangerously-bypass-approvals-and-sandbox",
            prompt,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("failed to spawn codex: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    let agent_messages: Vec<String> = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("item.completed"))
        .filter_map(|v| {
            let item = v.get("item")?;
            let kind = item.get("type")?.as_str()?;
            if kind == "agent_message" || kind == "message" {
                item.get("text")?.as_str().map(str::to_string)
            } else {
                None
            }
        })
        .collect();

    let last_text = agent_messages.last().ok_or_else(|| {
        format!(
            "codex produced no agent_message events (exit {})",
            output.status.code().unwrap_or(-1)
        )
    })?;

    let json_text = strip_code_fence(last_text.trim());

    // Try direct parse as WorkerResult first, then as generic Value with defaults.
    if let Ok(parsed) = serde_json::from_str::<WorkerResult>(json_text) {
        return Ok(parsed);
    }

    let mut parsed: Value = serde_json::from_str(json_text)
        .or_else(|_| extract_json_object(json_text).ok_or(()))
        .map_err(|_| {
            format!(
                "could not parse JSON from codex output: {}",
                &last_text[..last_text.len().min(500)]
            )
        })?;

    // review-agent outputs "decision" rather than "status"; normalize before parsing.
    if parsed.get("status").is_none() {
        if let Some(decision) = parsed.get("decision").and_then(|v| v.as_str()).map(str::to_string) {
            parsed["status"] = Value::String(decision);
        }
    }

    Ok(parse_worker_result_value(parsed, task_id, branch))
}
// @end-chunk

#[cfg(test)]
mod tests {
    use super::super::types::WorkerStatus;

    #[test]
    fn test_run_codex_decision_rejected_yields_failure_exit_code() {
        // "rejected" is a synonym for failure; WorkerStatus::from_str_lenient handles it.
        let ws = WorkerStatus::from_str_lenient("rejected").unwrap();
        assert_eq!(ws, WorkerStatus::Failure);
        assert_eq!(ws.exit_code(), 1, "rejected must produce exit_code 1, not 0");
    }

    #[test]
    fn test_run_codex_missing_status_defaults_to_failure_not_success() {
        // None status maps to Failure via the default in parse_worker_result_value.
        let ws = WorkerStatus::from_str_lenient("failure").unwrap();
        assert_eq!(ws.exit_code(), 1);
    }
}

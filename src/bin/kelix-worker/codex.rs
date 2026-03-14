use serde_json::Value;
use std::process::{Command, Stdio};

use super::helpers::{extract_json_object, strip_code_fence};
use super::types::WorkerResult;

// @chunk kelix-worker/codex-backend
// Invokes `codex exec --json` and extracts the last agent_message text.
// Codex emits `item.completed` events; agent replies arrive as
// `{"type":"item.completed","item":{"type":"agent_message","text":"..."}}`.
// The last such message is parsed as JSON; if it matches WorkerResult it is
// used directly, otherwise it is treated as free-text summary.
pub fn run_codex(prompt: &str, task_id: &str, _branch: &str) -> Result<WorkerResult, String> {
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

    let parsed: Value = serde_json::from_str(json_text)
        .or_else(|_| extract_json_object(json_text).ok_or(()))
        .map_err(|_| {
            format!(
                "could not parse JSON from codex output: {}",
                &last_text[..last_text.len().min(500)]
            )
        })?;

    // Map generic Value to WorkerResult using the protocol field names.
    Ok(WorkerResult {
        task_id: parsed
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or(task_id)
            .to_string(),
        status: parsed
            .get("status")
            .and_then(|v| v.as_str())
            // review-agent outputs "decision" rather than "status"
            .or_else(|| parsed.get("decision").and_then(|v| v.as_str()))
            // Default to "failure" when no status/decision field is present:
            // unknown outcome must not be silently reported as success.
            .unwrap_or("failure")
            .to_string(),
        summary: parsed
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        failure_kind: parsed
            .get("failure_kind")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        blocked_reason: parsed
            .get("blocked_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        handover: parsed.get("handover").cloned(),
        ..Default::default()
    })
}
// @end-chunk

#[cfg(test)]
mod tests {
    #[test]
    fn test_run_codex_decision_rejected_yields_failure_exit_code() {
        // Simulate the generic-Value fallback path in run_codex.
        // The review-agent uses "decision" rather than "status".
        let json = r#"{"task_id":"t1","decision":"rejected","summary":"needs work"}"#;
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();

        let status = parsed
            .get("status")
            .and_then(|v| v.as_str())
            .or_else(|| parsed.get("decision").and_then(|v| v.as_str()))
            .unwrap_or("failure")
            .to_string();

        assert_eq!(status, "rejected");
        let exit_code = match status.as_str() {
            "success" => 0,
            "blocked" => 2,
            "handover" => 3,
            _ => 1,
        };
        assert_eq!(exit_code, 1, "rejected must produce exit_code 1, not 0");
    }

    #[test]
    fn test_run_codex_missing_status_defaults_to_failure_not_success() {
        let json = r#"{"task_id":"t1","summary":"something happened"}"#;
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();

        let status = parsed
            .get("status")
            .and_then(|v| v.as_str())
            .or_else(|| parsed.get("decision").and_then(|v| v.as_str()))
            .unwrap_or("failure")
            .to_string();

        assert_eq!(status, "failure");
        let exit_code = match status.as_str() {
            "success" => 0,
            "blocked" => 2,
            "handover" => 3,
            _ => 1,
        };
        assert_eq!(exit_code, 1);
    }
}

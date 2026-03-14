use serde_json::Value;
use std::fmt;
use std::io;
use uuid::Uuid;

pub fn new_request_id() -> String {
    format!("req-{}", Uuid::new_v4())
}

// @chunk orchestrator/normalize-error-types
// Structured normalization errors used by backend wrappers and core-facing diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalizeErrorKind {
    JsonContract,
    InvalidPayload,
    Internal,
}

#[derive(Debug, Clone)]
pub struct NormalizeError {
    kind: NormalizeErrorKind,
    message: String,
}

impl NormalizeError {
    pub fn json_contract(message: impl Into<String>) -> Self {
        Self {
            kind: NormalizeErrorKind::JsonContract,
            message: message.into(),
        }
    }

    pub fn invalid_payload(message: impl Into<String>) -> Self {
        Self {
            kind: NormalizeErrorKind::InvalidPayload,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: NormalizeErrorKind::Internal,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> NormalizeErrorKind {
        self.kind
    }
}

impl fmt::Display for NormalizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}
// @end-chunk

pub fn normalize_orchestrator_json_from_output(stdout: &str) -> Result<String, NormalizeError> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err(NormalizeError::invalid_payload(
            "opencode produced no output",
        ));
    }

    if let Ok(normalized) = normalize_orchestrator_json_line(trimmed) {
        return Ok(normalized);
    }

    // Fallback for chatty outputs where only the final non-empty line is JSON.
    let last_line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(str::trim)
        .unwrap_or("");
    if !last_line.is_empty() {
        return normalize_orchestrator_json_line(last_line);
    }

    Err(NormalizeError::invalid_payload(
        "opencode produced no output",
    ))
}

pub fn normalize_orchestrator_json_line(text: &str) -> Result<String, NormalizeError> {
    let stripped = strip_code_fence(text.trim());
    let parsed: Value = serde_json::from_str(stripped)
        .or_else(|_| {
            extract_json_object(stripped).ok_or(serde_json::Error::io(io::Error::new(
                io::ErrorKind::InvalidData,
                "no json object found",
            )))
        })
        .map_err(|_| {
            NormalizeError::json_contract(format!(
                "could not parse JSON from agent output: {}",
                &text[..text.len().min(500)]
            ))
        })?;

    let normalized = normalize_orchestrator_value(parsed)?;

    serde_json::to_string(&normalized)
        .map_err(|e| NormalizeError::internal(format!("failed to serialize JSON response: {e}")))
}

// @chunk orchestrator/normalize-json-lines
// Normalize one or more orchestrator JSON messages. Multi-message payloads must
// be newline-delimited JSON objects so core can consume them as protocol lines.
pub fn normalize_orchestrator_json_lines(text: &str) -> Result<Vec<String>, NormalizeError> {
    let stripped = strip_code_fence(text.trim());
    if stripped.is_empty() {
        return Err(NormalizeError::json_contract(
            "could not parse JSON from agent output: empty output",
        ));
    }

    let non_empty_lines: Vec<&str> = stripped
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    if non_empty_lines.len() > 1 {
        return non_empty_lines
            .into_iter()
            .map(normalize_orchestrator_json_line)
            .collect();
    }

    Ok(vec![normalize_orchestrator_json_line(stripped)?])
}
// @end-chunk

// @chunk orchestrator/non-json-fallback
// Convert non-JSON backend output into a protocol-valid blocked message so
// core can keep the session alive instead of crashing the orchestrator process.
pub fn synthesize_blocked_from_non_json(raw_output: &str) -> String {
    let raw = raw_output.trim();
    let snippet = if raw.is_empty() {
        "empty output".to_string()
    } else {
        raw.chars().take(500).collect()
    };
    serde_json::json!({
        "id": new_request_id(),
        "type": "blocked",
        "message": format!(
            "orchestrator backend returned non-JSON output; manual intervention required. raw output: {}",
            snippet
        ),
    })
    .to_string()
}
// @end-chunk

pub fn normalize_orchestrator_value(parsed: Value) -> Result<Value, NormalizeError> {
    if parsed.get("type").and_then(|t| t.as_str()).is_some() {
        return Ok(parsed);
    }

    let Some(status) = parsed.get("status").and_then(|s| s.as_str()) else {
        return Ok(parsed);
    };

    match status {
        "blocked" | "needs_input" => {
            let message = parsed
                .get("message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    NormalizeError::invalid_payload(format!(
                        "{status} response is missing a string message"
                    ))
                })?;
            Ok(serde_json::json!({
                "id": new_request_id(),
                "type": "blocked",
                "message": message,
            }))
        }
        "complete" | "completed" | "done" | "ok" | "success" => {
            let summary = parsed
                .get("summary")
                .and_then(|v| v.as_str())
                .or_else(|| parsed.get("message").and_then(|v| v.as_str()))
                .unwrap_or("Task completed.");
            Ok(serde_json::json!({
                "id": new_request_id(),
                "type": "complete",
                "summary": summary,
            }))
        }
        // @chunk orchestrator/rejected-mapping
        // "rejected" and related failure statuses must NOT be mapped to "complete".
        // A review-agent rejection means the work is not done; map to "blocked" so
        // the orchestrator escalates to a human rather than silently ending the session.
        // Without this arm, the value would fall through to the catch-all and be
        // returned without a "type" field, causing kelix core to drop the message.
        "rejected" | "failure" | "failed" => {
            let message = parsed
                .get("message")
                .and_then(|v| v.as_str())
                .or_else(|| parsed.get("summary").and_then(|v| v.as_str()))
                .or_else(|| parsed.get("reason").and_then(|v| v.as_str()))
                .unwrap_or(
                    "Task rejected or failed; review the outcome and decide how to proceed.",
                );
            Ok(serde_json::json!({
                "id": new_request_id(),
                "type": "blocked",
                "message": message,
            }))
        }
        // @end-chunk
        "notify" | "info" | "warning" | "error" => {
            let text = parsed
                .get("text")
                .and_then(|v| v.as_str())
                .or_else(|| parsed.get("message").and_then(|v| v.as_str()))
                .ok_or_else(|| {
                    NormalizeError::invalid_payload(format!("{status} response is missing text"))
                })?;
            let mut normalized = serde_json::json!({
                "id": new_request_id(),
                "type": "notify",
                "message": text,
            });
            if matches!(status, "warning" | "error") {
                normalized["level"] = Value::String(status.to_string());
            }
            Ok(normalized)
        }
        _ => Ok(parsed),
    }
}

pub fn strip_code_fence(s: &str) -> &str {
    if let Some(inner) = s.strip_prefix("```") {
        let after_lang = inner.find('\n').map(|i| &inner[i + 1..]).unwrap_or(inner);
        if let Some(body) = after_lang.rfind("```").map(|i| after_lang[..i].trim()) {
            return body;
        }
        return after_lang.trim();
    }
    s
}

pub fn extract_json_object(s: &str) -> Option<Value> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    serde_json::from_str(&s[start..=end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_orchestrator_value_maps_blocked_status() {
        let normalized = normalize_orchestrator_value(serde_json::json!({
            "status": "blocked",
            "message": "Need input",
        }))
        .unwrap();

        assert_eq!(
            normalized.get("type").and_then(|v| v.as_str()),
            Some("blocked")
        );
        assert_eq!(
            normalized.get("message").and_then(|v| v.as_str()),
            Some("Need input")
        );
        assert!(normalized.get("id").and_then(|v| v.as_str()).is_some());
    }

    #[test]
    fn test_normalize_orchestrator_value_maps_needs_input_status() {
        let normalized = normalize_orchestrator_value(serde_json::json!({
            "status": "needs_input",
            "message": "Which worker roles?",
        }))
        .unwrap();

        assert_eq!(
            normalized.get("type").and_then(|v| v.as_str()),
            Some("blocked")
        );
        assert_eq!(
            normalized.get("message").and_then(|v| v.as_str()),
            Some("Which worker roles?")
        );
        assert!(normalized.get("id").and_then(|v| v.as_str()).is_some());
    }

    #[test]
    fn test_normalize_orchestrator_value_maps_complete_status() {
        let normalized = normalize_orchestrator_value(serde_json::json!({
            "status": "success",
            "message": "Wrote /workspace/kelix.toml",
        }))
        .unwrap();

        assert_eq!(
            normalized.get("type").and_then(|v| v.as_str()),
            Some("complete")
        );
        assert_eq!(
            normalized.get("summary").and_then(|v| v.as_str()),
            Some("Wrote /workspace/kelix.toml")
        );
        assert!(normalized.get("id").and_then(|v| v.as_str()).is_some());
    }

    #[test]
    fn test_normalize_orchestrator_value_maps_notify_status() {
        let normalized = normalize_orchestrator_value(serde_json::json!({
            "status": "notify",
            "message": "Planning tasks",
        }))
        .unwrap();

        assert_eq!(
            normalized.get("type").and_then(|v| v.as_str()),
            Some("notify")
        );
        assert_eq!(
            normalized.get("message").and_then(|v| v.as_str()),
            Some("Planning tasks")
        );
        assert!(normalized.get("id").and_then(|v| v.as_str()).is_some());
    }

    #[test]
    fn test_normalize_orchestrator_value_maps_rejected_to_blocked() {
        // "rejected" must NOT become "complete" — it must escalate to "blocked".
        let normalized = normalize_orchestrator_value(serde_json::json!({
            "status": "rejected",
            "message": "Review failed: code quality issues detected.",
        }))
        .unwrap();

        assert_eq!(
            normalized.get("type").and_then(|v| v.as_str()),
            Some("blocked"),
            "rejected must map to blocked, not complete"
        );
        assert_eq!(
            normalized.get("message").and_then(|v| v.as_str()),
            Some("Review failed: code quality issues detected.")
        );
        assert!(normalized.get("id").and_then(|v| v.as_str()).is_some());
    }

    #[test]
    fn test_normalize_orchestrator_value_maps_rejected_uses_summary_fallback() {
        let normalized = normalize_orchestrator_value(serde_json::json!({
            "status": "rejected",
            "summary": "PR needs revision before merge.",
        }))
        .unwrap();

        assert_eq!(
            normalized.get("type").and_then(|v| v.as_str()),
            Some("blocked")
        );
        assert_eq!(
            normalized.get("message").and_then(|v| v.as_str()),
            Some("PR needs revision before merge.")
        );
    }

    #[test]
    fn test_normalize_orchestrator_value_maps_failure_to_blocked() {
        let normalized = normalize_orchestrator_value(serde_json::json!({
            "status": "failure",
            "summary": "Build failed with 3 errors.",
        }))
        .unwrap();

        assert_eq!(
            normalized.get("type").and_then(|v| v.as_str()),
            Some("blocked"),
            "failure must map to blocked, not complete"
        );
    }

    #[test]
    fn test_normalize_orchestrator_value_maps_failed_to_blocked() {
        let normalized = normalize_orchestrator_value(serde_json::json!({
            "status": "failed",
            "reason": "Deployment timed out.",
        }))
        .unwrap();

        assert_eq!(
            normalized.get("type").and_then(|v| v.as_str()),
            Some("blocked"),
            "failed must map to blocked, not complete"
        );
        assert_eq!(
            normalized.get("message").and_then(|v| v.as_str()),
            Some("Deployment timed out.")
        );
    }

    #[test]
    fn test_normalize_orchestrator_value_rejected_not_treated_as_success() {
        // Regression guard: "rejected" must never produce type "complete".
        let normalized = normalize_orchestrator_value(serde_json::json!({
            "status": "rejected",
            "summary": "Linting errors found.",
        }))
        .unwrap();

        assert_ne!(
            normalized.get("type").and_then(|v| v.as_str()),
            Some("complete"),
            "rejected must not be treated as success/complete"
        );
    }

    #[test]
    fn test_normalize_orchestrator_json_from_output_parses_multiline_fenced_json() {
        let output = "```json\n{\n  \"status\": \"success\",\n  \"summary\": \"done\"\n}\n```";
        let normalized = normalize_orchestrator_json_from_output(output).unwrap();
        let v: serde_json::Value = serde_json::from_str(&normalized).unwrap();
        assert_eq!(v.get("type").and_then(|x| x.as_str()), Some("complete"));
    }

    #[test]
    fn test_normalize_orchestrator_json_from_output_parses_last_line_fallback() {
        let output = "thinking...\n{\"status\":\"notify\",\"message\":\"planning\"}";
        let normalized = normalize_orchestrator_json_from_output(output).unwrap();
        let v: serde_json::Value = serde_json::from_str(&normalized).unwrap();
        assert_eq!(v.get("type").and_then(|x| x.as_str()), Some("notify"));
        assert_eq!(v.get("message").and_then(|x| x.as_str()), Some("planning"));
    }

    #[test]
    fn test_normalize_orchestrator_json_lines_supports_multi_message_ndjson() {
        let output = "{\"type\":\"notify\",\"id\":\"req-1\",\"message\":\"planning\"}\n{\"type\":\"spawn\",\"id\":\"req-2\",\"subagent\":\"coding-agent\",\"input\":{\"prompt\":\"do x\"}}";
        let lines = normalize_orchestrator_json_lines(output).unwrap();
        assert_eq!(lines.len(), 2);
        let first: Value = serde_json::from_str(&lines[0]).unwrap();
        let second: Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(first.get("type").and_then(|x| x.as_str()), Some("notify"));
        assert_eq!(second.get("type").and_then(|x| x.as_str()), Some("spawn"));
    }

    #[test]
    fn test_normalize_orchestrator_json_lines_supports_single_message() {
        let output = "{\"status\":\"notify\",\"message\":\"ok\"}";
        let lines = normalize_orchestrator_json_lines(output).unwrap();
        assert_eq!(lines.len(), 1);
        let v: Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(v.get("type").and_then(|x| x.as_str()), Some("notify"));
    }

    #[test]
    fn test_synthesize_blocked_from_non_json_is_protocol_valid() {
        let raw = "Done. updated file.";
        let out = synthesize_blocked_from_non_json(raw);
        let parsed: Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(parsed.get("type").and_then(|v| v.as_str()), Some("blocked"));
        let message = parsed.get("message").and_then(|v| v.as_str()).unwrap_or("");
        assert!(message.contains("non-JSON output"));
        assert!(message.contains("Done. updated file."));
    }
}

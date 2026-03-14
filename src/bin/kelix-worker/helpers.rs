use serde_json::Value;
use std::process::Command;

use super::types::WorkerResult;

// @chunk kelix-worker/helpers
pub fn emit_failure(task_id: &str, branch: &str, error: &str, kind: &str) {
    let result = WorkerResult {
        task_id: task_id.to_string(),
        status: "failure".into(),
        branch: branch.to_string(),
        summary: truncate(error, 200),
        error: error.to_string(),
        failure_kind: kind.to_string(),
        ..Default::default()
    };
    println!("{}", serde_json::to_string(&result).unwrap());
}

pub fn git_head_revision() -> String {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub fn truncate(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let taken: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{taken}…")
    } else {
        taken
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

pub fn parse_worker_result_value(v: Value, task_id: &str, branch: &str) -> WorkerResult {
    let status = v
        .get("status")
        .and_then(|s| s.as_str())
        // Default to "failure" when no status field is present:
        // unknown outcome must not be silently reported as success.
        .unwrap_or("failure");
    WorkerResult {
        task_id: v
            .get("task_id")
            .and_then(|s| s.as_str())
            .unwrap_or(task_id)
            .to_string(),
        status: status.to_string(),
        branch: v
            .get("branch")
            .and_then(|s| s.as_str())
            .unwrap_or(branch)
            .to_string(),
        base_revision: v
            .get("base_revision")
            .and_then(|s| s.as_str())
            .map(str::to_string)
            .unwrap_or_else(git_head_revision),
        summary: v
            .get("summary")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        error: v
            .get("error")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        failure_kind: v
            .get("failure_kind")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        blocked_reason: v
            .get("blocked_reason")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        handover: v.get("handover").cloned(),
    }
}

pub fn parse_worker_result_text(
    text: &str,
    task_id: &str,
    branch: &str,
) -> Result<WorkerResult, String> {
    let stripped = strip_code_fence(text.trim());
    if stripped.is_empty() {
        return Err("worker output is empty; expected JSON worker result".to_string());
    }

    if let Ok(parsed) = serde_json::from_str::<WorkerResult>(stripped) {
        return Ok(parsed);
    }

    if let Some(v) = extract_json_object(stripped) {
        return Ok(parse_worker_result_value(v, task_id, branch));
    }

    Err(format!(
        "worker output is not valid JSON worker result: {}",
        truncate(text.trim(), 300)
    ))
}

pub fn parse_opencode_worker_output(
    stdout: &str,
    task_id: &str,
    branch: &str,
) -> Option<WorkerResult> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }

    let last_line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(str::trim)
        .unwrap_or("");

    for candidate in [trimmed, last_line] {
        if candidate.is_empty() {
            continue;
        }
        let stripped = strip_code_fence(candidate);
        if let Ok(parsed) = serde_json::from_str::<WorkerResult>(stripped) {
            return Some(parsed);
        }
        if let Some(v) = extract_json_object(stripped) {
            return Some(parse_worker_result_value(v, task_id, branch));
        }
    }

    None
}
// @end-chunk

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_code_fence_removes_fence() {
        let fenced = "```json\n{\"status\":\"success\"}\n```";
        assert_eq!(strip_code_fence(fenced), "{\"status\":\"success\"}");
    }

    #[test]
    fn test_strip_code_fence_passthrough_plain() {
        let plain = "{\"status\":\"success\"}";
        assert_eq!(strip_code_fence(plain), plain);
    }

    #[test]
    fn test_extract_json_object_from_plain() {
        let v = extract_json_object(r#"{"task_id":"t1","status":"success","summary":"done"}"#);
        assert!(v.is_some());
        let v = v.unwrap();
        assert_eq!(v.get("status").and_then(|s| s.as_str()), Some("success"));
    }

    #[test]
    fn test_extract_json_object_from_embedded() {
        let v = extract_json_object(r#"some text {"task_id":"t1","status":"success"} here"#);
        assert!(v.is_some());
    }

    #[test]
    fn test_extract_json_object_no_object() {
        assert!(extract_json_object("no json here").is_none());
    }

    #[test]
    fn test_truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long_string() {
        let result = truncate("hello world", 5);
        assert!(result.starts_with("hello"));
        assert!(result.contains('…'));
    }

    #[test]
    fn test_parse_opencode_worker_output_parses_multiline_fenced_json() {
        let stdout = "```json\n{\n  \"task_id\": \"t1\",\n  \"status\": \"blocked\",\n  \"blocked_reason\": \"approval_required\"\n}\n```";
        let parsed = parse_opencode_worker_output(stdout, "fallback-task", "main").unwrap();
        assert_eq!(parsed.task_id, "t1");
        assert_eq!(parsed.status, "blocked");
        assert_eq!(parsed.blocked_reason, "approval_required");
    }

    #[test]
    fn test_parse_opencode_worker_output_parses_json_with_log_prefix() {
        let stdout = "some log line\n{\"task_id\":\"t1\",\"status\":\"failure\",\"summary\":\"compile failed\"}";
        let parsed = parse_opencode_worker_output(stdout, "fallback-task", "main").unwrap();
        assert_eq!(parsed.task_id, "t1");
        assert_eq!(parsed.status, "failure");
        assert_eq!(parsed.summary, "compile failed");
    }

    #[test]
    fn test_parse_worker_result_text_rejects_plain_text() {
        let err = parse_worker_result_text("done, all tests pass", "t1", "main").unwrap_err();
        assert!(err.contains("not valid JSON worker result"));
    }
}

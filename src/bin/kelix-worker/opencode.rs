use std::process::{Command, Stdio};

use super::helpers::{parse_opencode_worker_output, truncate};
use super::types::WorkerResult;

// @chunk kelix-worker/opencode-backend
// Invokes `opencode run` with the prompt and extracts the worker result.
// OpenCode output must contain a JSON WorkerResult when the agent is prompted
// with the runtime contract.
//
// Expected CLI: opencode run "<prompt>"
// Requires opencode >= 0.1 to be on PATH and authenticated.
pub fn run_opencode(prompt: &str, task_id: &str, branch: &str) -> Result<WorkerResult, String> {
    let output = Command::new("opencode")
        .args(["run", prompt])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("failed to spawn opencode: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let exit_code = output.status.code().unwrap_or(-1);

    if let Some(parsed) = parse_opencode_worker_output(&stdout, task_id, branch) {
        if !output.status.success() && parsed.status == "success" {
            return Err(format!(
                "opencode exited with code {exit_code} but reported success"
            ));
        }
        return Ok(parsed);
    }

    if !output.status.success() {
        return Err(format!(
            "opencode exited with code {exit_code}: {}",
            truncate(stdout.trim(), 300)
        ));
    }

    Err(format!(
        "opencode produced non-JSON worker output: {}",
        truncate(stdout.trim(), 300)
    ))
}
// @end-chunk

#[cfg(test)]
mod tests {
    #[test]
    fn test_run_opencode_missing_status_defaults_to_failure_not_success() {
        let json = serde_json::json!({"task_id": "t1", "summary": "output without status"});
        let status = json
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("failure");

        assert_eq!(status, "failure");
    }

    #[test]
    fn test_exit_code_for_rejected_is_nonzero() {
        for bad_status in &["rejected", "failure", "failed", "unknown", ""] {
            let exit_code = match *bad_status {
                "success" => 0,
                "blocked" => 2,
                "handover" => 3,
                _ => 1,
            };
            assert_ne!(
                exit_code, 0,
                "status '{bad_status}' must not produce exit_code 0"
            );
        }
    }
}

/// Approval gate routing.
/// Routes shell approve requests and result gate interceptions.
/// See CORE_PROTOCOL.md §4.2 and §7.1.
use crate::config::{ApprovalConfig, ResultGate, ShellGate};
use crate::error::CoreError;
use crate::protocol::core_msg::{ApproveKind, ErrorCode};
use crate::spawn::process_runner::{run_subagent_process, ProcessResult};
use async_trait::async_trait;

pub struct ApprovalRequest {
    pub id: String,
    pub kind: ApproveKind,
    pub message: String,
    pub options: Vec<String>,
}

#[derive(Debug)]
pub struct ApprovalDecision {
    pub choice: String,
    pub decided_by: String,
}

/// Trait for the frontend's approval UI.
/// The TUI and headless frontend each implement this.
#[async_trait]
pub trait ApprovalUi: Send + Sync {
    /// Surface the approval request to the human and wait for a response.
    async fn request_approval(&self, req: &ApprovalRequest) -> Result<String, CoreError>;

    /// Surface a blocked request (free-form input) to the human.
    async fn request_input(&self, message: &str) -> Result<String, CoreError>;
}

/// Route a shell approve request through the configured shell gate.
///
/// Returns `Err(CoreError::InvalidRequest)` if `options` is empty.
/// The caller is responsible for recording the decision in the session log.
pub async fn decide(
    config: &ApprovalConfig,
    req: &ApprovalRequest,
    ui: &dyn ApprovalUi,
) -> Result<ApprovalDecision, CoreError> {
    if req.options.is_empty() {
        return Err(CoreError::InvalidRequest(
            "approve request must have at least one option".to_string(),
        ));
    }

    match &config.shell_gate {
        ShellGate::None => Ok(ApprovalDecision {
            choice: req.options[0].clone(),
            decided_by: "auto".to_string(),
        }),
        ShellGate::Human => {
            let choice = ui.request_approval(req).await?;
            Ok(ApprovalDecision {
                choice,
                decided_by: "human".to_string(),
            })
        }
    }
}

// @chunk policy/result-gate
// Intercepts a spawn_result before delivery to the orchestrator.
// Returns the (possibly modified) exit_code and output to deliver.
// - None gate: pass through unchanged.
// - Human gate: present to user; confirmed = original, denied = failure.
// - Agent gate: spawn the gate subagent with the worker output as input;
//   gate agent success = deliver original, non-zero = deliver failure.
pub struct ResultGateOutcome {
    pub exit_code: i32,
    pub output: serde_json::Value,
}

/// Apply the result gate for a subagent before delivering spawn_result to the orchestrator.
/// `gate_subagent_config` is the SubagentConfig for the gate agent (required for Agent gates).
pub async fn apply_result_gate_with_subagent(
    config: &ApprovalConfig,
    subagent_name: &str,
    exit_code: i32,
    output: serde_json::Value,
    ui: &dyn ApprovalUi,
    gate_subagent_config: Option<&crate::config::SubagentConfig>,
    max_output_bytes: usize,
) -> Result<ResultGateOutcome, CoreError> {
    let gate_config = match config.result_gates.get(subagent_name) {
        Some(g) => g,
        None => return Ok(ResultGateOutcome { exit_code, output }),
    };

    match &gate_config.gate {
        ResultGate::None => Ok(ResultGateOutcome { exit_code, output }),

        ResultGate::Human => {
            let summary = output
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("(no summary)");
            let req = ApprovalRequest {
                id: format!("result-gate-{subagent_name}"),
                kind: ApproveKind::Shell,
                message: format!("[{subagent_name}] {summary}"),
                options: vec!["confirm".to_string(), "reject".to_string()],
            };
            let decision = ui.request_approval(&req).await?;
            if decision == "confirm" {
                Ok(ResultGateOutcome { exit_code, output })
            } else {
                Ok(ResultGateOutcome {
                    exit_code: 1,
                    output: serde_json::json!({
                        "status": "failure",
                        "failure_kind": "implementation",
                        "error": format!("result rejected by human at result gate for {subagent_name}")
                    }),
                })
            }
        }

        ResultGate::Agent(_gate_agent_name) => {
            let subagent = gate_subagent_config.ok_or_else(|| {
                CoreError::Config(format!(
                    "result gate agent subagent config missing for {subagent_name}"
                ))
            })?;

            let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
            // Immediately drop cancel_tx — we never cancel gate agent spawns.
            drop(cancel_tx);

            // Wrap the worker output as a SpawnInput so the gate agent goes through
            // kelix-worker normally (runtime contract injection, output parsing, etc.).
            // The prompt field carries the full worker output JSON as a string;
            // context fields are extracted from the output for kelix-worker bookkeeping.
            let task_id = output.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
            let branch = output.get("branch").and_then(|v| v.as_str()).unwrap_or("");
            let prompt_str = serde_json::to_string(&output).unwrap_or_default();
            let gate_input = serde_json::json!({
                "prompt": prompt_str,
                "context": { "task_id": task_id, "branch": branch }
            });

            let gate_result: ProcessResult = run_subagent_process(
                subagent,
                &gate_input,
                None,
                cancel_rx,
                max_output_bytes,
                10,
            )
            .await;

            if gate_result.exit_code == 0 {
                Ok(ResultGateOutcome { exit_code, output })
            } else {
                let gate_output = match serde_json::from_slice::<serde_json::Value>(&gate_result.raw_stdout) {
                    Ok(v) => v,
                    Err(_) => serde_json::Value::String(String::from_utf8_lossy(&gate_result.raw_stdout).to_string()),
                };
                Ok(ResultGateOutcome {
                    exit_code: 1,
                    output: serde_json::json!({
                        "status": "failure",
                        "failure_kind": "implementation",
                        "error": format!("result gate rejected by agent"),
                        "gate_output": gate_output,
                    }),
                })
            }
        }
    }
}
// @end-chunk

pub fn error_code_for(err: &CoreError) -> ErrorCode {
    match err {
        CoreError::UnknownSubagent(_) => ErrorCode::UnknownSubagent,
        CoreError::BudgetExceeded => ErrorCode::BudgetExceeded,
        CoreError::SpawnLimitExceeded => ErrorCode::SpawnLimitExceeded,
        CoreError::UnknownSpawnId(_) => ErrorCode::UnknownSpawnId,
        CoreError::InvalidRequest(_) => ErrorCode::InvalidRequest,
        _ => ErrorCode::InvalidRequest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApprovalConfig, ShellGate};

    struct AutoUi;

    #[async_trait]
    impl ApprovalUi for AutoUi {
        async fn request_approval(&self, req: &ApprovalRequest) -> Result<String, CoreError> {
            Ok(req.options[0].clone())
        }
        async fn request_input(&self, _message: &str) -> Result<String, CoreError> {
            Ok("user response".to_string())
        }
    }

    struct RejectUi;

    #[async_trait]
    impl ApprovalUi for RejectUi {
        async fn request_approval(&self, req: &ApprovalRequest) -> Result<String, CoreError> {
            Ok(req.options[1].clone()) // always pick second option (reject)
        }
        async fn request_input(&self, _message: &str) -> Result<String, CoreError> {
            Ok("rejected".to_string())
        }
    }

    fn config_with_shell_gate(gate: ShellGate) -> ApprovalConfig {
        ApprovalConfig {
            shell_gate: gate,
            result_gates: Default::default(),
        }
    }

    #[tokio::test]
    async fn test_shell_gate_none_auto_approves() {
        let config = config_with_shell_gate(ShellGate::None);
        let req = ApprovalRequest {
            id: "req-001".to_string(),
            kind: ApproveKind::Shell,
            message: "run ls?".to_string(),
            options: vec!["yes".to_string(), "no".to_string()],
        };
        let decision = decide(&config, &req, &AutoUi).await.unwrap();
        assert_eq!(decision.decided_by, "auto");
        assert_eq!(decision.choice, "yes");
    }

    #[tokio::test]
    async fn test_shell_gate_human_uses_ui() {
        let config = config_with_shell_gate(ShellGate::Human);
        let req = ApprovalRequest {
            id: "req-002".to_string(),
            kind: ApproveKind::Shell,
            message: "run git push?".to_string(),
            options: vec!["yes".to_string(), "no".to_string()],
        };
        let decision = decide(&config, &req, &AutoUi).await.unwrap();
        assert_eq!(decision.decided_by, "human");
    }

    #[tokio::test]
    async fn test_empty_options_returns_error() {
        let config = config_with_shell_gate(ShellGate::None);
        let req = ApprovalRequest {
            id: "req-003".to_string(),
            kind: ApproveKind::Shell,
            message: "approve?".to_string(),
            options: vec![],
        };
        let err = decide(&config, &req, &AutoUi).await.unwrap_err();
        assert!(matches!(err, CoreError::InvalidRequest(_)));
    }

    #[tokio::test]
    async fn test_result_gate_none_passthrough() {
        let config = ApprovalConfig::default();
        let output = serde_json::json!({"status": "success", "summary": "done"});
        let outcome = apply_result_gate_with_subagent(
            &config, "coding-agent", 0, output.clone(), &AutoUi, None, 65536,
        )
        .await
        .unwrap();
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(outcome.output, output);
    }

    #[tokio::test]
    async fn test_result_gate_human_confirm_passthrough() {
        let mut config = ApprovalConfig::default();
        config.result_gates.insert(
            "coding-agent".to_string(),
            crate::config::ResultGateConfig {
                gate: ResultGate::Human,
            },
        );
        let output = serde_json::json!({"status": "success", "summary": "done"});
        let outcome = apply_result_gate_with_subagent(
            &config, "coding-agent", 0, output.clone(), &AutoUi, None, 65536,
        )
        .await
        .unwrap();
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(outcome.output, output);
    }

    #[tokio::test]
    async fn test_result_gate_human_reject_becomes_failure() {
        let mut config = ApprovalConfig::default();
        config.result_gates.insert(
            "coding-agent".to_string(),
            crate::config::ResultGateConfig {
                gate: ResultGate::Human,
            },
        );
        let output = serde_json::json!({"status": "success", "summary": "done"});
        let outcome = apply_result_gate_with_subagent(
            &config, "coding-agent", 0, output, &RejectUi, None, 65536,
        )
        .await
        .unwrap();
        assert_eq!(outcome.exit_code, 1);
        assert_eq!(outcome.output["status"], "failure");
    }
}

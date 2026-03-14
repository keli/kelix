/// Approval gate routing.
/// Routes approve requests to: human (TUI/stdin), agent (spawn), or auto (none).
/// See CORE_PROTOCOL.md §4.2 and DESIGN.md §4.
use crate::config::{ApprovalConfig, Gate};
use crate::error::CoreError;
use crate::protocol::core_msg::{ApproveKind, ErrorCode};
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

/// Route an approve request through the configured gate.
///
/// Returns `Err(CoreError::InvalidRequest)` if `options` is empty.
/// The caller is responsible for recording the decision in the session log.
pub async fn decide(
    config: &ApprovalConfig,
    req: &ApprovalRequest,
    ui: &dyn ApprovalUi,
    is_approval_agent_spawn: bool,
) -> Result<ApprovalDecision, CoreError> {
    if req.options.is_empty() {
        return Err(CoreError::InvalidRequest(
            "approve request must have at least one option".to_string(),
        ));
    }

    let gate = match req.kind {
        ApproveKind::Shell => &config.shell_gate,
        ApproveKind::Plan => &config.plan_gate,
        ApproveKind::Merge => &config.merge_gate,
    };

    // Approval-agent spawn requests always go to human to prevent self-approval loops.
    let effective_gate = if is_approval_agent_spawn {
        &Gate::Human
    } else {
        gate
    };

    match effective_gate {
        Gate::None => Ok(ApprovalDecision {
            choice: req.options[0].clone(),
            decided_by: "auto".to_string(),
        }),
        Gate::Human => {
            let choice = ui.request_approval(req).await?;
            Ok(ApprovalDecision {
                choice,
                decided_by: "human".to_string(),
            })
        }
        Gate::Agent => {
            // Spawn the approval-agent and use its response.
            // For now, fall through to human.
            // TODO: implement agent-gated approval
            let choice = ui.request_approval(req).await?;
            Ok(ApprovalDecision {
                choice,
                decided_by: "human".to_string(),
            })
        }
    }
}

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
    use crate::config::{ApprovalConfig, Gate};

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

    fn config_with_gate(gate: Gate) -> ApprovalConfig {
        ApprovalConfig {
            shell_gate: gate.clone(),
            plan_gate: gate.clone(),
            merge_gate: gate,
            agent: None,
        }
    }

    #[tokio::test]
    async fn test_gate_none_auto_approves() {
        let config = config_with_gate(Gate::None);
        let req = ApprovalRequest {
            id: "req-001".to_string(),
            kind: ApproveKind::Shell,
            message: "run ls?".to_string(),
            options: vec!["yes".to_string(), "no".to_string()],
        };
        let decision = decide(&config, &req, &AutoUi, false).await.unwrap();
        assert_eq!(decision.decided_by, "auto");
        assert_eq!(decision.choice, "yes");
    }

    #[tokio::test]
    async fn test_gate_human_uses_ui() {
        let config = config_with_gate(Gate::Human);
        let req = ApprovalRequest {
            id: "req-002".to_string(),
            kind: ApproveKind::Merge,
            message: "merge?".to_string(),
            options: vec!["yes".to_string(), "no".to_string()],
        };
        let decision = decide(&config, &req, &AutoUi, false).await.unwrap();
        assert_eq!(decision.decided_by, "human");
    }

    #[tokio::test]
    async fn test_empty_options_returns_error() {
        let config = config_with_gate(Gate::None);
        let req = ApprovalRequest {
            id: "req-003".to_string(),
            kind: ApproveKind::Plan,
            message: "approve plan?".to_string(),
            options: vec![],
        };
        let err = decide(&config, &req, &AutoUi, false).await.unwrap_err();
        assert!(matches!(err, CoreError::InvalidRequest(_)));
    }

    #[tokio::test]
    async fn test_approval_agent_spawn_always_goes_to_human() {
        let config = config_with_gate(Gate::None); // would be auto
        let req = ApprovalRequest {
            id: "req-004".to_string(),
            kind: ApproveKind::Shell,
            message: "spawn approval-agent?".to_string(),
            options: vec!["yes".to_string()],
        };
        // is_approval_agent_spawn = true should override Gate::None → human
        let decision = decide(&config, &req, &AutoUi, true).await.unwrap();
        assert_eq!(decision.decided_by, "human");
    }
}

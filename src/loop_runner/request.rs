use tokio::process::ChildStdin;

use crate::config::Config;
use crate::error::CoreError;
use crate::frontend::Frontend;
use crate::policy::gate::{self, ApprovalRequest};
use crate::protocol::core_msg::{CoreMessage, OrchestratorRequest};
use crate::session::state::Session;
use crate::spawn::SpawnDispatcher;

use super::util::{config_value, debug_log, log_turn, write_message};

/// Handle a single request from the orchestrator.
/// Returns `true` if the loop should exit (i.e. `complete` received).
pub async fn handle_request(
    req: &OrchestratorRequest,
    config: &Config,
    session: &mut Session,
    orch_stdin: &mut ChildStdin,
    dispatcher: &mut SpawnDispatcher,
    frontend: &dyn Frontend,
    debug: bool,
) -> Result<bool, CoreError> {
    match req {
        OrchestratorRequest::Spawn {
            id,
            subagent,
            input,
        } => {
            debug_log(
                debug,
                &format!("spawn request: id={id}, subagent={subagent}, input={input}"),
            );
            session.spawn_count += 1;
            match dispatcher.dispatch(id.clone(), subagent, input.clone(), session.spawn_count) {
                Ok(()) => {
                    let ack = CoreMessage::SpawnAck { id: id.clone() };
                    debug_log(debug, &format!("core -> orchestrator: spawn_ack id={id}"));
                    write_message(orch_stdin, &ack).await?;
                    // @chunk loop-runner/worker-events
                    // Notify the frontend that a new worker has started.
                    // In headless mode this emits a worker_started event to the adapter.
                    frontend.render_worker_started(id, subagent).await;
                    // @end-chunk
                    log_turn(session, None, Some(subagent.clone()), None).await;
                }
                Err(e) => {
                    session.spawn_count -= 1; // rollback
                    let msg = CoreMessage::Error {
                        id: id.clone(),
                        code: gate::error_code_for(&e),
                        message: e.to_string(),
                    };
                    debug_log(
                        debug,
                        &format!("core -> orchestrator: error id={id} message={e}"),
                    );
                    write_message(orch_stdin, &msg).await?;
                }
            }
        }

        OrchestratorRequest::Approve {
            id,
            kind,
            message,
            options,
        } => {
            let req = ApprovalRequest {
                id: id.clone(),
                kind: kind.clone(),
                message: message.clone(),
                options: options.clone(),
            };

            match gate::decide(&config.approval, &req, frontend).await {
                Ok(decision) => {
                    let msg = CoreMessage::ApproveResult {
                        id: id.clone(),
                        choice: decision.choice.clone(),
                        decided_by: decision.decided_by,
                    };
                    debug_log(
                        debug,
                        &format!(
                            "core -> orchestrator: approve_result id={id} choice={}",
                            decision.choice
                        ),
                    );
                    write_message(orch_stdin, &msg).await?;
                    log_turn(
                        session,
                        Some(format!("approval:{}", message)),
                        None,
                        Some(decision.choice),
                    )
                    .await;
                }
                Err(e) => {
                    let msg = CoreMessage::Error {
                        id: id.clone(),
                        code: gate::error_code_for(&e),
                        message: e.to_string(),
                    };
                    debug_log(
                        debug,
                        &format!("core -> orchestrator: error id={id} message={e}"),
                    );
                    write_message(orch_stdin, &msg).await?;
                }
            }
        }

        OrchestratorRequest::ConfigGet { id, key } => {
            let value = config_value(config, key);
            let msg = CoreMessage::ConfigResult {
                id: id.clone(),
                key: key.clone(),
                value,
            };
            debug_log(
                debug,
                &format!("core -> orchestrator: config_result id={id} key={key}"),
            );
            write_message(orch_stdin, &msg).await?;
        }

        OrchestratorRequest::Complete { id: _, summary } => {
            frontend.render_complete(summary).await;
            return Ok(true); // Signal loop exit
        }

        OrchestratorRequest::Blocked { id, message } => {
            match frontend.request_input(message).await {
                Ok(input) => {
                    let msg = CoreMessage::BlockedResult {
                        id: id.clone(),
                        input: input.clone(),
                    };
                    debug_log(
                        debug,
                        &format!("core -> orchestrator: blocked_result id={id}"),
                    );
                    write_message(orch_stdin, &msg).await?;
                    log_turn(
                        session,
                        Some(format!("blocked:{}", message)),
                        None,
                        Some(input),
                    )
                    .await;
                }
                Err(e) => {
                    // If input fails (e.g. EOF), abort the session.
                    let abort = CoreMessage::SessionAbort {
                        id: format!("evt-{}", uuid::Uuid::new_v4()),
                        reason: format!("input error: {e}"),
                    };
                    let _ = write_message(orch_stdin, &abort).await;
                    return Err(e);
                }
            }
        }

        OrchestratorRequest::Notify {
            id: _,
            message,
            level,
        } => {
            let level_str = level
                .as_ref()
                .map(|l| format!("{l:?}").to_lowercase())
                .unwrap_or_else(|| "info".to_string());
            frontend.render_notify(message, &level_str).await;
            // notify is fire-and-forget: no response
        }

        OrchestratorRequest::CancelSpawn {
            id,
            spawn_id,
            grace_period_secs: _,
        } => {
            let status = dispatcher.cancel(spawn_id);
            let msg = CoreMessage::CancelResult {
                id: id.clone(),
                spawn_id: spawn_id.clone(),
                status,
            };
            debug_log(
                debug,
                &format!("core -> orchestrator: cancel_result id={id} spawn_id={spawn_id}"),
            );
            write_message(orch_stdin, &msg).await?;
        }
    }

    Ok(false)
}

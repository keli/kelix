/// The core turn loop.
///
/// Owns the orchestrator process stdin/stdout. Runs a tokio::select! loop that:
/// - Reads orchestrator requests (one JSON line at a time)
/// - Handles spawn requests asynchronously
/// - Handles all other requests synchronously (inline await)
/// - Pushes spawn_result and user_input events back to the orchestrator
///
/// See CORE_PROTOCOL.md for the full specification.
mod request;
mod util;

use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::config::Config;
use crate::error::CoreError;
use crate::frontend::{Frontend, FrontendEvent};
use crate::protocol::core_msg::{
    CoreMessage, OrchestratorRequest, ProtocolInfo, SessionStartConfig,
};
use crate::session::state::Session;
use crate::spawn::{SpawnDispatcher, SpawnedResult};

use request::handle_request;
use util::{
    debug_log, format_orchestrator_exit_detail, parse_worker_output, persist_session_state,
    render_worker_debug_chunk, write_message,
};

fn new_id(prefix: &str) -> String {
    format!("{}-{}", prefix, Uuid::new_v4())
}

// @chunk loop-runner/loop-exit
// Describes how the orchestrator process terminated.
// Returned from `run` so that `main.rs` can decide whether to
// auto-restart (Handover) or mark suspended.
#[derive(Debug)]
pub enum LoopExit {
    /// Session suspended cleanly (orchestrator sent `complete`, wall-clock limit exceeded,
    /// adapter session end, etc.).
    Suspended { reason: &'static str },
    /// Orchestrator exited with exit code 3 (planned handover).
    /// The payload is the last JSON line written by the orchestrator before closing stdout.
    Handover { payload: Option<serde_json::Value> },
}
// @end-chunk

/// Run a complete session turn loop.
///
/// `prompt` is the user's original prompt.
/// `recovery` and `handover` are set on session resume or planned handover.
pub async fn run(
    config: Config,
    session: &mut Session,
    prompt: String,
    recovery: bool,
    handover: Option<serde_json::Value>,
    frontend: Arc<dyn Frontend>,
    debug: bool,
) -> Result<LoopExit, CoreError> {
    let mut debug_enabled = debug;
    // Spawn the orchestrator process.
    let orchestrator_config = config
        .subagents
        .get("orchestrator")
        .ok_or_else(|| CoreError::UnknownSubagent("orchestrator".to_string()))?
        .clone();

    // Expand {session_id} placeholder in command strings before env-var expansion.
    let expand_session = |s: &str| s.replace("{session_id}", &session.id);
    let stop_command = orchestrator_config.stop_command.as_deref().map(expand_session);
    let argv = crate::policy::parse_command(&expand_session(&orchestrator_config.start_command))?;
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| CoreError::InvalidCommand("orchestrator command is empty".to_string()))?;

    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(CoreError::Io)?;
    frontend.render_notify("Orchestrator started", "info").await;
    debug_log(
        debug_enabled,
        &format!("spawned orchestrator: {} {}", program, args.join(" ")),
    );

    let mut orch_stdin: ChildStdin = child.stdin.take().expect("stdin piped");
    let orch_stdout: ChildStdout = child.stdout.take().expect("stdout piped");
    let mut orch_reader = BufReader::new(orch_stdout).lines();

    // Stderr is always piped and read line-by-line in a background task.
    // In debug mode each line is printed immediately; in all modes lines are
    // buffered so they are available when reporting a crash or startup timeout.
    let (stderr_line_tx, mut stderr_line_rx) = mpsc::channel::<String>(256);
    if let Some(orch_stderr) = child.stderr.take() {
        let debug_now = debug_enabled;
        tokio::spawn(async move {
            let mut lines = BufReader::new(orch_stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if debug_now {
                    eprintln!("[orchestrator stderr] {line}");
                }
                // Best-effort; drop lines if receiver is gone.
                let _ = stderr_line_tx.try_send(line);
            }
        });
    }
    // Helper: drain buffered stderr lines into a single string.
    let collect_stderr = |rx: &mut mpsc::Receiver<String>| {
        let mut lines = Vec::new();
        while let Ok(line) = rx.try_recv() {
            lines.push(line);
        }
        lines.join("\n")
    };
    // stderr_task retained only for compatibility with the crash path below.
    let stderr_task: Option<tokio::task::JoinHandle<String>> = None;

    // Channel for completed spawn results.
    let (result_tx, mut result_rx) = mpsc::channel::<SpawnedResult>(64);
    // Channel for live worker stdout/stderr chunks.
    let (stream_tx, mut stream_rx) =
        mpsc::channel::<(String, crate::spawn::process_runner::WorkerStreamChunk)>(256);

    // Channel for user input and control events from the frontend.
    let (frontend_event_tx, mut frontend_event_rx) = mpsc::channel::<FrontendEvent>(16);

    // Spawn frontend input pump.
    let frontend_clone = Arc::clone(&frontend);
    let frontend_event_tx_clone = frontend_event_tx.clone();
    tokio::spawn(async move {
        while let Some(event) = frontend_clone.next_event().await {
            if frontend_event_tx_clone.send(event).await.is_err() {
                break;
            }
        }
    });

    let mut dispatcher = SpawnDispatcher::new(config.clone(), result_tx, stream_tx);

    // Send session_start to the orchestrator.
    let available_subagents: Vec<String> = if session.enabled_subagents.is_empty() {
        config.subagents.keys().cloned().collect()
    } else {
        config
            .subagents
            .keys()
            .filter(|k| session.enabled_subagents.contains(k))
            .cloned()
            .collect()
    };

    let session_start_subagents = available_subagents.clone();
    let session_start = CoreMessage::SessionStart {
        id: new_id("init"),
        prompt: prompt.clone(),
        recovery,
        session_id: session.id.clone(),
        handover,
        config: SessionStartConfig {
            subagents: session_start_subagents.clone(),
            max_spawns: config.agent.max_spawns,
            max_concurrent_spawns: config.agent.max_concurrent_spawns,
            max_wall_time_secs: config.agent.max_wall_time_secs,
            protocol: ProtocolInfo {
                request_types: OrchestratorRequest::all_type_names(),
                request_fields: OrchestratorRequest::field_schema(),
                instructions: OrchestratorRequest::protocol_instructions(),
            },
        },
    };
    debug_log(
        debug_enabled,
        &format!(
            "sending session_start: session_id={}, recovery={}, subagents={:?}",
            session.id, recovery, session_start_subagents
        ),
    );
    write_message(&mut orch_stdin, &session_start).await?;

    // Wall-clock timer (0 = disabled).
    let wall_time = config.agent.max_wall_time_secs;
    let wall_clock_future: tokio::time::Sleep = if wall_time > 0 {
        tokio::time::sleep(std::time::Duration::from_secs(wall_time))
    } else {
        tokio::time::sleep(std::time::Duration::MAX)
    };
    tokio::pin!(wall_clock_future);

    // @chunk loop-runner/exit-detection
    // Track the last raw line received from the orchestrator.
    // Used to extract the handover payload when exit code 3 is detected.
    let mut last_raw_line: Option<String> = None;
    // @end-chunk

    // Startup timeout: fires if the orchestrator produces no output within N seconds
    // of session_start. Detects hangs caused by auth failures or misconfiguration.
    // Cleared after the first line is received. 0 = disabled. Always disabled in
    // debug mode so the operator can observe stderr without being forcibly killed.
    let startup_timeout_secs = if debug_enabled {
        0
    } else {
        config.agent.orchestrator_startup_timeout_secs
    };
    let startup_deadline = if startup_timeout_secs > 0 {
        tokio::time::sleep(std::time::Duration::from_secs(startup_timeout_secs))
    } else {
        tokio::time::sleep(std::time::Duration::MAX)
    };
    tokio::pin!(startup_deadline);
    let mut startup_done = startup_timeout_secs == 0;

    // @chunk loop-runner/inactivity-timeout
    // Inactivity timeout: fires if the orchestrator produces no output for N seconds
    // after the last received line. Detects stuck orchestrators (e.g. blocked on a
    // backend call with no progress). Resets on every received line. 0 = disabled.
    let inactivity_secs = config.agent.orchestrator_inactivity_timeout_secs;
    let inactivity_duration = if inactivity_secs > 0 {
        std::time::Duration::from_secs(inactivity_secs)
    } else {
        std::time::Duration::MAX
    };
    let inactivity_deadline = tokio::time::sleep(inactivity_duration);
    tokio::pin!(inactivity_deadline);
    // @end-chunk

    // Main select loop.
    loop {
        tokio::select! {
            // Inactivity timeout: orchestrator has not produced output since the last line.
            _ = &mut inactivity_deadline, if inactivity_secs > 0 => {
                graceful_kill(&mut child, stop_command.as_deref()).await;
                let stderr_text = collect_stderr(&mut stderr_line_rx);
                let detail = if stderr_text.is_empty() {
                    format!(
                        "orchestrator did not respond for {inactivity_secs}s (no output). \
                         Treating as stuck and killing."
                    )
                } else {
                    format!(
                        "orchestrator did not respond for {inactivity_secs}s. stderr:\n{stderr_text}"
                    )
                };
                return Err(CoreError::OrchestratorExit(detail));
            }

            // Startup timeout: orchestrator produced no output since session_start.
            _ = &mut startup_deadline, if !startup_done => {
                graceful_kill(&mut child, stop_command.as_deref()).await;
                let stderr_text = collect_stderr(&mut stderr_line_rx);
                let detail = if stderr_text.is_empty() {
                    format!(
                        "orchestrator did not respond within {startup_timeout_secs}s of session_start \
                         (no stderr output). Check auth credentials and [subagents.orchestrator].start_command."
                    )
                } else {
                    format!(
                        "orchestrator did not respond within {startup_timeout_secs}s of session_start. \
                         stderr:\n{stderr_text}"
                    )
                };
                return Err(CoreError::OrchestratorExit(detail));
            }

            // Wall-clock limit.
            _ = &mut wall_clock_future, if wall_time > 0 => {
                let abort = CoreMessage::SessionAbort {
                    id: new_id("evt"),
                    reason: "wall_time_exceeded".to_string(),
                };
                let _ = write_message(&mut orch_stdin, &abort).await;
                drop(orch_stdin);
                graceful_kill(&mut child, stop_command.as_deref()).await;
                session.mark_suspended();
                persist_session_state(session).await;
                return Ok(LoopExit::Suspended { reason: "wall-clock time limit exceeded" });
            }

            // Spawn result delivered from a worker task.
            Some(spawned) = result_rx.recv() => {
                let is_cancelled = spawned.result.process_error.as_deref() == Some("cancelled");
                if !is_cancelled {
                    let output = parse_worker_output(&spawned.result.raw_stdout);
                    let msg = CoreMessage::SpawnResult {
                        id: spawned.spawn_id.clone(),
                        exit_code: spawned.result.exit_code,
                        output,
                        truncated: if spawned.result.truncated { Some(true) } else { None },
                    };
                    write_message(&mut orch_stdin, &msg).await?;
                }
                // @chunk loop-runner/worker-events
                // Notify the frontend that this worker has finished.
                // In headless mode this emits a worker_finished event to the adapter.
                frontend.render_worker_finished(&spawned.spawn_id, spawned.result.exit_code).await;
                // @end-chunk
                dispatcher.complete(&spawned.spawn_id);
            }

            // Live stdout/stderr chunk from an in-flight worker.
            Some((spawn_id, stream_chunk)) = stream_rx.recv() => {
                if debug_enabled {
                    render_worker_debug_chunk(frontend.as_ref(), &spawn_id, stream_chunk.stream, &stream_chunk.chunk).await;
                }
            }

            // Frontend delivered a user message or control event.
            Some(event) = frontend_event_rx.recv() => {
                match event {
                    FrontendEvent::UserMessage(message) => {
                        let msg = CoreMessage::UserInput {
                            id: new_id("evt"),
                            text: message.text,
                            metadata: message.metadata,
                        };
                        write_message(&mut orch_stdin, &msg).await?;
                    }
                    FrontendEvent::DebugMode { enabled } => {
                        debug_enabled = enabled.unwrap_or(!debug_enabled);
                        let status = if debug_enabled { "enabled" } else { "disabled" };
                        frontend
                            .render_notify(&format!("Debug mode {status}"), "info")
                            .await;
                    }
                    FrontendEvent::SessionEnd => {
                        let abort = CoreMessage::SessionAbort {
                            id: new_id("evt"),
                            reason: "adapter_session_end".to_string(),
                        };
                        let _ = write_message(&mut orch_stdin, &abort).await;
                        drop(orch_stdin);
                        graceful_kill(&mut child, stop_command.as_deref()).await;
                        session.mark_suspended();
                        persist_session_state(session).await;
                        return Ok(LoopExit::Suspended { reason: "session ended by adapter" });
                    }
                }
            }

            // Orchestrator sent a request.
            line_result = orch_reader.next_line() => {
                match line_result {
                    Err(e) => return Err(CoreError::Io(e)),
                    Ok(None) => {
                        // @chunk loop-runner/crash-counter
                        // Orchestrator closed stdout without sending `complete`.
                        // Read exit code to distinguish planned handover (exit 3) from crash.
                        let status = child.wait().await.ok();
                        let exit_code = status.and_then(|s| s.code()).unwrap_or(-1);
                        let stderr_text = collect_stderr(&mut stderr_line_rx);
                        drop(stderr_task);
                        debug_log(debug_enabled, &format!("orchestrator stdout closed, exit_code={exit_code}"));

                        if exit_code == 3 {
                            // Planned handover: extract payload from last line, reset crash counter.
                            let payload = last_raw_line
                                .as_deref()
                                .and_then(|l| serde_json::from_str(l).ok());
                            session.mark_suspended();
                            session.reset_crash_counter();
                            persist_session_state(session).await;
                            return Ok(LoopExit::Handover { payload });
                        }

                        // Unclean exit: increment crash counter.
                        session.increment_crash();
                        persist_session_state(session).await;
                        let detail = format_orchestrator_exit_detail(exit_code, &stderr_text);
                        return Err(CoreError::OrchestratorExit(detail));
                        // @end-chunk
                    }
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        startup_done = true;
                        last_raw_line = Some(line.clone());
                        // Reset inactivity deadline on every received line.
                        if inactivity_secs > 0 {
                            inactivity_deadline.as_mut().reset(
                                tokio::time::Instant::now() + inactivity_duration,
                            );
                        }
                        debug_log(debug_enabled, &format!("orchestrator -> core: {line}"));

                        let req: OrchestratorRequest = match serde_json::from_str(&line) {
                            Ok(r) => r,
                            Err(e) => {
                                eprintln!("warn: failed to parse orchestrator request: {e}: {line}");
                                continue;
                            }
                        };

                        let should_break = handle_request(
                            &req,
                            &config,
                            session,
                            &mut orch_stdin,
                            &mut dispatcher,
                            frontend.as_ref(),
                            debug_enabled,
                        ).await?;

                        if should_break {
                            break;
                        }
                    }
                }
            }
        }
    }

    // Suspend after `complete` so the session is resumable on the next user message.
    // The session log records the full history; `complete` is a protocol-level signal
    // that the current task finished, not that the conversation is over.
    session.mark_suspended();
    session.reset_crash_counter();
    persist_session_state(session).await;

    // @chunk loop-runner/complete-exit-handshake
    // After `complete`, close orchestrator stdin before waiting for process exit.
    // Some orchestrator backends keep reading stdin in a loop and only terminate
    // on EOF; without dropping stdin here, core can block indefinitely on wait().
    drop(orch_stdin);
    // Wait for orchestrator to exit.
    graceful_kill(&mut child, stop_command.as_deref()).await;
    // @end-chunk
    Ok(LoopExit::Suspended { reason: "task complete" })
}

// @chunk loop-runner/graceful-kill
// Wait up to 5 s for the child to exit on its own, then kill it.
// If stop_command is set, run it first so container runtimes (e.g. podman) can
// stop the inner container before the outer process is force-killed. The stop
// command gets up to 10 s; after that the child is killed unconditionally.
async fn graceful_kill(child: &mut tokio::process::Child, stop_command: Option<&str>) {
    if let Some(cmd) = stop_command {
        if let Ok(argv) = crate::policy::parse_command(cmd) {
            if let Some((program, args)) = argv.split_first() {
                let stop_timeout = tokio::time::Duration::from_secs(10);
                let _ = tokio::time::timeout(
                    stop_timeout,
                    tokio::process::Command::new(program).args(args).status(),
                )
                .await;
            }
        }
    }
    let timeout = tokio::time::Duration::from_secs(5);
    if tokio::time::timeout(timeout, child.wait()).await.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}
// @end-chunk

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loop_exit_variants() {
        // Verify the enum variants are constructible (compile-time check).
        let _suspended = LoopExit::Suspended { reason: "test" };
        let _handover = LoopExit::Handover { payload: None };
        let _handover_with = LoopExit::Handover {
            payload: Some(serde_json::json!({"next_prompt": "continue"})),
        };
    }
}

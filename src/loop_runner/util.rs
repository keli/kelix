use chrono::Utc;
use tokio::io::AsyncWriteExt;
use tokio::process::ChildStdin;

use crate::config::Config;
use crate::error::CoreError;
use crate::frontend::Frontend;
use crate::protocol::core_msg::CoreMessage;
use crate::session::index::SessionEntry;
use crate::session::state::{Session, Turn};
use crate::spawn::process_runner::WorkerStream;

const ORCHESTRATOR_ERROR_PREFIX: &str = "KELIX_ORCH_ERROR ";

pub fn debug_log(enabled: bool, message: &str) {
    if enabled {
        eprintln!("[debug] {message}");
    }
}

pub async fn render_worker_debug_chunk(
    frontend: &dyn Frontend,
    spawn_id: &str,
    stream: WorkerStream,
    chunk: &str,
) {
    let stream_name = match stream {
        WorkerStream::Stdout => "stdout",
        WorkerStream::Stderr => "stderr",
    };

    for line in chunk
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
    {
        let msg = format!("{}", truncate_for_debug(line, 800));
        frontend
            .render_worker_stream(spawn_id, stream_name, &msg)
            .await;
    }
}

pub fn truncate_for_debug(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for ch in s.chars() {
        if count >= max_chars {
            out.push_str("...");
            break;
        }
        out.push(ch);
        count += 1;
    }
    out
}

/// Write a single JSON message followed by a newline to the orchestrator's stdin.
pub async fn write_message(stdin: &mut ChildStdin, msg: &CoreMessage) -> Result<(), CoreError> {
    let mut line = serde_json::to_vec(msg)?;
    line.push(b'\n');
    stdin.write_all(&line).await?;
    stdin.flush().await?;
    Ok(())
}

/// Attempt to parse worker stdout as JSON; fall back to raw string.
pub fn parse_worker_output(raw: &[u8]) -> serde_json::Value {
    if raw.is_empty() {
        return serde_json::Value::String(String::new());
    }
    match serde_json::from_slice(raw) {
        Ok(v) => v,
        Err(_) => serde_json::Value::String(String::from_utf8_lossy(raw).to_string()),
    }
}

/// Look up a config value by dot-separated key path.
pub fn config_value(config: &Config, key: &str) -> serde_json::Value {
    match key {
        "tools.shell.timeout_secs" => serde_json::json!(config.tools.shell.timeout_secs),
        "tools.shell.max_output_bytes" => serde_json::json!(config.tools.shell.max_output_bytes),
        "tools.shell.enabled" => serde_json::json!(config.tools.shell.enabled),
        "agent.max_spawns" => serde_json::json!(config.agent.max_spawns),
        "agent.max_concurrent_spawns" => serde_json::json!(config.agent.max_concurrent_spawns),
        "agent.max_wall_time_secs" => serde_json::json!(config.agent.max_wall_time_secs),
        "budget.max_tokens" => serde_json::json!(config.budget.max_tokens),
        _ => serde_json::Value::Null,
    }
}

/// Append a turn to the in-memory session and persist to the JSONL log.
pub async fn log_turn(
    session: &mut Session,
    prompt: Option<String>,
    subagent_cmd: Option<String>,
    output: Option<String>,
) {
    let turn = Turn {
        timestamp: Utc::now(),
        prompt,
        subagent_cmd,
        output,
    };
    session.append_turn(turn.clone());
    let _ = crate::session::log::append_turn(&session.id, &turn).await;
}

/// Persist session state to the index.
pub async fn persist_session_state(session: &Session) {
    let entry = SessionEntry {
        id: session.id.clone(),
        config_path: session.config_path.clone(),
        state: session.state.clone(),
        last_active: session.last_active,
        enabled_subagents: session.enabled_subagents.clone(),
        crash_counter: session.crash_counter,
    };
    let _ = crate::session::index::update(|idx| idx.upsert(entry)).await;
}

pub fn format_orchestrator_exit_detail(exit_code: i32, stderr_text: &str) -> String {
    let report = parse_orchestrator_error_report(stderr_text);
    let stderr_summary = summarize_stderr(stderr_text);

    let append_mount_hint = should_append_mount_hint(report.as_ref());
    if stderr_summary.is_empty() && report.is_none() {
        let mut detail = format!("exit code {exit_code} (no stderr output)");
        if append_mount_hint {
            detail.push_str(". Check [subagents.orchestrator].command and its auth mounts.");
        }
        return detail;
    }

    let primary_message = report
        .as_ref()
        .map(|r| r.message.as_str())
        .filter(|m| !m.is_empty())
        .unwrap_or(stderr_summary.as_str());
    let mut detail = format!("exit code {exit_code}. stderr: {primary_message}");
    if append_mount_hint {
        detail.push_str(". Check [subagents.orchestrator].command and its auth mounts.");
    }
    detail
}

// @chunk loop-runner/orchestrator-error-report
// Parse machine-readable orchestrator stderr envelope.
// This avoids branching on provider-specific prose.
#[derive(Debug, serde::Deserialize)]
struct OrchestratorErrorReport {
    category: String,
    message: String,
}

fn parse_orchestrator_error_report(stderr_text: &str) -> Option<OrchestratorErrorReport> {
    stderr_text.lines().find_map(|line| {
        let trimmed = line.trim();
        let payload = trimmed.strip_prefix(ORCHESTRATOR_ERROR_PREFIX)?;
        serde_json::from_str::<OrchestratorErrorReport>(payload).ok()
    })
}

fn summarize_stderr(stderr_text: &str) -> String {
    stderr_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with(ORCHESTRATOR_ERROR_PREFIX))
        .take(3)
        .collect::<Vec<_>>()
        .join(" | ")
}

fn should_append_mount_hint(report: Option<&OrchestratorErrorReport>) -> bool {
    match report {
        Some(r) => r.category != "protocol_contract",
        None => true,
    }
}
// @end-chunk

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_orchestrator_exit_detail_omits_mount_hint_for_protocol_contract_report() {
        let detail = format_orchestrator_exit_detail(
            1,
            r#"KELIX_ORCH_ERROR {"category":"protocol_contract","code":"orchestrator_json_contract_violation","message":"claude orchestrator turn failed: could not parse JSON from agent output: not json"}"#,
        );

        assert!(detail.contains("could not parse JSON from agent output"));
        assert!(!detail.contains("auth mounts"));
    }

    #[test]
    fn test_format_orchestrator_exit_detail_keeps_mount_hint_for_runtime_report() {
        let detail = format_orchestrator_exit_detail(
            1,
            r#"KELIX_ORCH_ERROR {"category":"runtime","code":"orchestrator_runtime_failure","message":"failed to spawn claude: No such file or directory"}"#,
        );

        assert!(detail.contains("failed to spawn claude"));
        assert!(detail.contains("auth mounts"));
    }
}

use serde_json::Value;
use std::fmt;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use super::error_report::{emit_orchestrator_error, OrchestratorErrorCategory};
use super::log::{append_turn_log, stderr_stdio, TurnLog};
use super::normalize::{
    normalize_orchestrator_json_lines, synthesize_blocked_from_non_json, NormalizeErrorKind,
};
use super::runtime_contract::{CONTINUATION_PREFIX, FIRST_TURN_SUFFIX};

pub fn run_codex_session(system_prompt: &str, log_file: Option<&Path>) {
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin.lock());
    let mut stdout = io::stdout();
    let mut line = String::new();
    let mut session_id: Option<String> = None;

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                eprintln!("failed to read stdin: {e}");
                std::process::exit(1);
            }
        }

        let request = line.trim();
        if request.is_empty() {
            continue;
        }

        let prompt = build_codex_prompt(system_prompt, request, session_id.is_none());
        let turn = match run_codex_turn(session_id.as_deref(), &prompt, log_file) {
            Ok(turn) => turn,
            Err(e) => {
                let message = format!("codex orchestrator turn failed: {e}");
                emit_orchestrator_error(e.category(), e.code(), &message);
                std::process::exit(1);
            }
        };

        if session_id.is_none() {
            session_id = turn.session_id;
        }

        if writeln!(stdout, "{}", turn.response).is_err() || stdout.flush().is_err() {
            break;
        }
    }

    std::process::exit(0);
}

pub fn build_codex_prompt(system_prompt: &str, request: &str, is_first_turn: bool) -> String {
    let mut prompt = String::new();

    if is_first_turn {
        prompt.push_str(system_prompt);
        prompt.push_str(FIRST_TURN_SUFFIX);
    } else {
        prompt.push_str(CONTINUATION_PREFIX);
    }

    prompt.push_str("\n# Current Input\n");
    prompt.push_str(request);
    prompt
}

pub struct CodexTurn {
    pub response: String,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum CodexTurnErrorKind {
    Runtime,
}

#[derive(Debug, Clone)]
pub struct CodexTurnError {
    kind: CodexTurnErrorKind,
    message: String,
}

impl CodexTurnError {
    fn runtime(message: impl Into<String>) -> Self {
        Self {
            kind: CodexTurnErrorKind::Runtime,
            message: message.into(),
        }
    }

    fn category(&self) -> OrchestratorErrorCategory {
        match self.kind {
            CodexTurnErrorKind::Runtime => OrchestratorErrorCategory::Runtime,
        }
    }

    fn code(&self) -> &'static str {
        match self.kind {
            CodexTurnErrorKind::Runtime => "orchestrator_runtime_failure",
        }
    }
}

impl fmt::Display for CodexTurnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

pub fn run_codex_turn(
    session_id: Option<&str>,
    prompt: &str,
    log_file: Option<&Path>,
) -> Result<CodexTurn, CodexTurnError> {
    let mut child = Command::new("codex")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr_stdio(log_file))
        .args(codex_args(session_id))
        .spawn()
        .map_err(|e| CodexTurnError::runtime(format!("failed to spawn codex: {e}")))?;

    if let Some(mut child_stdin) = child.stdin.take() {
        child_stdin
            .write_all(prompt.as_bytes())
            .map_err(|e| CodexTurnError::runtime(format!("failed to write codex prompt: {e}")))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| CodexTurnError::runtime(format!("failed to wait for codex: {e}")))?;

    if let Some(path) = log_file {
        append_turn_log(
            path,
            &TurnLog {
                backend: "codex",
                session_id,
                stdout: &output.stdout,
            },
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut detected_session_id = None;
    let agent_messages: Vec<String> = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .inspect(|v| {
            if detected_session_id.is_none()
                && v.get("type").and_then(|t| t.as_str()) == Some("thread.started")
            {
                detected_session_id = v
                    .get("thread_id")
                    .and_then(|id| id.as_str())
                    .map(str::to_string);
            }
        })
        .filter(|v| {
            matches!(
                v.get("type").and_then(|t| t.as_str()),
                Some("item.completed") | Some("item.created")
            )
        })
        .filter_map(|v| {
            let item = v.get("item")?;
            let kind = item.get("type")?.as_str()?;
            if kind == "agent_message" || kind == "message" {
                item.get("text")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .or_else(|| {
                        item.get("content").and_then(|v| v.as_array()).map(|arr| {
                            arr.iter()
                                .filter_map(|c| c.get("text").and_then(|v| v.as_str()))
                                .collect::<Vec<_>>()
                                .join("")
                        })
                    })
                    .or_else(|| {
                        item.get("content")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    })
            } else {
                None
            }
        })
        .collect();

    let last_text = agent_messages.last().ok_or_else(|| {
        CodexTurnError::runtime(format!(
            "codex produced no agent_message events (exit {})",
            output.status.code().unwrap_or(-1)
        ))
    })?;

    let response = match normalize_orchestrator_json_lines(last_text) {
        Ok(normalized_lines) => normalized_lines.join("\n"),
        Err(e) => match e.kind() {
            NormalizeErrorKind::JsonContract | NormalizeErrorKind::InvalidPayload => {
                synthesize_blocked_from_non_json(last_text)
            }
            NormalizeErrorKind::Internal => return Err(CodexTurnError::runtime(e.to_string())),
        },
    };

    Ok(CodexTurn {
        response,
        session_id: detected_session_id,
    })
}

pub fn codex_args(session_id: Option<&str>) -> Vec<String> {
    let mut args = vec!["exec".to_string()];
    if let Some(id) = session_id {
        args.push("resume".to_string());
        args.extend([
            "--json".to_string(),
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
            "--skip-git-repo-check".to_string(),
            id.to_string(),
            "-".to_string(),
        ]);
    } else {
        args.extend([
            "--json".to_string(),
            "--sandbox".to_string(),
            "danger-full-access".to_string(),
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
            "--skip-git-repo-check".to_string(),
            "-".to_string(),
        ]);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_codex_prompt_first_turn_includes_system_prompt() {
        let prompt = build_codex_prompt(
            "You are an orchestrator.",
            r#"{"type":"session_start"}"#,
            true,
        );
        assert!(prompt.contains("You are an orchestrator."));
        assert!(prompt.contains("Runtime Contract"));
    }

    #[test]
    fn test_build_codex_prompt_subsequent_turn_omits_system_prompt() {
        let prompt = build_codex_prompt("System", r#"{"type":"spawn_result"}"#, false);
        assert!(prompt.contains("Continue the orchestrator session"));
        assert!(!prompt.contains("System\n\n"));
    }
}

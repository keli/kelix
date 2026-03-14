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

pub fn run_claude_session(system_prompt: &str, log_file: Option<&Path>) {
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin.lock());
    let mut stdout = io::stdout();
    let mut line = String::new();
    let mut session_id: Option<String> = None;
    let mut is_first_turn = true;

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

        let prompt = build_claude_prompt(system_prompt, request, is_first_turn);
        is_first_turn = false;

        let turn = match run_claude_turn(session_id.as_deref(), &prompt, log_file) {
            Ok(turn) => turn,
            Err(e) => {
                let message = format!("claude orchestrator turn failed: {e}");
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

pub fn build_claude_prompt(system_prompt: &str, request: &str, is_first_turn: bool) -> String {
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

pub struct ClaudeTurn {
    pub response: String,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum ClaudeTurnErrorKind {
    Runtime,
}

#[derive(Debug, Clone)]
pub struct ClaudeTurnError {
    kind: ClaudeTurnErrorKind,
    message: String,
}

impl ClaudeTurnError {
    fn runtime(message: impl Into<String>) -> Self {
        Self {
            kind: ClaudeTurnErrorKind::Runtime,
            message: message.into(),
        }
    }

    fn category(&self) -> OrchestratorErrorCategory {
        match self.kind {
            ClaudeTurnErrorKind::Runtime => OrchestratorErrorCategory::Runtime,
        }
    }

    fn code(&self) -> &'static str {
        match self.kind {
            ClaudeTurnErrorKind::Runtime => "orchestrator_runtime_failure",
        }
    }
}

impl fmt::Display for ClaudeTurnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

pub fn run_claude_turn(
    session_id: Option<&str>,
    prompt: &str,
    log_file: Option<&Path>,
) -> Result<ClaudeTurn, ClaudeTurnError> {
    let output = Command::new("claude")
        .args(claude_args(session_id, prompt))
        .stdout(Stdio::piped())
        .stderr(stderr_stdio(log_file))
        .output()
        .map_err(|e| ClaudeTurnError::runtime(format!("failed to spawn claude: {e}")))?;

    if let Some(path) = log_file {
        append_turn_log(
            path,
            &TurnLog {
                backend: "claude",
                session_id,
                stdout: &output.stdout,
                stderr: &output.stderr,
            },
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut detected_session_id = None;

    let result_event = stdout
        .lines()
        .filter_map(|l| {
            let parsed = serde_json::from_str::<Value>(l).ok()?;
            if detected_session_id.is_none() {
                detected_session_id = extract_session_id(&parsed);
            }
            Some(parsed)
        })
        .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("result"))
        .ok_or_else(|| {
            ClaudeTurnError::runtime(format!(
                "claude produced no result event (exit {})",
                output.status.code().unwrap_or(-1)
            ))
        })?;

    let subtype = result_event
        .get("subtype")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let result_text = result_event
        .get("result")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if subtype != "success" {
        return Err(ClaudeTurnError::runtime(format!(
            "claude failed ({subtype}): {result_text}"
        )));
    }

    let response = match normalize_orchestrator_json_lines(result_text) {
        Ok(normalized_lines) => normalized_lines.join("\n"),
        Err(e) => match e.kind() {
            NormalizeErrorKind::JsonContract | NormalizeErrorKind::InvalidPayload => {
                synthesize_blocked_from_non_json(result_text)
            }
            NormalizeErrorKind::Internal => return Err(ClaudeTurnError::runtime(e.to_string())),
        },
    };

    Ok(ClaudeTurn {
        response,
        session_id: detected_session_id,
    })
}

pub fn claude_args(session_id: Option<&str>, prompt: &str) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        prompt.to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--dangerously-skip-permissions".to_string(),
    ];
    if let Some(id) = session_id {
        args.extend(["--resume".to_string(), id.to_string()]);
    }
    args
}

fn extract_session_id(v: &Value) -> Option<String> {
    // Be permissive because stream-json event names/field names can vary by CLI version.
    v.get("session_id")
        .and_then(|id| id.as_str())
        .map(str::to_string)
        .or_else(|| {
            v.get("sessionId")
                .and_then(|id| id.as_str())
                .map(str::to_string)
        })
        .or_else(|| {
            v.get("conversation_id")
                .and_then(|id| id.as_str())
                .map(str::to_string)
        })
        .or_else(|| {
            v.get("conversationId")
                .and_then(|id| id.as_str())
                .map(str::to_string)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claude_args_without_resume() {
        let args = claude_args(None, "hello");
        assert!(args.iter().any(|a| a == "-p"));
        assert!(!args.iter().any(|a| a == "--resume"));
    }

    #[test]
    fn test_claude_args_with_resume() {
        let args = claude_args(Some("sess-123"), "hello");
        let resume_pos = args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(
            args.get(resume_pos + 1).map(String::as_str),
            Some("sess-123")
        );
    }
}

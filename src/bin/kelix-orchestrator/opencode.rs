use std::fmt;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use super::error_report::{emit_orchestrator_error, OrchestratorErrorCategory};
use super::log::{append_turn_log, stderr_stdio, TurnLog};
use super::normalize::{
    normalize_orchestrator_json_from_output, synthesize_blocked_from_non_json, NormalizeErrorKind,
};
use super::runtime_contract::{CONTINUATION_PREFIX, FIRST_TURN_SUFFIX};

// @chunk orchestrator/opencode-session
// Stateless-turn orchestrator session using `opencode run`.
//
// OpenCode does not expose a streaming multi-turn stdin/stdout API, so this
// backend follows the same stateless-turn model as the Codex backend: each
// incoming JSON line from kelix core triggers a new `opencode run` invocation
// whose prompt is the system prompt followed by the full accumulated context
// (system_start message + all prior turns).
//
// Expected CLI: opencode run "<prompt>"
pub fn run_opencode_session(system_prompt: &str, log_file: Option<&Path>) {
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin.lock());
    let mut stdout = io::stdout();
    let mut line = String::new();
    let mut is_first_turn = true;
    // Accumulated conversation kept in raw JSON-line form so it can be replayed.
    let mut history: Vec<String> = Vec::new();

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

        history.push(request.to_string());

        let prompt = build_opencode_prompt(system_prompt, &history, is_first_turn);
        is_first_turn = false;

        match run_opencode_turn(&prompt, log_file) {
            Ok(response) => {
                // Record the assistant reply for subsequent context.
                history.push(format!("assistant: {response}"));
                if writeln!(stdout, "{response}").is_err() || stdout.flush().is_err() {
                    break;
                }
            }
            Err(e) => {
                let message = format!("opencode orchestrator turn failed: {e}");
                emit_orchestrator_error(e.category(), e.code(), &message);
                std::process::exit(1);
            }
        }
    }

    std::process::exit(0);
}

pub fn build_opencode_prompt(
    system_prompt: &str,
    history: &[String],
    is_first_turn: bool,
) -> String {
    let mut prompt = String::new();

    if is_first_turn {
        prompt.push_str(system_prompt);
        prompt.push_str(FIRST_TURN_SUFFIX);
    } else {
        prompt.push_str(CONTINUATION_PREFIX);
    }

    prompt.push_str("\n# Conversation History\n");
    for entry in history.iter().take(history.len().saturating_sub(1)) {
        prompt.push_str(entry);
        prompt.push('\n');
    }

    prompt.push_str("\n# Current Input\n");
    if let Some(last) = history.last() {
        prompt.push_str(last);
    }

    prompt
}

#[derive(Debug, Clone, Copy)]
pub enum OpencodeTurnErrorKind {
    Runtime,
}

#[derive(Debug, Clone)]
pub struct OpencodeTurnError {
    kind: OpencodeTurnErrorKind,
    message: String,
}

impl OpencodeTurnError {
    fn runtime(message: impl Into<String>) -> Self {
        Self {
            kind: OpencodeTurnErrorKind::Runtime,
            message: message.into(),
        }
    }

    fn category(&self) -> OrchestratorErrorCategory {
        match self.kind {
            OpencodeTurnErrorKind::Runtime => OrchestratorErrorCategory::Runtime,
        }
    }

    fn code(&self) -> &'static str {
        match self.kind {
            OpencodeTurnErrorKind::Runtime => "orchestrator_runtime_failure",
        }
    }
}

impl fmt::Display for OpencodeTurnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

pub fn run_opencode_turn(prompt: &str, log_file: Option<&Path>) -> Result<String, OpencodeTurnError> {
    let output = Command::new("opencode")
        .args(["run", prompt])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(stderr_stdio(log_file))
        .output()
        .map_err(|e| OpencodeTurnError::runtime(format!("failed to spawn opencode: {e}")))?;

    if let Some(path) = log_file {
        append_turn_log(
            path,
            &TurnLog {
                backend: "opencode",
                session_id: None,
                stdout: &output.stdout,
            },
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let normalized = match normalize_orchestrator_json_from_output(&stdout) {
        Ok(normalized) => normalized,
        Err(e) => match e.kind() {
            NormalizeErrorKind::JsonContract | NormalizeErrorKind::InvalidPayload => {
                synthesize_blocked_from_non_json(&stdout)
            }
            NormalizeErrorKind::Internal => return Err(OpencodeTurnError::runtime(e.to_string())),
        },
    };

    if !output.status.success() {
        return Err(OpencodeTurnError::runtime(format!(
            "opencode exited with code {} despite producing output",
            output.status.code().unwrap_or(-1)
        )));
    }

    Ok(normalized)
}
// @end-chunk

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_opencode_prompt_first_turn_includes_system_prompt() {
        let history = vec![r#"{"id":"init","type":"session_start"}"#.to_string()];
        let prompt = build_opencode_prompt("You are an orchestrator.", &history, true);
        assert!(prompt.contains("You are an orchestrator."));
        assert!(prompt.contains("Runtime Contract"));
        assert!(prompt.contains("session_start"));
    }

    #[test]
    fn test_build_opencode_prompt_subsequent_turn_omits_system_prompt() {
        let history = vec![
            r#"{"id":"init","type":"session_start"}"#.to_string(),
            r#"assistant: {"id":"r1","type":"notify","message":"ok"}"#.to_string(),
            r#"{"id":"r2","type":"spawn_result"}"#.to_string(),
        ];
        let prompt = build_opencode_prompt("System", &history, false);
        assert!(prompt.contains("Continue the orchestrator session"));
        assert!(prompt.contains("session_start"));
        assert!(prompt.contains("spawn_result"));
    }

    #[test]
    fn test_build_opencode_prompt_current_input_is_last_history_entry() {
        let history = vec![
            "turn1".to_string(),
            "turn2".to_string(),
            "current_message".to_string(),
        ];
        let prompt = build_opencode_prompt("sys", &history, false);
        let current_section = prompt.split("# Current Input").nth(1).unwrap_or("");
        assert!(current_section.contains("current_message"));
    }
}

// @chunk kelix-worker/main
// Generic worker entrypoint for kelix subagent containers.
//
// Reads a spawn-input JSON object from stdin, invokes the selected agent
// backend CLI, and writes a worker-result JSON object to stdout before
// exiting with the appropriate exit code per ORCHESTRATOR_PROTOCOL.md §5.
//
// Usage:
//   kelix-worker --agent claude   # coding, planning, research, etc.
//   kelix-worker --agent codex    # review, analysis, etc.
//
// The --system-prompt flag embeds a markdown prompt file at build time via
// the --system-prompt-file flag, or accepts a literal string.
//
// Input (stdin):
//   { "prompt": "...", "context": { "task_id": "...", "branch": "..." } }
//
// Output (stdout, per Worker Output Contract):
//   { "task_id": "...", "status": "success|failure|blocked|handover",
//     "branch": "...", "base_revision": "...", "summary": "...",
//     "error": "...", "failure_kind": "...", "blocked_reason": "...",
//     "handover": { ... } }
//
// Exit codes (ORCHESTRATOR_PROTOCOL.md §5):
//   0 = success, 1 = failure, 2 = blocked, 3 = handover
// @end-chunk

mod claude;
mod codex;
mod helpers;
mod opencode;
mod runtime_contract;
mod types;

use clap::{Parser, ValueEnum};
use std::io::{self, Read};

use helpers::emit_failure;
use runtime_contract::WORKER_RUNTIME_CONTRACT;
use types::{SpawnInput, WorkerResult};

// @chunk kelix-worker/cli
#[derive(Debug, Parser)]
#[command(name = "kelix-worker", about = "kelix worker entrypoint")]
struct Cli {
    /// Agent backend to invoke.
    #[arg(long, value_enum)]
    agent: AgentBackend,

    /// System prompt prepended to the task prompt (literal string).
    #[arg(long, default_value = "")]
    system_prompt: String,

    /// Path to a file whose contents are appended to the system prompt.
    /// May be specified multiple times; files are concatenated in order.
    #[arg(long)]
    system_prompt_file: Vec<std::path::PathBuf>,
}

#[derive(Debug, Clone, ValueEnum)]
enum AgentBackend {
    /// Claude Code (`claude -p --output-format stream-json`)
    Claude,
    /// OpenAI Codex (`codex exec --json`)
    Codex,
    /// OpenCode (`opencode run`)
    Opencode,
}
// @end-chunk

fn main() {
    let mut cli = Cli::parse();

    for path in &cli.system_prompt_file {
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                if !cli.system_prompt.is_empty() {
                    cli.system_prompt.push('\n');
                }
                cli.system_prompt.push_str(&contents);
            }
            Err(e) => {
                emit_failure(
                    "unknown",
                    "",
                    &format!(
                        "failed to read --system-prompt-file {}: {e}",
                        path.display()
                    ),
                    types::FailureKind::Implementation,
                );
                std::process::exit(1);
            }
        }
    }

    let mut raw = String::new();
    if io::stdin().read_to_string(&mut raw).is_err() {
        emit_failure("unknown", "", "failed to read stdin", types::FailureKind::Implementation);
        std::process::exit(1);
    }

    let input: SpawnInput = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            emit_failure(
                "unknown",
                "",
                &format!("malformed stdin JSON: {e}"),
                types::FailureKind::Implementation,
            );
            std::process::exit(1);
        }
    };

    let task_id = &input.context.task_id;
    let branch = &input.context.branch;

    // @chunk kelix-worker/prompt-construction
    // Prepend system prompt and non-optional runtime contract before task context.
    let contract: &str = &*WORKER_RUNTIME_CONTRACT;
    let full_prompt = if cli.system_prompt.is_empty() {
        format!(
            "{contract}\n\n# Task Context\ntask_id: {task_id}\nbranch: {branch}\n\n# Task\n{}",
            input.prompt
        )
    } else {
        format!(
            "{}\n\n{contract}\n\n# Task Context\ntask_id: {task_id}\nbranch: {branch}\n\n# Task\n{}",
            cli.system_prompt, input.prompt
        )
    };
    // @end-chunk

    let agent_output = match cli.agent {
        AgentBackend::Claude => claude::run_claude(&full_prompt, task_id, branch),
        AgentBackend::Codex => codex::run_codex(&full_prompt, task_id, branch),
        AgentBackend::Opencode => opencode::run_opencode(&full_prompt, task_id, branch),
    };

    let result = match agent_output {
        Ok(r) => r,
        Err(e) => {
            emit_failure(task_id, branch, &e, types::FailureKind::Implementation);
            std::process::exit(1);
        }
    };

    let result = match validate_worker_result(result, task_id, branch) {
        Ok(r) => r,
        Err(e) => {
            emit_failure(task_id, branch, &e, types::FailureKind::Implementation);
            std::process::exit(1);
        }
    };

    let exit_code = result.status.exit_code();

    println!("{}", serde_json::to_string(&result).unwrap());
    std::process::exit(exit_code);
}

// @chunk kelix-worker/result-validation
// Enforce protocol-valid worker status values regardless of prompt content.
// status is already a typed enum so deserialization handles validation;
// this function fills in missing context fields from the spawn input.
fn validate_worker_result(
    mut result: WorkerResult,
    task_id: &str,
    branch: &str,
) -> Result<WorkerResult, String> {
    if result.task_id.is_empty() {
        result.task_id = task_id.to_string();
    }
    if result.branch.is_empty() {
        result.branch = branch.to_string();
    }
    Ok(result)
}
// @end-chunk

#[cfg(test)]
mod tests {
    use super::types::{SpawnInput, WorkerResult, WorkerStatus};
    use super::validate_worker_result;

    #[test]
    fn test_worker_result_deserialize_full() {
        let json = r#"{
            "task_id": "t1",
            "status": "success",
            "branch": "main",
            "base_revision": "abc123",
            "summary": "all done"
        }"#;
        let r: WorkerResult = serde_json::from_str(json).unwrap();
        assert_eq!(r.task_id, "t1");
        assert_eq!(r.status, WorkerStatus::Success);
        assert_eq!(r.summary, "all done");
    }

    #[test]
    fn test_worker_result_deserialize_rejects_unknown_status() {
        // Unknown status must fail deserialization (typed enum).
        let json = r#"{"task_id":"t1","status":"mystery","branch":"main"}"#;
        assert!(serde_json::from_str::<WorkerResult>(json).is_err());
    }

    #[test]
    fn test_worker_result_deserialize_minimal_missing_prompt_field() {
        // SpawnInput requires `prompt`; missing it must produce an error.
        let json = r#"{"context": {"task_id": "t1", "branch": "main"}}"#;
        assert!(serde_json::from_str::<SpawnInput>(json).is_err());
    }

    #[test]
    fn test_validate_worker_result_fills_missing_task_context_fields() {
        let result = WorkerResult {
            task_id: String::new(),
            status: WorkerStatus::Success,
            branch: String::new(),
            ..Default::default()
        };

        let normalized =
            validate_worker_result(result, "task-from-context", "branch-from-context").unwrap();
        assert_eq!(normalized.task_id, "task-from-context");
        assert_eq!(normalized.branch, "branch-from-context");
    }

    #[test]
    fn test_worker_status_exit_codes() {
        assert_eq!(WorkerStatus::Success.exit_code(), 0);
        assert_eq!(WorkerStatus::Failure.exit_code(), 1);
        assert_eq!(WorkerStatus::Blocked.exit_code(), 2);
        assert_eq!(WorkerStatus::Handover.exit_code(), 3);
    }
}

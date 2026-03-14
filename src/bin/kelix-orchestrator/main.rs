// @chunk orchestrator/main
// Entrypoint for the orchestrator container.
//
// Spawns the selected agent backend with bidirectional stream I/O and the
// orchestrator system prompt, then forwards stdin/stdout between the caller
// and the backend process.
//
// The system prompt is provided via the same CLI flags as `kelix-worker`.
// @end-chunk

mod claude;
mod codex;
mod error_report;
mod log;
mod normalize;
mod opencode;
mod runtime_contract;

use clap::{Parser, ValueEnum};

// @chunk orchestrator/cli
#[derive(Debug, Parser)]
#[command(name = "kelix-orchestrator", about = "kelix orchestrator entrypoint")]
struct Cli {
    /// Agent backend to invoke.
    #[arg(long, value_enum)]
    agent: AgentBackend,

    /// System prompt prepended to the orchestrator session (literal string).
    #[arg(long, default_value = "")]
    system_prompt: String,

    /// Path to a file whose contents are appended to the system prompt.
    /// May be specified multiple times; files are concatenated in order.
    #[arg(long)]
    system_prompt_file: Vec<std::path::PathBuf>,

    /// If set, append each agent turn's raw stream-json output to this file.
    #[arg(long)]
    log_file: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, ValueEnum)]
enum AgentBackend {
    /// Claude Code stream-json session.
    Claude,
    /// Reserved for a future Codex session backend.
    Codex,
    /// OpenCode stateless-turn session.
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
                eprintln!(
                    "failed to read --system-prompt-file {}: {e}",
                    path.display()
                );
                std::process::exit(1);
            }
        }
    }

    if cli.system_prompt.is_empty() {
        eprintln!(
            "missing orchestrator system prompt: pass --system-prompt or --system-prompt-file"
        );
        std::process::exit(1);
    }

    match cli.agent {
        AgentBackend::Claude => {
            claude::run_claude_session(&cli.system_prompt, cli.log_file.as_deref())
        }
        AgentBackend::Codex => codex::run_codex_session(&cli.system_prompt, cli.log_file.as_deref()),
        AgentBackend::Opencode => {
            opencode::run_opencode_session(&cli.system_prompt, cli.log_file.as_deref())
        }
    }
}

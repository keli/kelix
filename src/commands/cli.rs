use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

use crate::adapter::AdapterOptions;
use crate::gateway::GatewayOptions;
use crate::tui_client::TuiOptions;

#[derive(Parser)]
#[command(
    name = "kelix",
    about = "Unified entrypoint for core, gateway, and TUI"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start a new session: launch gateway (if not running) and open TUI.
    Start {
        /// Path to kelix.toml config file.
        #[arg(required_unless_present_any = ["list_examples", "example"])]
        config: Option<PathBuf>,

        /// Built-in example alias (e.g. onboarding, codex-onboarding, claude-onboarding).
        #[arg(long, conflicts_with = "config", conflicts_with = "list_examples")]
        example: Option<String>,

        /// List available example .toml configs and exit.
        #[arg(long)]
        list_examples: bool,

        /// Initial prompt for the orchestrator.
        #[arg(long)]
        prompt: Option<String>,

        /// Comma-separated subagent allowlist for the new session.
        #[arg(long, value_delimiter = ',')]
        enabled_subagents: Vec<String>,

        /// Session id (default: timestamp-based).
        #[arg(long)]
        session: Option<String>,

        /// Sender id attached to TUI messages.
        #[arg(long, default_value = "kelix-tui")]
        sender_id: String,

        /// Gateway bind address.
        #[arg(long, default_value = "127.0.0.1:9000")]
        listen_addr: String,

        /// Path to the kelix executable used by gateway to spawn core sessions.
        #[arg(long)]
        core_bin: Option<String>,

        /// Enable verbose diagnostics: orchestrator stderr is printed in real time.
        /// Equivalent to setting KELIX_DEBUG=1 in the environment.
        #[arg(long)]
        debug: bool,
    },
    /// Attach TUI to a running gateway session (gateway must already be up).
    Attach {
        /// Session id to attach to.
        #[arg(long)]
        session: String,

        /// Sender id attached to TUI messages.
        #[arg(long, default_value = "kelix-tui")]
        sender_id: String,

        /// Gateway WebSocket address.
        #[arg(long, default_value = "127.0.0.1:9000")]
        listen_addr: String,
    },
    /// Resume a suspended session and open TUI.
    Resume {
        /// Session ID to resume.
        id: String,

        /// Override the 3-crash safety limit and force a resume attempt.
        #[arg(long)]
        force: bool,

        /// Sender id attached to TUI messages.
        #[arg(long, default_value = "kelix-tui")]
        sender_id: String,

        /// Gateway bind address.
        #[arg(long, default_value = "127.0.0.1:9000")]
        listen_addr: String,

        /// Path to the kelix executable used by gateway to spawn core sessions.
        #[arg(long)]
        core_bin: Option<String>,

        /// Enable verbose diagnostics: orchestrator stderr is printed in real time.
        /// Equivalent to setting KELIX_DEBUG=1 in the environment.
        #[arg(long)]
        debug: bool,
    },
    /// List all known sessions.
    List {
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove sessions inactive for longer than --days (default: 30).
    /// Active sessions are never removed.
    Purge {
        /// Remove sessions not active within this many days.
        #[arg(long, default_value = "30")]
        days: u64,

        /// Remove all non-active sessions regardless of age.
        #[arg(long)]
        all: bool,

        /// Print what would be removed without actually removing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Advanced/debug entrypoint for core process behavior.
    /// Most users should use top-level `start/resume/list`.
    Core(CoreCli),
    /// Run the WebSocket gateway (no config required; sessions supply their own).
    Gateway(GatewayOptions),
    /// Run an interactive TUI client connected to gateway.
    Tui(TuiOptions),
    /// Run external chat adapters (Telegram now; more providers later).
    Adapter(AdapterOptions),
}

#[derive(Args)]
#[command(about = "Advanced core namespace (top-level start/resume/list recommended)")]
pub struct CoreCli {
    /// Enable verbose runtime diagnostics for orchestrator I/O and process behavior.
    #[arg(long)]
    pub debug: bool,

    #[command(subcommand)]
    pub command: CoreCommands,
}

#[derive(Subcommand)]
pub enum CoreCommands {
    /// Start a new session.
    Start {
        /// Path to kelix.toml config file.
        #[arg(required_unless_present_any = ["list_examples", "example"])]
        config: Option<PathBuf>,

        /// Built-in example alias (e.g. onboarding, codex-onboarding, claude-onboarding).
        #[arg(long, conflicts_with = "config", conflicts_with = "list_examples")]
        example: Option<String>,

        /// List available example .toml configs and exit.
        #[arg(long)]
        list_examples: bool,

        /// Initial prompt for the orchestrator.
        #[arg(long)]
        prompt: Option<String>,

        /// Comma-separated list of subagents to enable (default: all).
        #[arg(long, value_delimiter = ',')]
        enabled_subagents: Vec<String>,

        /// Override the session ID (useful for adapter-controlled sessions).
        #[arg(long)]
        session_id: Option<String>,
    },

    /// Resume a suspended session.
    Resume {
        /// Session ID to resume.
        id: String,

        /// Override the 3-crash safety limit and force a resume attempt.
        #[arg(long)]
        force: bool,
    },

    /// List all known sessions.
    List {
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub mod cli;
mod core;
mod examples;
mod purge;
mod start;

use anyhow::Result;
use clap::Parser;

use crate::gateway;
use crate::tui_client;

pub use cli::{Cli, Commands, CoreCli, CoreCommands};

use core::run_core;
use purge::run_purge;
use start::{ensure_gateway_running, run_start};

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Start {
            config,
            example,
            list_examples,
            prompt,
            enabled_subagents,
            session,
            sender_id,
            listen_addr,
            core_bin,
            debug,
        } => {
            if debug {
                std::env::set_var("KELIX_DEBUG", "1");
            }
            run_start(
                config,
                example,
                list_examples,
                prompt,
                enabled_subagents,
                session,
                sender_id,
                listen_addr,
                core_bin,
            )
            .await
        }
        Commands::Attach {
            session,
            sender_id,
            listen_addr,
        } => {
            let working_dir = std::env::current_dir()?;
            tui_client::run(tui_client::TuiOptions {
                url: format!("ws://{}", listen_addr),
                session,
                sender_id,
                config: None,
                working_dir,
                enabled_subagents: vec![],
                initial_message: None,
                resume: false,
                force: false,
            })
            .await
        }
        Commands::Resume {
            id,
            force,
            sender_id,
            listen_addr,
            core_bin,
            debug,
        } => {
            if debug {
                std::env::set_var("KELIX_DEBUG", "1");
            }
            use anyhow::Context;
            let core_bin = match core_bin {
                Some(path) => path,
                None => std::env::current_exe()
                    .context("failed to resolve current executable path for core_bin")?
                    .to_string_lossy()
                    .to_string(),
            };
            ensure_gateway_running(
                &listen_addr,
                &gateway::GatewayOptions {
                    core_bin,
                    listen_addr: listen_addr.clone(),
                },
            )
            .await?;
            let working_dir = std::env::current_dir()?;
            tui_client::run(tui_client::TuiOptions {
                url: format!("ws://{}", listen_addr),
                session: id,
                sender_id,
                config: None,
                working_dir,
                enabled_subagents: vec![],
                initial_message: None,
                resume: true,
                force,
            })
            .await
        }
        Commands::List { json } => {
            run_core(CoreCli {
                debug: false,
                command: CoreCommands::List { json },
            })
            .await
        }
        Commands::Purge { days, all, dry_run } => run_purge(days, all, dry_run).await,
        Commands::Core(core) => run_core(core).await,
        Commands::Gateway(options) => gateway::run(options).await,
        Commands::Tui(options) => tui_client::run(options).await,
    }
}

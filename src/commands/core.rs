use anyhow::{Context, Result};
use std::sync::Arc;
use uuid::Uuid;

use crate::frontend;
use crate::loop_runner::{self, LoopExit};
use crate::session;
use crate::session::index::{SessionEntry, SessionIndex};
use crate::session::state::{Session, SessionState};

use super::cli::{CoreCli, CoreCommands};
use super::examples::print_example_configs;

// @chunk main/ensure-workspace-state-dir
// Ensure workspace-local state directory exists before spawning orchestrator.
// Some example configs mount `$PWD/.kelix` into orchestrator containers and
// runtime startup fails if the host path is missing.
fn ensure_workspace_state_dir() -> Result<()> {
    let cwd = std::env::current_dir().context("failed to resolve current directory")?;
    let state_dir = cwd.join(".kelix");
    std::fs::create_dir_all(&state_dir)
        .with_context(|| format!("failed to create state directory: {}", state_dir.display()))?;
    Ok(())
}
// @end-chunk

pub async fn run_core(core: CoreCli) -> Result<()> {
    let debug = core.debug || debug_env_enabled();

    match core.command {
        CoreCommands::Start {
            config: config_path,
            example,
            list_examples,
            prompt,
            enabled_subagents,
            session_id,
        } => {
            if list_examples {
                print_example_configs()?;
                return Ok(());
            }

            let config_path = match (config_path, example) {
                (Some(path), None) => path,
                (None, Some(name)) => super::examples::resolve_example_config(&name)?,
                (None, None) => {
                    anyhow::bail!(
                        "config path is required unless --list-examples or --example is specified"
                    )
                }
                (Some(_), Some(_)) => {
                    unreachable!("clap should prevent passing both config and --example")
                }
            };
            let config = crate::config::load(&config_path)
                .with_context(|| format!("failed to load config: {}", config_path.display()))?;
            ensure_workspace_state_dir()?;

            let id = session_id.unwrap_or_else(|| format!("sess-{}", Uuid::new_v4()));

            let mut session = Session::new(id.clone(), config_path.clone(), enabled_subagents);

            // Register in session index as active.
            let entry = SessionEntry {
                id: id.clone(),
                config_path,
                state: SessionState::Active,
                last_active: session.last_active,
                enabled_subagents: session.enabled_subagents.clone(),
                crash_counter: 0,
            };
            session::index::update(|idx| idx.upsert(entry))
                .await
                .context("failed to write session index")?;

            let frontend = make_frontend(id.clone());

            // If no prompt was supplied on the command line, ask interactively via the frontend.
            let prompt = match prompt {
                Some(p) => p,
                None => frontend
                    .request_input("What would you like to do?")
                    .await
                    .context("failed to read initial prompt")?,
            };

            // @chunk main/start-restart-loop
            // Restart loop: continues on planned handover (exit 3), stops on completion or error.
            // After 3 consecutive unclean exits the session is suspended and the user must
            // resume manually with `kelix resume --force <id>`.
            let mut recovery = false;
            let mut handover: Option<serde_json::Value> = None;
            loop {
                match loop_runner::run(
                    config.clone(),
                    &mut session,
                    prompt.clone(),
                    recovery,
                    handover.take(),
                    Arc::clone(&frontend),
                    debug,
                )
                .await
                {
                    Ok(LoopExit::Suspended { reason }) => {
                        eprintln!("Session {} suspended: {}.", id, reason);
                        break;
                    }
                    Ok(LoopExit::Handover { payload }) => {
                        // Planned handover: crash_counter was reset inside loop_runner.
                        recovery = true;
                        handover = payload;
                        // continue → re-spawn orchestrator with recovery: true
                    }
                    Err(crate::error::CoreError::OrchestratorExit(reason)) => {
                        session.mark_suspended();
                        frontend
                            .render_session_error(&format!(
                                "orchestrator exited unexpectedly: {reason}"
                            ))
                            .await;
                        let sus_entry = SessionEntry {
                            id: session.id.clone(),
                            config_path: session.config_path.clone(),
                            state: SessionState::Suspended,
                            last_active: session.last_active,
                            enabled_subagents: session.enabled_subagents.clone(),
                            crash_counter: session.crash_counter,
                        };
                        session::index::update(|idx| idx.upsert(sus_entry))
                            .await
                            .ok();

                        if session.crash_counter >= 3 {
                            eprintln!(
                                "Session {} suspended after 3 consecutive crashes. \
                                 Run `kelix core resume --force {}` to override.",
                                id, id
                            );
                        } else {
                            eprintln!(
                                "Session {} suspended (orchestrator exited unexpectedly: {}).",
                                id, reason
                            );
                        }
                        std::process::exit(1);
                    }
                    Err(e) => {
                        frontend.render_session_error(&e.to_string()).await;
                        eprintln!("Session error: {e}");
                        std::process::exit(1);
                    }
                }
            }
            // @end-chunk

            // Fallback: if the loop exits without explicitly marking the session,
            // ensure it is not left as Active in the index.
            if session.state == SessionState::Active {
                session.mark_suspended();
                let fallback_entry = SessionEntry {
                    id: session.id.clone(),
                    config_path: session.config_path.clone(),
                    state: SessionState::Suspended,
                    last_active: session.last_active,
                    enabled_subagents: session.enabled_subagents.clone(),
                    crash_counter: session.crash_counter,
                };
                session::index::update(|idx| idx.upsert(fallback_entry))
                    .await
                    .ok();
            }
        }

        CoreCommands::Resume { id, force } => {
            let index = SessionIndex::load()
                .await
                .context("failed to load session index")?;
            let entry = index
                .get(&id)
                .ok_or_else(|| anyhow::anyhow!("session '{}' not found", id))?
                .clone();


            // @chunk main/resume-crash-guard
            // Refuse auto-resume after 3 consecutive crashes unless --force is given.
            // This prevents an infinite crash loop while still allowing operator override.
            if entry.crash_counter >= 3 && !force {
                anyhow::bail!(
                    "session '{}' has crashed {} time(s) consecutively. \
                     Use `kelix core resume --force {}` to override.",
                    id,
                    entry.crash_counter,
                    id
                );
            }
            // @end-chunk

            let config = crate::config::load(&entry.config_path).with_context(|| {
                format!("failed to load config: {}", entry.config_path.display())
            })?;
            ensure_workspace_state_dir()?;

            let turns = session::log::load_turns(&id)
                .await
                .context("failed to load session log")?;
            let total_turns = turns.len() as u64;

            let mut session = Session::new(
                id.clone(),
                entry.config_path.clone(),
                entry.enabled_subagents.clone(),
            );
            session.turns = turns;
            session.total_turns = total_turns;
            // Restore crash counter; reset to 0 when --force overrides the limit.
            session.crash_counter = if force { 0 } else { entry.crash_counter };

            // Mark as active in index.
            let active_entry = SessionEntry {
                state: SessionState::Active,
                last_active: chrono::Utc::now(),
                crash_counter: session.crash_counter,
                ..entry.clone()
            };
            session::index::update(|idx| idx.upsert(active_entry))
                .await
                .context("failed to update session index")?;

            let frontend = make_frontend(id.clone());
            // prompt is reconstructed by orchestrator from session state on recovery
            let prompt = String::new();

            // @chunk main/resume-restart-loop
            // Same restart loop as Start: handles planned handover and crash limit on resume.
            let mut recovery = true;
            let mut handover: Option<serde_json::Value> = None;
            loop {
                match loop_runner::run(
                    config.clone(),
                    &mut session,
                    prompt.clone(),
                    recovery,
                    handover.take(),
                    Arc::clone(&frontend),
                    debug,
                )
                .await
                {
                    Ok(LoopExit::Suspended { reason }) => {
                        eprintln!("Session {} suspended: {}.", id, reason);
                        break;
                    }
                    Ok(LoopExit::Handover { payload }) => {
                        recovery = true;
                        handover = payload;
                    }
                    Err(crate::error::CoreError::OrchestratorExit(reason)) => {
                        session.mark_suspended();
                        frontend
                            .render_session_error(&format!(
                                "orchestrator exited unexpectedly: {reason}"
                            ))
                            .await;
                        let suspended = SessionEntry {
                            state: SessionState::Suspended,
                            last_active: chrono::Utc::now(),
                            crash_counter: session.crash_counter,
                            ..entry.clone()
                        };
                        session::index::update(|idx| idx.upsert(suspended))
                            .await
                            .ok();

                        if session.crash_counter >= 3 {
                            eprintln!(
                                "Session {} suspended after 3 consecutive crashes. \
                                 Run `kelix resume --force {}` to override.",
                                id, id
                            );
                        } else {
                            eprintln!(
                                "Session {} suspended (orchestrator exited unexpectedly: {}).",
                                id, reason
                            );
                        }
                        std::process::exit(1);
                    }
                    Err(e) => {
                        frontend.render_session_error(&e.to_string()).await;
                        eprintln!("Session error: {e}");
                        let suspended = SessionEntry {
                            state: SessionState::Suspended,
                            last_active: chrono::Utc::now(),
                            crash_counter: session.crash_counter,
                            ..entry.clone()
                        };
                        session::index::update(|idx| idx.upsert(suspended))
                            .await
                            .ok();
                        std::process::exit(1);
                    }
                }
            }
            // @end-chunk

            // Fallback: if the loop exits without explicitly marking the session,
            // ensure it is not left as Active in the index.
            if session.state == SessionState::Active {
                session.mark_suspended();
                let fallback_entry = SessionEntry {
                    state: SessionState::Suspended,
                    last_active: session.last_active,
                    crash_counter: session.crash_counter,
                    ..entry.clone()
                };
                session::index::update(|idx| idx.upsert(fallback_entry))
                    .await
                    .ok();
            }
        }

        CoreCommands::List { json } => {
            let index = SessionIndex::load()
                .await
                .context("failed to load session index")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&index)?);
            } else {
                println!("{:<40} {:<12} {}", "SESSION ID", "STATE", "LAST ACTIVE");
                println!("{}", "-".repeat(72));
                for entry in &index.sessions {
                    println!(
                        "{:<40} {:<12} {}",
                        entry.id,
                        entry.state,
                        entry.last_active.format("%Y-%m-%d %H:%M:%S UTC")
                    );
                }
            }
        }
    }

    Ok(())
}

fn make_frontend(session_id: String) -> Arc<dyn frontend::Frontend> {
    Arc::new(frontend::headless::HeadlessFrontend::new(session_id))
}

fn debug_env_enabled() -> bool {
    std::env::var("KELIX_DEBUG")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

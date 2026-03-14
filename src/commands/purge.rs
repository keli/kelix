use anyhow::{Context, Result};

use crate::session::index::SessionIndex;
use crate::session::state::SessionState;

pub async fn run_purge(days: u64, all: bool, dry_run: bool) -> Result<()> {
    let mut index = SessionIndex::load()
        .await
        .context("failed to load session index")?;

    let to_remove: Vec<_> = if all {
        index.sessions.iter().cloned().collect()
    } else {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
        index
            .sessions
            .iter()
            .filter(|e| e.state != SessionState::Active && e.last_active < cutoff)
            .cloned()
            .collect()
    };

    if to_remove.is_empty() {
        if all {
            println!("No sessions to purge.");
        } else {
            println!("No sessions to purge (older than {} days).", days);
        }
        return Ok(());
    }

    for entry in &to_remove {
        println!(
            "{} {:<40} {:<12} {}",
            if dry_run { "[dry-run]" } else { "purging" },
            entry.id,
            entry.state.to_string(),
            entry.last_active.format("%Y-%m-%d %H:%M:%S UTC"),
        );
    }

    if !dry_run {
        let removed = if all {
            let before = index.sessions.len();
            index.sessions.clear();
            before
        } else {
            index.purge_old(days)
        };
        index.save().await.context("failed to save session index")?;
        println!("Removed {} session(s).", removed);
    }

    Ok(())
}

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::net::TcpStream;
use tokio::time::{sleep, Duration};

use crate::gateway::GatewayOptions;
use crate::session::index::SessionIndex;
use crate::tui_client::{self, TuiOptions};

pub async fn run_start(
    config: Option<PathBuf>,
    example: Option<String>,
    list_examples: bool,
    prompt: Option<String>,
    enabled_subagents: Vec<String>,
    session: Option<String>,
    sender_id: String,
    listen_addr: String,
    core_bin: Option<String>,
) -> Result<()> {
    if list_examples {
        super::examples::print_example_configs()?;
        return Ok(());
    }

    let config = match (config, example) {
        (Some(path), None) => path,
        (None, Some(name)) => super::examples::resolve_example_config(&name)?,
        (None, None) => {
            anyhow::bail!(
                "config path is required unless --list-examples or --example is specified"
            )
        }
        (Some(_), Some(_)) => unreachable!("clap should prevent passing both config and --example"),
    };
    anyhow::ensure!(
        config.exists(),
        "config file not found: {}",
        config.display()
    );

    // Silently purge sessions inactive for more than 30 days on every start.
    if let Ok(mut index) = SessionIndex::load().await {
        if index.purge_old(30) > 0 {
            let _ = index.save().await;
        }
    }

    let core_bin = match core_bin {
        Some(path) => path,
        None => std::env::current_exe()
            .context("failed to resolve current executable path for core_bin")?
            .to_string_lossy()
            .to_string(),
    };

    let session = session.unwrap_or_else(default_session_id);
    let working_dir = std::env::current_dir().context("failed to resolve current working dir")?;

    ensure_gateway_running(
        &listen_addr,
        &GatewayOptions {
            core_bin,
            listen_addr: listen_addr.clone(),
        },
    )
    .await?;

    tui_client::run(TuiOptions {
        url: format!("ws://{}", listen_addr),
        session,
        sender_id,
        config: Some(config),
        working_dir,
        enabled_subagents,
        initial_message: prompt,
        resume: false,
        force: false,
    })
    .await
}

pub fn default_session_id() -> String {
    chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string()
}

pub async fn ensure_gateway_running(listen_addr: &str, options: &GatewayOptions) -> Result<()> {
    if TcpStream::connect(listen_addr).await.is_ok() {
        return Ok(());
    }

    let mut command = std::process::Command::new(
        std::env::current_exe().context("failed to resolve current executable path")?,
    );
    command
        .arg("gateway")
        .arg("--core-bin")
        .arg(&options.core_bin)
        .arg("--listen-addr")
        .arg(&options.listen_addr)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let _child = command.spawn().context("failed to spawn gateway process")?;
    wait_for_gateway(listen_addr).await
}

pub async fn wait_for_gateway(listen_addr: &str) -> Result<()> {
    for _ in 0..50 {
        if TcpStream::connect(listen_addr).await.is_ok() {
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("gateway did not become ready at {}", listen_addr);
}

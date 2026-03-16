use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::net::TcpStream;
use tokio::time::{Duration, sleep};

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

    maybe_autostart_adapter(&config)?;

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

// @chunk start/adapter-autostart
// Project-level adapter can be auto-launched from [adapter] on `kelix start`.
// Launch is fire-and-forget to keep start path responsive; stdout/stderr are
// inherited so adapter logs remain visible in the current terminal.
fn maybe_autostart_adapter(config_path: &std::path::Path) -> Result<()> {
    let config = crate::config::load(config_path)
        .with_context(|| format!("failed to load config: {}", config_path.display()))?;
    if !config.adapter.autostart {
        return Ok(());
    }
    let provider_name = config
        .adapter
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow::anyhow!("adapter.autostart=true requires adapter.provider"))?;
    let provider_cfg = config
        .adapter
        .providers
        .get(provider_name)
        .ok_or_else(|| anyhow::anyhow!("adapter provider '{}' not found", provider_name))?;
    let launch = resolve_adapter_launch(config_path, provider_name, provider_cfg)?;

    let pid_path = adapter_pid_path(config_path, provider_name);
    if let Some(existing_pid) = read_adapter_pid(&pid_path)? {
        if is_process_running(existing_pid) {
            eprintln!(
                "kelix: restarting adapter '{}' pid={} to avoid duplicates",
                provider_name, existing_pid
            );
            stop_process(existing_pid)?;
        }
    }

    let mut command = std::process::Command::new("sh");
    if let Some(ready_file) = launch.ready_file.as_ref() {
        let _ = std::fs::remove_file(ready_file);
    }
    command
        .arg("-lc")
        .arg(&launch.start_command)
        .stdin(Stdio::null());
    let child = command
        .spawn()
        .with_context(|| format!("failed to spawn adapter command: {}", launch.start_command))?;
    let pid = child.id();
    write_adapter_pid(&pid_path, child.id())?;
    if let Some(ready_file) = launch.ready_file.as_ref() {
        wait_for_ready_file(ready_file, pid)?;
    }
    Ok(())
}
// @end-chunk

struct AdapterLaunch {
    start_command: String,
    ready_file: Option<PathBuf>,
}

fn resolve_adapter_launch(
    config_path: &std::path::Path,
    provider_name: &str,
    provider_cfg: &crate::config::AdapterProviderConfig,
) -> Result<AdapterLaunch> {
    match provider_cfg {
        crate::config::AdapterProviderConfig::Builtin => {
            let ready_file = adapter_ready_file_path(config_path, provider_name);
            Ok(AdapterLaunch {
                start_command: format!(
                    "kelix adapter --provider {} --ready-file {}",
                    shell_escape(provider_name),
                    shell_escape_path(&ready_file)
                ),
                ready_file: Some(ready_file),
            })
        }
        crate::config::AdapterProviderConfig::External { start_command } => {
            let cmd = start_command.trim();
            if cmd.is_empty() {
                anyhow::bail!(
                    "adapter.providers.{}.start_command cannot be empty",
                    provider_name
                );
            }
            Ok(AdapterLaunch {
                start_command: cmd.to_string(),
                ready_file: None,
            })
        }
    }
}

fn adapter_pid_path(_config_path: &std::path::Path, provider_name: &str) -> PathBuf {
    global_kelix_dir()
        .join(format!("adapter-autostart-{}.pid", sanitize_key(provider_name)))
}

fn adapter_ready_file_path(_config_path: &std::path::Path, provider_name: &str) -> PathBuf {
    global_kelix_dir()
        .join(format!("adapter-ready-{}.flag", sanitize_key(provider_name)))
}

fn global_kelix_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".kelix")
}

fn read_adapter_pid(path: &std::path::Path) -> Result<Option<u32>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read adapter pid file: {}", path.display()))?;
    let parsed = raw.trim().parse::<u32>().ok();
    Ok(parsed)
}

fn write_adapter_pid(path: &std::path::Path, pid: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, pid.to_string())
        .with_context(|| format!("failed to write adapter pid file: {}", path.display()))?;
    Ok(())
}

fn is_process_running(pid: u32) -> bool {
    use nix::sys::signal;
    use nix::unistd::Pid;
    signal::kill(Pid::from_raw(pid as i32), None).is_ok()
}

fn stop_process(pid: u32) -> Result<()> {
    use nix::sys::signal::{self, Signal};
    use nix::unistd::Pid;
    let nix_pid = Pid::from_raw(pid as i32);
    if signal::kill(nix_pid, Signal::SIGTERM).is_err() {
        return Ok(());
    }

    for _ in 0..20 {
        if !is_process_running(pid) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    let _ = signal::kill(nix_pid, Signal::SIGKILL);
    Ok(())
}

fn wait_for_ready_file(path: &std::path::Path, pid: u32) -> Result<()> {
    for _ in 0..100 {
        if path.exists() {
            return Ok(());
        }
        if !is_process_running(pid) {
            anyhow::bail!(
                "adapter process exited before readiness signal: {}",
                path.display()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    anyhow::bail!("adapter readiness timeout waiting for {}", path.display());
}

fn sanitize_key(raw: &str) -> String {
    raw.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn shell_escape(raw: &str) -> String {
    if raw
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return raw.to_string();
    }
    format!("'{}'", raw.replace('\'', "'\"'\"'"))
}

fn shell_escape_path(path: &std::path::Path) -> String {
    shell_escape(&path.to_string_lossy())
}

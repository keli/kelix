/// Single subagent process lifecycle: spawn, write input, collect output, exit.
use crate::config::SubagentConfig;
use crate::policy::truncate_at_newline;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

pub struct ProcessResult {
    pub exit_code: i32,
    pub raw_stdout: Vec<u8>,
    pub truncated: bool,
    /// Set when the worker failed at the process level (crash, signal, etc.)
    pub process_error: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum WorkerStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone)]
pub struct WorkerStreamChunk {
    pub stream: WorkerStream,
    pub chunk: String,
}

/// Spawn a configured subagent process, write `input` JSON to its stdin, collect stdout.
///
/// `cancel_rx`: if the sender fires, the process is sent SIGTERM then SIGKILL.
pub async fn run_subagent_process(
    config: &SubagentConfig,
    input: &serde_json::Value,
    stream_tx: Option<mpsc::Sender<WorkerStreamChunk>>,
    cancel_rx: oneshot::Receiver<()>,
    max_output_bytes: usize,
    grace_period_secs: u64,
) -> ProcessResult {
    let argv = match crate::policy::parse_command(&config.command) {
        Ok(v) => v,
        Err(e) => {
            return ProcessResult {
                exit_code: -1,
                raw_stdout: vec![],
                truncated: false,
                process_error: Some(format!("invalid command: {e}")),
            };
        }
    };

    let (program, args) = match argv.split_first() {
        Some(pair) => pair,
        None => {
            return ProcessResult {
                exit_code: -1,
                raw_stdout: vec![],
                truncated: false,
                process_error: Some("empty command".to_string()),
            };
        }
    };

    let mut child = match tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return ProcessResult {
                exit_code: -1,
                raw_stdout: vec![],
                truncated: false,
                process_error: Some(format!("failed to spawn process: {e}")),
            };
        }
    };

    // Write JSON input to stdin, then close (signals EOF to the worker).
    if let Some(mut stdin) = child.stdin.take() {
        let payload = match serde_json::to_vec(input) {
            Ok(b) => b,
            Err(e) => {
                let _ = child.kill().await;
                return ProcessResult {
                    exit_code: -1,
                    raw_stdout: vec![],
                    truncated: false,
                    process_error: Some(format!("cannot serialize input: {e}")),
                };
            }
        };
        // Best-effort: if write fails, worker will get EOF and may error on its own.
        let _ = stdin.write_all(&payload).await;
        let _ = stdin.write_all(b"\n").await;
        // stdin dropped here → EOF
    }

    let mut stdout_handle = child.stdout.take().expect("stdout piped");
    let mut stderr_handle = child.stderr.take().expect("stderr piped");

    // Read stdout/stderr concurrently and race against cancel signal.
    let read_future = async {
        let stdout_fut =
            read_stream_with_chunks(&mut stdout_handle, WorkerStream::Stdout, stream_tx.clone());
        let stderr_fut =
            read_stream_with_chunks(&mut stderr_handle, WorkerStream::Stderr, stream_tx);
        let (stdout, _stderr) = tokio::try_join!(stdout_fut, stderr_fut)?;
        Ok::<Vec<u8>, std::io::Error>(stdout)
    };

    tokio::select! {
        raw_result = read_future => {
            let raw = raw_result.unwrap_or_default();
            let status = child.wait().await;
            let exit_code = status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
            let (out, truncated) = truncate_at_newline(&raw, max_output_bytes);
            ProcessResult { exit_code, raw_stdout: out, truncated, process_error: None }
        }
        _ = cancel_rx => {
            terminate_process(&mut child, grace_period_secs).await;
            ProcessResult {
                exit_code: -1,
                raw_stdout: vec![],
                truncated: false,
                process_error: Some("cancelled".to_string()),
            }
        }
    }
}

async fn read_stream_with_chunks<R: AsyncRead + Unpin>(
    reader: &mut R,
    stream: WorkerStream,
    stream_tx: Option<mpsc::Sender<WorkerStreamChunk>>,
) -> std::io::Result<Vec<u8>> {
    let mut buf = [0u8; 4096];
    let mut collected = Vec::new();
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        collected.extend_from_slice(chunk);
        if let Some(tx) = &stream_tx {
            // Best-effort diagnostics channel for debug mode.
            let _ = tx.try_send(WorkerStreamChunk {
                stream,
                chunk: String::from_utf8_lossy(chunk).to_string(),
            });
        }
    }
    Ok(collected)
}

async fn terminate_process(child: &mut tokio::process::Child, grace_period_secs: u64) {
    // SIGTERM
    let _ = child.start_kill();
    let grace = Duration::from_secs(grace_period_secs);
    match tokio::time::timeout(grace, child.wait()).await {
        Ok(_) => {}
        Err(_) => {
            // Grace period expired: SIGKILL
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic_config(command: &str) -> SubagentConfig {
        SubagentConfig {
            command: command.to_string(),
            lifecycle: crate::config::Lifecycle::Task,
            volume: None,
        }
    }

    #[tokio::test]
    async fn test_worker_echo() {
        let config = basic_config("cat");
        let (_cancel_tx, cancel_rx) = oneshot::channel();
        let result = run_subagent_process(
            &config,
            &serde_json::json!({"prompt": "hello"}),
            None,
            cancel_rx,
            65536,
            10,
        )
        .await;
        assert_eq!(result.exit_code, 0);
        assert!(result.process_error.is_none());
        // cat echoes input back; should contain our JSON
        let out = String::from_utf8_lossy(&result.raw_stdout);
        assert!(out.contains("hello"));
    }

    #[tokio::test]
    async fn test_worker_cancel() {
        // sleep for 60s — cancel should interrupt it
        let config = basic_config("sleep 60");
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let handle = tokio::spawn(async move {
            run_subagent_process(&config, &serde_json::json!({}), None, cancel_rx, 65536, 1).await
        });
        // Give the process a moment to start
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = cancel_tx.send(());
        let result = handle.await.unwrap();
        assert!(result.process_error.is_some());
        assert_eq!(result.process_error.as_deref(), Some("cancelled"));
    }

    #[tokio::test]
    async fn test_worker_truncation() {
        // Generate output larger than max_output_bytes
        let config = basic_config("yes"); // outputs "y\n" forever
        let (_cancel_tx, cancel_rx) = oneshot::channel::<()>();
        // We need to let it run briefly then check; but `yes` runs forever.
        // Instead test truncation logic directly — see policy tests.
        // This test just verifies the worker exits cleanly with a small limit.
        drop(cancel_rx); // immediately cancel
        let (cancel_tx2, cancel_rx2) = oneshot::channel();
        let handle = tokio::spawn(async move {
            run_subagent_process(&config, &serde_json::json!({}), None, cancel_rx2, 10, 1).await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = cancel_tx2.send(());
        let result = handle.await.unwrap();
        assert!(result.process_error.is_some()); // cancelled
    }
}

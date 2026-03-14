/// Frontend trait and module re-exports.
/// The frontend renders events to the user and collects input.
/// Two implementations: TUI (default) and Headless (adapter event stream).
pub mod headless;

use crate::error::CoreError;
use crate::policy::gate::{ApprovalRequest, ApprovalUi};
use async_trait::async_trait;
use std::io::Write;

#[derive(Debug, Clone)]
pub struct UserMessage {
    pub text: String,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub enum FrontendEvent {
    UserMessage(UserMessage),
    SessionEnd,
    /// Runtime debug toggle request from interactive frontend.
    /// - `Some(true)`: force enable
    /// - `Some(false)`: force disable
    /// - `None`: toggle
    DebugMode {
        enabled: Option<bool>,
    },
}

/// Frontend interface for the loop runner to interact with the user.
#[async_trait]
pub trait Frontend: ApprovalUi + Send + Sync {
    /// Display a notify message (info/warning/error level).
    async fn render_notify(&self, text: &str, level: &str);

    /// Display a summary and indicate session completion.
    async fn render_complete(&self, summary: &str);

    /// Receive the next frontend event.
    /// Returns `None` when the input stream is closed (e.g. EOF).
    async fn next_event(&self) -> Option<FrontendEvent>;

    // @chunk frontend/worker-events
    // Worker lifecycle hooks. Default no-ops so existing impls need no changes.
    // HeadlessFrontend overrides these to emit WorkerStarted/WorkerFinished events
    // to the adapter. TUI and stdio impls use the defaults.

    /// Called after a spawn is acknowledged. In headless mode emits worker_started.
    async fn render_worker_started(&self, _spawn_id: &str, _subagent: &str) {}

    /// Called after a spawn result is delivered. In headless mode emits worker_finished.
    async fn render_worker_finished(&self, _spawn_id: &str, _exit_code: i32) {}

    /// Called when a worker writes a debug stream line.
    /// Headless can emit structured worker_stream events.
    async fn render_worker_stream(&self, spawn_id: &str, stream: &str, line: &str) {
        self.render_notify(
            &format!("[debug][worker:{spawn_id}][{stream}] {line}"),
            "info",
        )
        .await;
    }

    /// Called when the session encounters an unrecoverable error.
    async fn render_session_error(&self, _reason: &str) {}
    // @end-chunk
}

/// Simple stdin/stdout frontend used when there is no TTY.
/// Used as a fallback and in tests.
pub struct StdioFrontend;

#[async_trait]
impl ApprovalUi for StdioFrontend {
    async fn request_approval(&self, req: &ApprovalRequest) -> Result<String, CoreError> {
        println!("approval: {}", req.message);
        for (idx, option) in req.options.iter().enumerate() {
            println!("{}. {}", idx + 1, option);
        }
        print!("approval> ");
        std::io::stdout().flush().map_err(CoreError::Io)?;
        let mut buf = String::new();
        std::io::stdin()
            .read_line(&mut buf)
            .map_err(CoreError::Io)?;
        Ok(buf.trim().to_string())
    }

    async fn request_input(&self, message: &str) -> Result<String, CoreError> {
        println!("input: {message}");
        print!("input> ");
        std::io::stdout().flush().map_err(CoreError::Io)?;
        let mut buf = String::new();
        std::io::stdin()
            .read_line(&mut buf)
            .map_err(CoreError::Io)?;
        Ok(buf.trim().to_string())
    }
}

#[async_trait]
impl Frontend for StdioFrontend {
    async fn render_notify(&self, text: &str, level: &str) {
        println!("{level}: {text}");
    }

    async fn render_complete(&self, summary: &str) {
        println!("complete: {summary}");
    }

    async fn next_event(&self) -> Option<FrontendEvent> {
        let mut buf = String::new();
        match std::io::stdin().read_line(&mut buf) {
            Ok(0) => None,
            Ok(_) => Some(FrontendEvent::UserMessage(UserMessage {
                text: buf.trim().to_string(),
                metadata: None,
            })),
            Err(_) => None,
        }
    }
}

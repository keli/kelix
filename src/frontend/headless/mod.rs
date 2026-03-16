/// Headless frontend: adapter event stream over core's own stdin/stdout.
/// Implements ADAPTER_PROTOCOL — see docs/ADAPTER_PROTOCOL.md.
mod routing;
mod tasks;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use uuid::Uuid;

use async_trait::async_trait;

use crate::error::CoreError;
use crate::frontend::{FrontendEvent, UserMessage};
use crate::policy::gate::{ApprovalRequest, ApprovalUi};
use crate::protocol::adapter_msg::AdapterOutboundMessage;

use super::Frontend;

pub(crate) use routing::route_message;

fn new_evt_id() -> String {
    format!("evt-{}", Uuid::new_v4())
}

// @chunk headless/state
// Internal mutable state for the headless frontend, shared between the
// background stdin-reader task and the trait method implementations.
//
// Invariants:
// - At most one `event_waiter` is set at a time (the frontend pump is
//   single-threaded from loop_runner's perspective).
// - `pending_approvals` maps a request id to the oneshot that will receive
//   the operator's choice; multiple approval requests may be in-flight.
pub(crate) struct HeadlessState {
    /// Pending approval waiters keyed by the approval request id.
    pub(crate) pending_approvals: HashMap<String, oneshot::Sender<String>>,
    /// Pending free-form input waiter for a blocked request.
    pub(crate) pending_input: Option<oneshot::Sender<String>>,
    /// Parked waiter for the next adapter event (at most one).
    pub(crate) event_waiter: Option<oneshot::Sender<FrontendEvent>>,
    /// Buffered frontend events that arrived before `next_event` was called.
    pub(crate) event_queue: VecDeque<FrontendEvent>,
    /// Most recent routing metadata for this session.
    pub(crate) routing: Option<RoutingState>,
}
// @end-chunk

#[derive(Debug, Clone)]
pub(crate) struct RoutingState {
    pub(crate) channel_id: Option<String>,
}

// @chunk headless/frontend-struct
// HeadlessFrontend: zero-copy design — all I/O happens in background tasks;
// trait methods only manipulate channels and the shared state lock.
pub struct HeadlessFrontend {
    state: Arc<Mutex<HeadlessState>>,
    /// Sender for emitting outbound adapter messages to stdout.
    event_tx: mpsc::Sender<AdapterOutboundMessage>,
    session_id: String,
}
// @end-chunk

impl HeadlessFrontend {
    pub fn new(session_id: String) -> Self {
        let state = Arc::new(Mutex::new(HeadlessState {
            pending_approvals: HashMap::new(),
            pending_input: None,
            event_waiter: None,
            event_queue: VecDeque::new(),
            routing: None,
        }));

        let (event_tx, event_rx) = mpsc::channel::<AdapterOutboundMessage>(64);

        // @chunk headless/event-writer-task
        // Background task: serialize outbound messages to newline-delimited JSON
        // on tokio stdout. This is the only writer to stdout in headless mode.
        tokio::spawn(tasks::event_writer_task(event_rx));
        // @end-chunk

        // @chunk headless/stdin-reader-task
        // Background task: deserialize AdapterMessage from newline-delimited JSON
        // on tokio stdin and route each message to the appropriate waiter.
        let state_clone = Arc::clone(&state);
        let event_tx_clone = event_tx.clone();
        tokio::spawn(tasks::stdin_reader_task(state_clone, event_tx_clone));
        // @end-chunk

        Self {
            state,
            event_tx,
            session_id,
        }
    }

    async fn routing_channel(&self) -> Option<String> {
        self.state
            .lock()
            .await
            .routing
            .as_ref()
            .and_then(|routing| routing.channel_id.clone())
    }
}

impl Default for HeadlessFrontend {
    fn default() -> Self {
        Self::new("unknown-session".to_string())
    }
}

#[async_trait]
impl ApprovalUi for HeadlessFrontend {
    // @chunk headless/approval-waiter
    // Emits an approval_required event to the adapter and parks a oneshot
    // keyed by req.id. The stdin-reader fulfills it on ApprovalResponse.
    async fn request_approval(&self, req: &ApprovalRequest) -> Result<String, CoreError> {
        let (tx, rx) = oneshot::channel::<String>();
        {
            let mut s = self.state.lock().await;
            s.pending_approvals.insert(req.id.clone(), tx);
        }
        let kind_str = match req.kind {
            crate::protocol::core_msg::ApproveKind::Shell => "shell",
        };
        let event = AdapterOutboundMessage::ApprovalRequired {
            id: new_evt_id(),
            request_id: req.id.clone(),
            kind: kind_str.to_string(),
            message: req.message.clone(),
            options: req.options.clone(),
            session_id: self.session_id.clone(),
            channel_id: self.routing_channel().await,
        };
        let _ = self.event_tx.send(event).await;
        rx.await
            .map_err(|_| CoreError::InvalidRequest("approval channel closed".to_string()))
    }
    // @end-chunk

    // @chunk headless/user-input
    // Emits an agent_message event to the adapter and then waits for the
    // next user_message from the adapter stdin.
    async fn request_input(&self, message: &str) -> Result<String, CoreError> {
        let (tx, rx) = oneshot::channel::<String>();
        {
            let mut s = self.state.lock().await;
            s.pending_input = Some(tx);
        }
        let event = AdapterOutboundMessage::AgentMessage {
            id: new_evt_id(),
            text: message.to_string(),
            session_id: self.session_id.clone(),
            channel_id: self.routing_channel().await,
        };
        let _ = self.event_tx.send(event).await;
        rx.await
            .map_err(|_| CoreError::InvalidRequest("input stream closed".to_string()))
    }
    // @end-chunk
}

#[async_trait]
impl Frontend for HeadlessFrontend {
    async fn render_notify(&self, text: &str, level: &str) {
        let event = AdapterOutboundMessage::Notify {
            id: new_evt_id(),
            text: text.to_string(),
            level: level.to_string(),
            session_id: self.session_id.clone(),
            channel_id: self.routing_channel().await,
            event: None,
            spawn_id: None,
            subagent: None,
            exit_code: None,
            stream: None,
        };
        let _ = self.event_tx.send(event).await;
    }

    async fn render_complete(&self, summary: &str) {
        let event = AdapterOutboundMessage::SessionComplete {
            id: new_evt_id(),
            summary: summary.to_string(),
            session_id: self.session_id.clone(),
        };
        let _ = self.event_tx.send(event).await;
    }

    // @chunk headless/user-input
    // Dequeues a buffered frontend event if available; otherwise parks a
    // oneshot in event_waiter and awaits it. The stdin-reader task fulfills
    // the waiter when a matching AdapterMessage arrives.
    async fn next_event(&self) -> Option<FrontendEvent> {
        let (tx, rx) = oneshot::channel::<FrontendEvent>();
        {
            let mut s = self.state.lock().await;
            if let Some(event) = s.event_queue.pop_front() {
                return Some(event);
            }
            s.event_waiter = Some(tx);
        }
        rx.await.ok()
    }
    // @end-chunk

    // @chunk headless/worker-events
    // Emit worker lifecycle events as notify payloads so the adapter can
    // account for resources without needing a second event shape.
    async fn render_worker_started(&self, spawn_id: &str, subagent: &str) {
        let event = AdapterOutboundMessage::Notify {
            id: new_evt_id(),
            text: format!("Worker started: {subagent} ({spawn_id})"),
            level: "info".to_string(),
            session_id: self.session_id.clone(),
            channel_id: self.routing_channel().await,
            event: Some("worker_started".to_string()),
            spawn_id: Some(spawn_id.to_string()),
            subagent: Some(subagent.to_string()),
            exit_code: None,
            stream: None,
        };
        let _ = self.event_tx.send(event).await;
    }

    async fn render_worker_finished(&self, spawn_id: &str, exit_code: i32) {
        let event = AdapterOutboundMessage::Notify {
            id: new_evt_id(),
            text: format!("Worker finished: {spawn_id}, exit_code={exit_code}"),
            level: "info".to_string(),
            session_id: self.session_id.clone(),
            channel_id: self.routing_channel().await,
            event: Some("worker_finished".to_string()),
            spawn_id: Some(spawn_id.to_string()),
            subagent: None,
            exit_code: Some(exit_code),
            stream: None,
        };
        let _ = self.event_tx.send(event).await;
    }

    async fn render_worker_stream(&self, spawn_id: &str, stream: &str, line: &str) {
        let event = AdapterOutboundMessage::Notify {
            id: new_evt_id(),
            text: line.to_string(),
            level: "info".to_string(),
            session_id: self.session_id.clone(),
            channel_id: self.routing_channel().await,
            event: Some("worker_stream".to_string()),
            spawn_id: Some(spawn_id.to_string()),
            subagent: None,
            exit_code: None,
            stream: Some(stream.to_string()),
        };
        let _ = self.event_tx.send(event).await;
    }

    async fn render_session_error(&self, reason: &str) {
        let event = AdapterOutboundMessage::SessionError {
            id: new_evt_id(),
            reason: reason.to_string(),
            session_id: self.session_id.clone(),
        };
        let _ = self.event_tx.send(event).await;
    }
    // @end-chunk
}

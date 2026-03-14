use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use crate::frontend::{FrontendEvent, UserMessage};
use crate::protocol::adapter_msg::{AdapterMessage, AdapterOutboundMessage};

use super::{HeadlessState, RoutingState};

// @chunk headless/route-message
// Routes a parsed AdapterMessage to the correct waiter.
//
// - UserMessage: fulfills a parked event_waiter, or queues the event.
// - ApprovalResponse: fulfills the matching pending_approval oneshot.
// - SessionEnd: yields a SessionEnd control event to the loop runner.
pub async fn route_message(
    msg: AdapterMessage,
    state: &Arc<Mutex<HeadlessState>>,
    event_tx: &mpsc::Sender<AdapterOutboundMessage>,
) {
    match msg {
        AdapterMessage::UserMessage {
            id,
            text,
            sender_id,
            channel_id,
        } => {
            let mut s = state.lock().await;
            s.routing = Some(RoutingState {
                channel_id: channel_id.clone(),
            });

            let metadata = serde_json::json!({
                "sender_id": sender_id,
                "channel_id": channel_id,
            });
            if let Some(waiter) = s.pending_input.take() {
                let _ = waiter.send(text);
            } else {
                let event = FrontendEvent::UserMessage(UserMessage {
                    text,
                    metadata: Some(metadata),
                });
                if let Some(waiter) = s.event_waiter.take() {
                    let _ = waiter.send(event);
                } else {
                    s.event_queue.push_back(event);
                }
            }
            drop(s);

            let _ = event_tx
                .send(AdapterOutboundMessage::UserMessageAck { id })
                .await;
        }
        AdapterMessage::ApprovalResponse {
            id,
            request_id,
            choice,
        } => {
            let mut s = state.lock().await;
            if let Some(tx) = s.pending_approvals.remove(&request_id) {
                drop(s);
                let _ = tx.send(choice);
                let _ = event_tx
                    .send(AdapterOutboundMessage::ApprovalResponseAck { id })
                    .await;
            } else {
                drop(s);
                let _ = event_tx
                    .send(AdapterOutboundMessage::Error {
                        id,
                        code: "invalid_request".to_string(),
                        message: format!("no pending approval for request_id '{request_id}'"),
                    })
                    .await;
            }
        }
        AdapterMessage::DebugMode { enabled, .. } => {
            let mut s = state.lock().await;
            let event = FrontendEvent::DebugMode { enabled };
            if let Some(waiter) = s.event_waiter.take() {
                let _ = waiter.send(event);
            } else {
                s.event_queue.push_back(event);
            }
        }
        AdapterMessage::SessionEnd { .. } => {
            let mut s = state.lock().await;
            let event = FrontendEvent::SessionEnd;
            if let Some(waiter) = s.event_waiter.take() {
                let _ = waiter.send(event);
            } else {
                s.event_queue.push_back(event);
            }
        }
    }
}
// @end-chunk

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::adapter_msg::AdapterOutboundMessage;
    use std::collections::{HashMap, VecDeque};
    use tokio::sync::oneshot;

    fn make_state() -> Arc<Mutex<HeadlessState>> {
        Arc::new(Mutex::new(HeadlessState {
            pending_approvals: HashMap::new(),
            pending_input: None,
            event_waiter: None,
            event_queue: VecDeque::new(),
            routing: None,
        }))
    }

    #[tokio::test]
    async fn test_user_message_queues_when_no_waiter() {
        let state = make_state();
        let (tx, mut rx) = mpsc::channel::<AdapterOutboundMessage>(8);
        let msg = AdapterMessage::UserMessage {
            id: "m1".to_string(),
            text: "hello".to_string(),
            sender_id: Some("user-1".to_string()),
            channel_id: Some("chat-1".to_string()),
        };
        route_message(msg, &state, &tx).await;

        let s = state.lock().await;
        assert_eq!(s.event_queue.len(), 1);
        match &s.event_queue[0] {
            FrontendEvent::UserMessage(message) => assert_eq!(message.text, "hello"),
            FrontendEvent::SessionEnd => panic!("expected user message"),
            FrontendEvent::DebugMode { .. } => panic!("expected user message"),
        }
        drop(s);

        match rx.recv().await.unwrap() {
            AdapterOutboundMessage::UserMessageAck { id } => assert_eq!(id, "m1"),
            other => panic!("unexpected outbound message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_user_message_fulfills_waiter() {
        let state = make_state();
        let (tx, _rx) = mpsc::channel::<AdapterOutboundMessage>(8);
        let (waiter_tx, waiter_rx) = oneshot::channel::<FrontendEvent>();
        {
            let mut s = state.lock().await;
            s.event_waiter = Some(waiter_tx);
        }
        let msg = AdapterMessage::UserMessage {
            id: "m2".to_string(),
            text: "world".to_string(),
            sender_id: None,
            channel_id: Some("chat-1".to_string()),
        };
        route_message(msg, &state, &tx).await;

        let result = waiter_rx.await.unwrap();
        match result {
            FrontendEvent::UserMessage(message) => assert_eq!(message.text, "world"),
            FrontendEvent::SessionEnd => panic!("expected user message"),
            FrontendEvent::DebugMode { .. } => panic!("expected user message"),
        }
        let s = state.lock().await;
        assert!(s.event_queue.is_empty());
    }

    #[tokio::test]
    async fn test_approval_response_routes_to_correct_waiter() {
        let state = make_state();
        let (tx, mut rx) = mpsc::channel::<AdapterOutboundMessage>(8);
        let (approval_tx, approval_rx) = oneshot::channel::<String>();
        {
            let mut s = state.lock().await;
            s.pending_approvals
                .insert("req-001".to_string(), approval_tx);
        }
        let msg = AdapterMessage::ApprovalResponse {
            id: "r1".to_string(),
            request_id: "req-001".to_string(),
            choice: "yes".to_string(),
        };
        route_message(msg, &state, &tx).await;

        let result = approval_rx.await.unwrap();
        assert_eq!(result, "yes");
        match rx.recv().await.unwrap() {
            AdapterOutboundMessage::ApprovalResponseAck { id } => assert_eq!(id, "r1"),
            other => panic!("unexpected outbound message: {other:?}"),
        }
        let s = state.lock().await;
        assert!(s.pending_approvals.is_empty());
    }

    #[tokio::test]
    async fn test_approval_response_unknown_id_returns_error() {
        let state = make_state();
        let (tx, mut rx) = mpsc::channel::<AdapterOutboundMessage>(8);
        let msg = AdapterMessage::ApprovalResponse {
            id: "r2".to_string(),
            request_id: "nonexistent".to_string(),
            choice: "yes".to_string(),
        };
        route_message(msg, &state, &tx).await;

        match rx.recv().await.unwrap() {
            AdapterOutboundMessage::Error { id, code, .. } => {
                assert_eq!(id, "r2");
                assert_eq!(code, "invalid_request");
            }
            other => panic!("unexpected outbound message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_debug_mode_queues_event() {
        let state = make_state();
        let (tx, _rx) = mpsc::channel::<AdapterOutboundMessage>(8);
        let msg = AdapterMessage::DebugMode {
            id: "dbg-1".to_string(),
            enabled: Some(true),
        };
        route_message(msg, &state, &tx).await;

        let mut s = state.lock().await;
        match s.event_queue.pop_front() {
            Some(FrontendEvent::DebugMode { enabled }) => assert_eq!(enabled, Some(true)),
            other => panic!("expected debug mode event, got {other:?}"),
        }
    }
}

/// ADAPTER_PROTOCOL message types.
/// See docs/ADAPTER_PROTOCOL.md for the full specification.
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Adapter → Core (in headless mode, over core's own stdin)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdapterMessage {
    UserMessage {
        id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel_id: Option<String>,
    },
    ApprovalResponse {
        id: String,
        request_id: String,
        choice: String,
    },
    DebugMode {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
    },
    SessionEnd {
        id: String,
    },
}

// ---------------------------------------------------------------------------
// Core → Adapter (in headless mode, over core's own stdout)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdapterOutboundMessage {
    UserMessageAck {
        id: String,
    },
    ApprovalResponseAck {
        id: String,
    },
    AgentMessage {
        id: String,
        text: String,
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        channel_id: Option<String>,
    },
    Notify {
        id: String,
        text: String,
        level: String,
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        channel_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        event: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        spawn_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        subagent: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stream: Option<String>,
    },
    ApprovalRequired {
        id: String,
        request_id: String,
        kind: String,
        message: String,
        options: Vec<String>,
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        channel_id: Option<String>,
    },
    SessionComplete {
        id: String,
        summary: String,
        session_id: String,
    },
    SessionError {
        id: String,
        reason: String,
        session_id: String,
    },
    Error {
        id: String,
        code: String,
        message: String,
    },
}

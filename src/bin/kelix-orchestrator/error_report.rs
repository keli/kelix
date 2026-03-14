use serde::Serialize;

pub const ORCHESTRATOR_ERROR_PREFIX: &str = "KELIX_ORCH_ERROR ";

#[derive(Debug, Clone, Copy)]
pub enum OrchestratorErrorCategory {
    Runtime,
}

impl OrchestratorErrorCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
        }
    }
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    category: &'a str,
    code: &'a str,
    message: &'a str,
}

// @chunk orchestrator/error-envelope
// Emit a single machine-readable stderr marker that core can parse reliably.
// Keep a plain-text line after the marker for terminal operators.
pub fn emit_orchestrator_error(
    category: OrchestratorErrorCategory,
    code: &'static str,
    message: &str,
) {
    let envelope = ErrorEnvelope {
        category: category.as_str(),
        code,
        message,
    };
    if let Ok(json) = serde_json::to_string(&envelope) {
        eprintln!("{ORCHESTRATOR_ERROR_PREFIX}{json}");
    }
    eprintln!("{message}");
}
// @end-chunk

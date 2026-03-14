use thiserror::Error;

#[derive(Error, Debug)]
pub enum CoreError {
    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("orchestrator exited unexpectedly: {0}")]
    OrchestratorExit(String),

    #[error("spawn limit exceeded")]
    SpawnLimitExceeded,

    #[error("budget exceeded")]
    BudgetExceeded,

    #[error("unknown subagent: {0}")]
    UnknownSubagent(String),

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("unknown spawn id: {0}")]
    UnknownSpawnId(String),

    #[error("invalid command: {0}")]
    InvalidCommand(String),

    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[error("session already complete: {0}")]
    SessionAlreadyComplete(String),
}

// @chunk orchestrator/session-state
// Persist and load backend session IDs (e.g. Claude --resume ID, Codex thread
// ID) across orchestrator process restarts.
//
// State files live in `.kelix/` relative to the current working directory
// (the session workspace), which is mounted into the orchestrator container
// and survives process restarts. One file per session per backend:
//   .kelix/{kelix_session_id}.{backend}-session
//
// Writes are best-effort: a failure to persist is logged but does not abort
// the orchestrator, since each restart will simply begin a new backend session
// (degraded, but not broken).
use std::path::{Path, PathBuf};

fn state_dir() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".kelix")
}

fn session_id_path_in(base: &Path, kelix_session_id: &str, backend: &str) -> PathBuf {
    base.join(format!("{kelix_session_id}.{backend}-session"))
}

/// Load a persisted backend session ID from the workspace state directory.
/// Returns `None` if no state file exists for this kelix session and backend.
pub fn load_backend_session_id(kelix_session_id: &str, backend: &str) -> Option<String> {
    load_backend_session_id_from(&state_dir(), kelix_session_id, backend)
}

fn load_backend_session_id_from(
    base: &Path,
    kelix_session_id: &str,
    backend: &str,
) -> Option<String> {
    let path = session_id_path_in(base, kelix_session_id, backend);
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Persist a backend session ID so it can be restored on the next restart.
/// Silently ignores I/O failures (degrades to a fresh session on next start).
pub fn save_backend_session_id(kelix_session_id: &str, backend: &str, backend_session_id: &str) {
    save_backend_session_id_to(&state_dir(), kelix_session_id, backend, backend_session_id);
}

fn save_backend_session_id_to(
    base: &Path,
    kelix_session_id: &str,
    backend: &str,
    backend_session_id: &str,
) {
    if std::fs::create_dir_all(base).is_err() {
        return;
    }
    let path = session_id_path_in(base, kelix_session_id, backend);
    if let Err(e) = std::fs::write(&path, backend_session_id) {
        eprintln!(
            "warn: failed to persist backend session id to {}: {e}",
            path.display()
        );
    }
}

/// Extract the kelix `session_id` from a `session_start` JSON line.
/// Returns `None` if the line is not a valid `session_start` message.
pub fn extract_kelix_session_id(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "session_start" {
        return None;
    }
    v.get("session_id")?.as_str().map(str::to_string)
}
// @end-chunk

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_kelix_session_id_from_session_start() {
        let line = r#"{"type":"session_start","id":"init-1","prompt":"","recovery":false,"session_id":"sess-abc123","config":{"subagents":[],"max_spawns":0,"max_concurrent_spawns":0,"max_wall_time_secs":0,"protocol":{"request_types":[],"request_fields":{},"instructions":[]}}}"#;
        assert_eq!(
            extract_kelix_session_id(line),
            Some("sess-abc123".to_string())
        );
    }

    #[test]
    fn test_extract_kelix_session_id_recovery_true() {
        let line = r#"{"type":"session_start","id":"init-2","prompt":"","recovery":true,"session_id":"sess-xyz789","config":{"subagents":[],"max_spawns":0,"max_concurrent_spawns":0,"max_wall_time_secs":0,"protocol":{"request_types":[],"request_fields":{},"instructions":[]}}}"#;
        assert_eq!(
            extract_kelix_session_id(line),
            Some("sess-xyz789".to_string())
        );
    }

    #[test]
    fn test_extract_kelix_session_id_wrong_type() {
        let line = r#"{"type":"spawn_result","session_id":"sess-abc123"}"#;
        assert_eq!(extract_kelix_session_id(line), None);
    }

    #[test]
    fn test_extract_kelix_session_id_not_json() {
        assert_eq!(extract_kelix_session_id("not json"), None);
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        save_backend_session_id_to(dir.path(), "sess-test-001", "claude", "cl-session-xyz");
        let loaded = load_backend_session_id_from(dir.path(), "sess-test-001", "claude");
        assert_eq!(loaded, Some("cl-session-xyz".to_string()));
    }

    #[test]
    fn test_load_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load_backend_session_id_from(dir.path(), "sess-nonexistent-xyz", "claude"),
            None
        );
    }

    #[test]
    fn test_different_backends_are_independent() {
        let dir = tempfile::tempdir().unwrap();
        save_backend_session_id_to(dir.path(), "sess-multi-001", "claude", "claude-id-x");
        save_backend_session_id_to(dir.path(), "sess-multi-001", "codex", "codex-id-y");
        assert_eq!(
            load_backend_session_id_from(dir.path(), "sess-multi-001", "claude"),
            Some("claude-id-x".to_string())
        );
        assert_eq!(
            load_backend_session_id_from(dir.path(), "sess-multi-001", "codex"),
            Some("codex-id-y".to_string())
        );
    }

    #[test]
    fn test_overwrite_existing_session_id() {
        let dir = tempfile::tempdir().unwrap();
        save_backend_session_id_to(dir.path(), "sess-update-001", "claude", "old-id");
        save_backend_session_id_to(dir.path(), "sess-update-001", "claude", "new-id");
        let loaded = load_backend_session_id_from(dir.path(), "sess-update-001", "claude");
        assert_eq!(loaded, Some("new-id".to_string()));
    }
}

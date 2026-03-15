// @chunk orchestrator/turn-logger
// Appends raw agent turn output (stdout + stderr) to an optional log file.
// All writes are best-effort; failures are silently ignored to avoid
// disrupting the orchestrator session.
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;

/// Returns the `Stdio` to use for an agent's stderr.
///
/// Always inherit so stderr passes through to the caller in real time,
/// which is visible in debug output regardless of whether a log file is set.
pub fn stderr_stdio(_log_file: Option<&Path>) -> Stdio {
    Stdio::inherit()
}

pub struct TurnLog<'a> {
    pub backend: &'a str,
    pub session_id: Option<&'a str>,
    pub stdout: &'a [u8],
}

pub fn append_turn_log(path: &Path, entry: &TurnLog<'_>) {
    let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(
        f,
        "=== turn backend={} session={} ===",
        entry.backend,
        entry.session_id.unwrap_or("new")
    );
    if !entry.stdout.is_empty() {
        let _ = f.write_all(entry.stdout);
        if !entry.stdout.ends_with(b"\n") {
            let _ = writeln!(f);
        }
    }
}
// @end-chunk

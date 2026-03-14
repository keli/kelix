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
/// With a log file: pipe stderr so it can be written to the file.
/// Without a log file: inherit so it passes through to the caller's stderr,
/// which is visible in debug output.
pub fn stderr_stdio(log_file: Option<&Path>) -> Stdio {
    if log_file.is_some() {
        Stdio::piped()
    } else {
        Stdio::inherit()
    }
}

pub struct TurnLog<'a> {
    pub backend: &'a str,
    pub session_id: Option<&'a str>,
    pub stdout: &'a [u8],
    pub stderr: &'a [u8],
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
        let _ = writeln!(f, "--- stdout ---");
        let _ = f.write_all(entry.stdout);
        if !entry.stdout.ends_with(b"\n") {
            let _ = writeln!(f);
        }
    }
    if !entry.stderr.is_empty() {
        let _ = writeln!(f, "--- stderr ---");
        let _ = f.write_all(entry.stderr);
        if !entry.stderr.ends_with(b"\n") {
            let _ = writeln!(f);
        }
    }
}
// @end-chunk

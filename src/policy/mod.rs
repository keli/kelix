mod expand;
/// Shell policy gate.
/// Procedure: enabled check → allowlist check → exec → truncate output.
/// See DESIGN.md §4 "Shell policy gate".
pub mod gate;

use crate::config::ShellConfig;
use crate::error::CoreError;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use expand::{expand_argv_home, expand_env_vars};

#[derive(Debug)]
pub struct ShellResult {
    pub stdout: Vec<u8>,
    pub exit_code: i32,
    pub truncated: bool,
}

/// Execute a command through the shell policy gate.
///
/// `argv` must be a non-empty slice; argv[0] is the program, the rest are args.
/// Returns `CoreError` if the command is not allowed or the policy is disabled.
pub async fn execute(config: &ShellConfig, argv: &[String]) -> Result<ShellResult, CoreError> {
    if !config.enabled {
        return Err(CoreError::InvalidRequest(
            "shell execution is disabled".to_string(),
        ));
    }

    let program = argv
        .first()
        .ok_or_else(|| CoreError::InvalidCommand("empty command".to_string()))?;

    if !config.allowed_commands.iter().any(|a| a == program) {
        return Err(CoreError::InvalidCommand(format!(
            "command '{}' is not in allowed_commands",
            program
        )));
    }

    let mut child = Command::new(program)
        .args(&argv[1..])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(CoreError::Io)?;

    let mut stdout_handle = child.stdout.take().expect("stdout piped");

    let timeout = Duration::from_secs(config.timeout_secs);
    let max_bytes = config.max_output_bytes;

    let read_result = tokio::time::timeout(timeout, async {
        let mut buf = Vec::new();
        stdout_handle.read_to_end(&mut buf).await?;
        Ok::<Vec<u8>, std::io::Error>(buf)
    })
    .await;

    let raw = match read_result {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(e)) => return Err(CoreError::Io(e)),
        Err(_) => {
            // Timeout: kill the process and return what we have so far
            let _ = child.kill().await;
            Vec::new()
        }
    };

    let status = child.wait().await.map_err(CoreError::Io)?;
    let exit_code = status.code().unwrap_or(-1);

    let (truncated_bytes, truncated) = truncate_at_newline(&raw, max_bytes);

    Ok(ShellResult {
        stdout: truncated_bytes,
        exit_code,
        truncated,
    })
}

/// Parse a command string into argv using shell-like tokenization.
pub fn parse_command(command: &str) -> Result<Vec<String>, CoreError> {
    let argv = shlex::split(command)
        .ok_or_else(|| CoreError::InvalidCommand(format!("cannot parse command: {command}")))?;
    expand_argv_home(&argv)
}

/// Truncate `bytes` at `max_bytes`, walking backward to the nearest `\n`.
/// Returns `(truncated_bytes, was_truncated)`.
pub fn truncate_at_newline(bytes: &[u8], max_bytes: usize) -> (Vec<u8>, bool) {
    if bytes.len() <= max_bytes {
        return (bytes.to_vec(), false);
    }

    // Walk backward from max_bytes to find a newline boundary.
    let cut = bytes[..max_bytes]
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|i| i + 1) // include the newline itself
        .unwrap_or_else(|| {
            // No newline: find last valid UTF-8 char boundary.
            let mut i = max_bytes;
            while i > 0 && (bytes[i] & 0xC0) == 0x80 {
                i -= 1;
            }
            i
        });

    (bytes[..cut].to_vec(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn shell_config(allowed: &[&str]) -> ShellConfig {
        ShellConfig {
            enabled: true,
            timeout_secs: 5,
            max_output_bytes: 65536,
            allowed_commands: allowed.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[tokio::test]
    async fn test_allowed_command_runs() {
        let config = shell_config(&["echo"]);
        let result = execute(&config, &["echo".to_string(), "hello".to_string()])
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim_ascii_end(), b"hello");
        assert!(!result.truncated);
    }

    #[tokio::test]
    async fn test_disallowed_command_rejected() {
        let config = shell_config(&["echo"]);
        let err = execute(&config, &["ls".to_string()]).await.unwrap_err();
        assert!(matches!(err, CoreError::InvalidCommand(_)));
    }

    #[tokio::test]
    async fn test_disabled_policy_rejects_all() {
        let mut config = shell_config(&["echo"]);
        config.enabled = false;
        let err = execute(&config, &["echo".to_string()]).await.unwrap_err();
        assert!(matches!(err, CoreError::InvalidRequest(_)));
    }

    #[test]
    fn test_truncate_at_newline_no_truncation_needed() {
        let data = b"hello\nworld\n";
        let (out, truncated) = truncate_at_newline(data, 100);
        assert_eq!(out, data);
        assert!(!truncated);
    }

    #[test]
    fn test_truncate_at_newline_cuts_at_newline() {
        let data = b"line1\nline2\nline3\n";
        // max_bytes = 12 → cuts after "line1\nline2\n" (12 bytes)
        let (out, truncated) = truncate_at_newline(data, 12);
        assert_eq!(out, b"line1\nline2\n");
        assert!(truncated);
    }

    #[test]
    fn test_truncate_no_newline_uses_byte_boundary() {
        let data = b"abcdefghij";
        let (out, truncated) = truncate_at_newline(data, 5);
        assert_eq!(out.len(), 5);
        assert!(truncated);
    }

    #[test]
    fn test_parse_command_splits_correctly() {
        let args = parse_command("podman run --rm -i my-image").unwrap();
        assert_eq!(args, &["podman", "run", "--rm", "-i", "my-image"]);
    }

    #[test]
    fn test_parse_command_handles_quoted_args() {
        let args = parse_command(r#"echo "hello world""#).unwrap();
        assert_eq!(args, &["echo", "hello world"]);
    }

    #[test]
    fn test_parse_command_expands_home_variable() {
        let home = env::var("HOME").expect("HOME should be set for tests");
        let args = parse_command("podman run -v $HOME/.codex:/auth:ro").unwrap();
        assert_eq!(
            args,
            vec![
                "podman".to_string(),
                "run".to_string(),
                "-v".to_string(),
                format!("{home}/.codex:/auth:ro"),
            ]
        );
    }

    #[test]
    fn test_parse_command_expands_braced_home_variable() {
        let home = env::var("HOME").expect("HOME should be set for tests");
        let args = parse_command("podman run -v ${HOME}/.codex:/auth:ro").unwrap();
        assert_eq!(
            args,
            vec![
                "podman".to_string(),
                "run".to_string(),
                "-v".to_string(),
                format!("{home}/.codex:/auth:ro"),
            ]
        );
    }

    #[test]
    fn test_parse_command_expands_tilde_prefix() {
        let home = env::var("HOME").expect("HOME should be set for tests");
        let args = parse_command("podman run -v ~/.codex:/auth:ro").unwrap();
        assert_eq!(
            args,
            vec![
                "podman".to_string(),
                "run".to_string(),
                "-v".to_string(),
                format!("{home}/.codex:/auth:ro"),
            ]
        );
    }

    #[test]
    fn test_parse_command_expands_kelix_home_from_env() {
        env::set_var("KELIX_HOME", "/opt/kelix");
        let args = parse_command("podman run -v $KELIX_HOME/prompts:/prompts:ro").unwrap();
        assert_eq!(
            args,
            vec![
                "podman".to_string(),
                "run".to_string(),
                "-v".to_string(),
                "/opt/kelix/prompts:/prompts:ro".to_string(),
            ]
        );
    }

    #[test]
    fn test_parse_command_expands_braced_kelix_home_from_env() {
        env::set_var("KELIX_HOME", "/opt/kelix");
        let args = parse_command("podman run -v ${KELIX_HOME}/prompts:/prompts:ro").unwrap();
        assert_eq!(
            args,
            vec![
                "podman".to_string(),
                "run".to_string(),
                "-v".to_string(),
                "/opt/kelix/prompts:/prompts:ro".to_string(),
            ]
        );
    }

    #[test]
    fn test_expand_env_vars_bare() {
        env::set_var("TEST_TOKEN_KELIX", "tok123");
        assert_eq!(expand_env_vars("--env $TEST_TOKEN_KELIX"), "--env tok123");
    }

    #[test]
    fn test_expand_env_vars_braced() {
        env::set_var("TEST_TOKEN_KELIX", "tok123");
        assert_eq!(
            expand_env_vars("prefix_${TEST_TOKEN_KELIX}_suffix"),
            "prefix_tok123_suffix"
        );
    }

    #[test]
    fn test_expand_env_vars_unset_is_empty() {
        env::remove_var("KELIX_DEFINITELY_UNSET");
        assert_eq!(expand_env_vars("--env $KELIX_DEFINITELY_UNSET"), "--env ");
    }

    #[test]
    fn test_expand_env_vars_bare_dollar_preserved() {
        assert_eq!(expand_env_vars("cost is $5"), "cost is $5");
    }

    #[test]
    fn test_parse_command_expands_arbitrary_env_var() {
        env::set_var("MY_API_KEY_KELIX", "secret");
        let args = parse_command("podman run --env MY_API_KEY=$MY_API_KEY_KELIX image").unwrap();
        assert_eq!(
            args,
            vec!["podman", "run", "--env", "MY_API_KEY=secret", "image"]
        );
    }
}

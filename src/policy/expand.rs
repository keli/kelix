// @chunk policy/env-expansion
// Expand `$VAR` and `${VAR}` references in argv without invoking a shell.
// No other shell semantics (command substitution, globbing, pipelines) are supported.
//
// Special cases:
// - `$HOME` / `${HOME}` / `~` / `~/...`: tilde expansion using the HOME env var.
// - `$KELIX_HOME` / `${KELIX_HOME}`: if unset, falls back to executable-relative
//   bundle/package candidates and prefers directories containing bundled assets.
// - All other variables: looked up from the process environment; unset variables
//   expand to an empty string.

use std::env;

use crate::error::CoreError;
use crate::paths::resolve_kelix_home_path;

pub(super) fn expand_argv_home(argv: &[String]) -> Result<Vec<String>, CoreError> {
    let needs_home = argv.iter().any(|arg| {
        arg.contains("$HOME") || arg.contains("${HOME}") || arg == "~" || arg.starts_with("~/")
    });
    let needs_kelix_home = argv
        .iter()
        .any(|arg| arg.contains("$KELIX_HOME") || arg.contains("${KELIX_HOME}"));

    let home = if needs_home {
        Some(env::var("HOME").map_err(|_| {
            CoreError::InvalidCommand("HOME is not set for command expansion".to_string())
        })?)
    } else {
        None
    };

    let kelix_home = if needs_kelix_home {
        Some(resolve_kelix_home()?)
    } else {
        None
    };

    argv.iter()
        .map(|arg| {
            let mut s = arg.clone();
            if let Some(ref h) = home {
                s = expand_home_token(&s, h);
            }
            if let Some(ref k) = kelix_home {
                s = s.replace("${KELIX_HOME}", k).replace("$KELIX_HOME", k);
            }
            s = expand_env_vars(&s);
            Ok(s)
        })
        .collect()
}

/// Expand remaining `${VAR}` and `$VAR` references using the process environment.
/// Unset variables expand to an empty string.
pub(super) fn expand_env_vars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }
        i += 1; // skip '$'
        if i < bytes.len() && bytes[i] == b'{' {
            i += 1; // skip '{'
            let start = i;
            while i < bytes.len() && bytes[i] != b'}' {
                i += 1;
            }
            let name = &s[start..i];
            if i < bytes.len() {
                i += 1; // skip '}'
            }
            out.push_str(&env::var(name).unwrap_or_default());
        } else if i < bytes.len() && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let name = &s[start..i];
            out.push_str(&env::var(name).unwrap_or_default());
        } else {
            out.push('$');
        }
    }
    out
}

fn resolve_kelix_home() -> Result<String, CoreError> {
    resolve_kelix_home_path()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(CoreError::InvalidCommand)
}

fn expand_home_token(arg: &str, home: &str) -> String {
    let mut expanded = arg.replace("${HOME}", home).replace("$HOME", home);
    if expanded == "~" {
        return home.to_string();
    }
    if let Some(rest) = expanded.strip_prefix("~/") {
        expanded = format!("{home}/{rest}");
    }
    expanded
}
// @end-chunk

use serde_json::json;
use std::path::Path;
use uuid::Uuid;

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

pub fn shutdown_msg() -> String {
    json!({
        "id": new_id("shutdown"),
        "type": "shutdown",
    })
    .to_string()
}

pub fn session_resume_msg(session: &str, force: bool) -> String {
    json!({
        "id": new_id("resume"),
        "type": "session_resume",
        "session_id": session,
        "force": force,
    })
    .to_string()
}

pub fn session_init_msg(
    session: &str,
    config: &Path,
    working_dir: &Path,
    enabled_subagents: &[String],
    initial_prompt: Option<&str>,
) -> String {
    json!({
        "id": new_id("init"),
        "type": "session_init",
        "session_id": session,
        "config": config,
        "working_dir": working_dir,
        "enabled_subagents": enabled_subagents,
        "initial_prompt": initial_prompt,
    })
    .to_string()
}

pub fn user_message_msg(session: &str, sender_id: &str, text: &str) -> String {
    json!({
        "id": new_id("msg"),
        "type": "user_message",
        "text": text,
        "session_id": session,
        "sender_id": sender_id,
    })
    .to_string()
}

pub fn approval_response_msg(session: &str, request_id: &str, choice: &str) -> String {
    json!({
        "id": new_id("approve"),
        "type": "approval_response",
        "request_id": request_id,
        "choice": choice,
        "session_id": session,
    })
    .to_string()
}

pub fn session_end_msg(session: &str) -> String {
    json!({
        "id": new_id("end"),
        "type": "session_end",
        "session_id": session,
    })
    .to_string()
}

pub fn debug_mode_msg(session: &str, enabled: Option<bool>) -> String {
    json!({
        "id": new_id("debug"),
        "type": "debug_mode",
        "enabled": enabled,
        "session_id": session,
    })
    .to_string()
}

pub fn parse_debug_arg(arg: &str) -> Option<bool> {
    match arg.trim().to_ascii_lowercase().as_str() {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

pub fn select_approval_choice(input: &str, options: &[String]) -> Option<String> {
    if options.is_empty() {
        return None;
    }

    if let Ok(idx) = input.parse::<usize>() {
        if (1..=options.len()).contains(&idx) {
            return Some(options[idx - 1].clone());
        }
    }

    options
        .iter()
        .find(|opt| opt.eq_ignore_ascii_case(input))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_approval_choice_by_index() {
        let options = vec!["yes".to_string(), "no".to_string()];
        assert_eq!(
            select_approval_choice("1", &options),
            Some("yes".to_string())
        );
        assert_eq!(select_approval_choice("3", &options), None);
    }

    #[test]
    fn test_select_approval_choice_by_text_case_insensitive() {
        let options = vec!["Approve".to_string(), "Reject".to_string()];
        assert_eq!(
            select_approval_choice("approve", &options),
            Some("Approve".to_string())
        );
    }

    #[test]
    fn test_parse_debug_arg() {
        assert_eq!(parse_debug_arg("on"), Some(true));
        assert_eq!(parse_debug_arg("off"), Some(false));
        assert_eq!(parse_debug_arg("maybe"), None);
    }
}

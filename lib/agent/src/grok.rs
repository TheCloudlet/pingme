use std::{env, fs};

use pingme_session::AgentSession;
use pingme_support::shell::shell_quote;
use serde_json::Value;

pub(crate) fn grok_command(
    session: &AgentSession,
    notify_socket_path: Option<&str>,
    notify_token: Option<&str>,
) -> String {
    match (
        session.thread_ts.as_deref(),
        notify_socket_path,
        notify_token,
    ) {
        (Some(thread_ts), Some(socket_path), Some(token)) => format!(
            "exec env PINGME_SESSION_NAME={} PINGME_THREAD_TS={} PINGME_NOTIFY_SOCKET={} PINGME_NOTIFY_TOKEN={} PINGME_EXECUTABLE={} grok",
            shell_quote(&session.session_name),
            shell_quote(thread_ts),
            shell_quote(socket_path),
            shell_quote(token),
            shell_quote(&env::current_exe().unwrap_or_default().display().to_string()),
        ),
        _ => "exec grok".to_owned(),
    }
}

const GROK_HOOK: &str = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"test -n \"${PINGME_NOTIFY_SOCKET:-}\" && test -n \"${PINGME_EXECUTABLE:-}\" && \"${PINGME_EXECUTABLE}\" notify grok || true","timeout":5}]}]}}"#;

pub fn install_grok_hook() -> Result<(), String> {
    let home = env::var("HOME")
        .map_err(|_| "missing environment variable HOME for Grok hook".to_owned())?;
    let hooks = std::path::Path::new(&home).join(".grok/hooks");
    fs::create_dir_all(&hooks)
        .map_err(|error| format!("failed to create Grok hook directory: {error}"))?;
    let hook = hooks.join("pingme.json");
    if hook.exists() {
        let existing = fs::read_to_string(&hook)
            .map_err(|error| format!("failed to read Grok completion hook: {error}"))?;
        if existing == GROK_HOOK {
            return Ok(());
        }
        return Err(format!(
            "Grok completion hook already exists with different content: {}",
            hook.display()
        ));
    }
    fs::write(hook, GROK_HOOK)
        .map_err(|error| format!("failed to install Grok completion hook: {error}"))
}

pub(crate) fn grok_final_response(notification: &Value) -> Option<&str> {
    if !matches!(
        notification.get("hookEventName").and_then(Value::as_str),
        Some("stop" | "Stop")
    ) || notification.get("reason").and_then(Value::as_str) != Some("end_turn")
        || notification
            .get("subagentType")
            .is_some_and(|value| !value.is_null())
    {
        return None;
    }
    Some(
        notification
            .get("lastAssistantMessage")
            .and_then(Value::as_str)
            .unwrap_or("(no final response)"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_top_level_grok_stop_completion() {
        let completion = serde_json::json!({
            "hookEventName": "stop",
            "reason": "end_turn",
            "lastAssistantMessage": "done"
        });
        assert_eq!(Some("done"), grok_final_response(&completion));

        let subagent = serde_json::json!({
            "hookEventName": "stop",
            "reason": "end_turn",
            "subagentType": "explore",
            "lastAssistantMessage": "child result"
        });
        assert_eq!(None, grok_final_response(&subagent));
    }
}

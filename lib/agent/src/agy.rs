use std::{env, fs};

use pingme_session::AgentSession;
use pingme_support::shell::shell_quote;
use serde_json::{Map, Value};

use crate::prompt_fingerprint;

pub(crate) fn agy_command(
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
            "exec env PINGME_SESSION_NAME={} PINGME_THREAD_TS={} PINGME_NOTIFY_SOCKET={} PINGME_NOTIFY_TOKEN={} PINGME_EXECUTABLE={} agy",
            shell_quote(&session.session_name),
            shell_quote(thread_ts),
            shell_quote(socket_path),
            shell_quote(token),
            shell_quote(&env::current_exe().unwrap_or_default().display().to_string()),
        ),
        _ => "exec agy".to_owned(),
    }
}

const AGY_HOOK_COMMAND: &str = r#"test -n "${PINGME_NOTIFY_SOCKET:-}" && test -n "${PINGME_EXECUTABLE:-}" && "${PINGME_EXECUTABLE}" notify agy || true; printf '{}'"#;

fn pingme_agy_hook_spec() -> Value {
    serde_json::json!({
        "Stop": [
            {
                "type": "command",
                "command": AGY_HOOK_COMMAND,
                "timeout": 5
            }
        ]
    })
}

pub fn install_agy_hook() -> Result<(), String> {
    let home = env::var("HOME")
        .map_err(|_| "missing environment variable HOME for Antigravity hook".to_owned())?;
    let config_dir = std::path::Path::new(&home).join(".gemini/config");
    fs::create_dir_all(&config_dir)
        .map_err(|error| format!("failed to create Antigravity config directory: {error}"))?;
    let hook_path = config_dir.join("hooks.json");
    let existing = if hook_path.exists() {
        Some(
            fs::read_to_string(&hook_path)
                .map_err(|error| format!("failed to read Antigravity hooks file: {error}"))?,
        )
    } else {
        None
    };
    let merged = merge_agy_hook(existing.as_deref())?;
    if existing.as_deref() == Some(merged.as_str()) {
        return Ok(());
    }
    fs::write(hook_path, merged)
        .map_err(|error| format!("failed to install Antigravity completion hook: {error}"))
}

fn merge_agy_hook(existing: Option<&str>) -> Result<String, String> {
    let mut root: Map<String, Value> = match existing {
        Some(text) if !text.trim().is_empty() => serde_json::from_str::<Value>(text)
            .map_err(|error| format!("Antigravity hooks.json is invalid JSON: {error}"))?
            .as_object()
            .cloned()
            .ok_or_else(|| "Antigravity hooks.json must be a JSON object".to_owned())?,
        _ => Map::new(),
    };
    let expected = pingme_agy_hook_spec();
    if let Some(current) = root.get("pingme") {
        if current == &expected {
            return Ok(existing.unwrap_or_default().to_owned());
        }
        return Err(
            "Antigravity hooks.json already has a pingme entry with different content".to_owned(),
        );
    }
    root.insert("pingme".to_owned(), expected);
    serde_json::to_string_pretty(&Value::Object(root))
        .map(|text| format!("{text}\n"))
        .map_err(|error| format!("failed to serialize Antigravity hooks.json: {error}"))
}

/// Enriches an Antigravity `Stop` hook stdin payload with `lastAssistantMessage`
/// read from `transcriptPath`, keeping turn parsing in the daemon free of disk I/O.
pub fn enrich_agy_notification(raw_stdin: &str) -> Result<String, String> {
    let mut payload: Value = serde_json::from_str(raw_stdin)
        .map_err(|error| format!("invalid agy notification JSON: {error}"))?;
    if payload
        .get("lastAssistantMessage")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty())
    {
        return Ok(raw_stdin.to_owned());
    }
    if let Some(path) = payload.get("transcriptPath").and_then(Value::as_str)
        && let Ok(transcript) = fs::read_to_string(path)
        && let Some(answer) = extract_last_assistant_message(&transcript)
        && let Some(object) = payload.as_object_mut()
    {
        object.insert("lastAssistantMessage".to_owned(), Value::String(answer));
        return serde_json::to_string(&payload)
            .map_err(|error| format!("failed to serialize enriched agy notification: {error}"));
    }
    Ok(raw_stdin.to_owned())
}

/// Finds the last visible `PLANNER_RESPONSE` text after the most recent `USER_INPUT`.
pub fn extract_last_assistant_message(transcript_jsonl: &str) -> Option<String> {
    for line in transcript_jsonl.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        match entry.get("type").and_then(Value::as_str) {
            Some("USER_INPUT") => return None,
            Some("PLANNER_RESPONSE") => {
                if let Some(content) = entry.get("content").and_then(Value::as_str)
                    && !content.trim().is_empty()
                {
                    return Some(content.to_owned());
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn agy_final_response(notification: &Value) -> Option<&str> {
    if !matches!(
        notification.get("hookEventName").and_then(Value::as_str),
        Some("stop" | "Stop")
    ) {
        return None;
    }
    if notification
        .get("terminationReason")
        .and_then(Value::as_str)
        == Some("USER_CANCELED")
    {
        return None;
    }
    let answer = notification
        .get("lastAssistantMessage")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .or_else(|| {
            notification
                .get("error")
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
        })
        .unwrap_or("(no final response)");
    Some(answer)
}

pub(crate) fn agy_terminal_prompts<'a>(
    notification: &'a Value,
    last_slack_prompt_fingerprint: Option<&str>,
) -> Vec<&'a str> {
    let Some(prompt) = notification
        .get("lastUserInput")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    else {
        return Vec::new();
    };
    if Some(prompt_fingerprint(prompt).as_str()) == last_slack_prompt_fingerprint {
        return Vec::new();
    }
    vec![prompt]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_pingme_hook_while_preserving_other_hooks() {
        let created = merge_agy_hook(None).unwrap();
        let parsed: Value = serde_json::from_str(&created).unwrap();
        assert_eq!(pingme_agy_hook_spec(), parsed["pingme"]);
        // Re-running on the same file is a no-op.
        assert_eq!(created, merge_agy_hook(Some(&created)).unwrap());

        let with_other = serde_json::json!({
            "lint": { "PostToolUse": [] }
        })
        .to_string();
        let merged = merge_agy_hook(Some(&with_other)).unwrap();
        let parsed: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(serde_json::json!({ "PostToolUse": [] }), parsed["lint"]);
        assert_eq!(pingme_agy_hook_spec(), parsed["pingme"]);

        let conflict = serde_json::json!({
            "pingme": { "Stop": [] }
        })
        .to_string();
        assert!(
            merge_agy_hook(Some(&conflict))
                .unwrap_err()
                .contains("different content")
        );
    }

    #[test]
    fn extracts_last_assistant_message_after_most_recent_user_input() {
        let transcript = [
            r#"{"step_index":0,"type":"USER_INPUT","content":"first"}"#,
            r#"{"step_index":1,"type":"PLANNER_RESPONSE","content":"old answer"}"#,
            r#"{"step_index":2,"type":"USER_INPUT","content":"second"}"#,
            r#"{"step_index":3,"type":"PLANNER_RESPONSE","tool_calls":[{"name":"read_file"}]}"#,
            r#"{"step_index":4,"type":"GENERIC","content":"tool output"}"#,
            r#"{"step_index":5,"type":"PLANNER_RESPONSE","content":"final answer"}"#,
        ]
        .join("\n");
        assert_eq!(
            Some("final answer".to_owned()),
            extract_last_assistant_message(&transcript)
        );

        let no_reply_yet = [
            r#"{"step_index":0,"type":"PLANNER_RESPONSE","content":"old answer"}"#,
            r#"{"step_index":1,"type":"USER_INPUT","content":"second"}"#,
            r#"{"step_index":2,"type":"PLANNER_RESPONSE","tool_calls":[{"name":"read_file"}]}"#,
        ]
        .join("\n");
        assert_eq!(None, extract_last_assistant_message(&no_reply_yet));
    }

    #[test]
    fn parses_agy_stop_notifications_and_terminal_prompts() {
        let done = serde_json::json!({
            "hookEventName": "Stop",
            "terminationReason": "NO_TOOL_CALL",
            "lastUserInput": "explain the diff",
            "lastAssistantMessage": "Here is the summary."
        });
        assert_eq!(Some("Here is the summary."), agy_final_response(&done));
        assert_eq!(vec!["explain the diff"], agy_terminal_prompts(&done, None));
        assert!(
            agy_terminal_prompts(&done, Some(&prompt_fingerprint("explain the diff"))).is_empty()
        );

        let canceled = serde_json::json!({
            "hookEventName": "Stop",
            "terminationReason": "USER_CANCELED",
            "lastAssistantMessage": "partial"
        });
        assert_eq!(None, agy_final_response(&canceled));

        let errored = serde_json::json!({
            "hookEventName": "Stop",
            "terminationReason": "ERROR",
            "error": "quota exceeded"
        });
        assert_eq!(Some("quota exceeded"), agy_final_response(&errored));
    }
}

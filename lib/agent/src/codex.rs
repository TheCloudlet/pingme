use std::env;

use pingme_session::AgentSession;
use pingme_support::shell::shell_quote;
use serde_json::Value;

use crate::prompt_fingerprint;

pub(crate) fn codex_command(
    session: &AgentSession,
    notify_socket_path: Option<&str>,
    notify_token: Option<&str>,
) -> String {
    match (
        session.thread_ts.as_deref(),
        notify_socket_path,
        notify_token,
        notify_arg(),
    ) {
        (Some(thread_ts), Some(socket_path), Some(token), Some(notify)) => format!(
            "exec env PINGME_SESSION_NAME={} PINGME_THREAD_TS={} PINGME_NOTIFY_SOCKET={} PINGME_NOTIFY_TOKEN={} codex -c {}",
            shell_quote(&session.session_name),
            shell_quote(thread_ts),
            shell_quote(socket_path),
            shell_quote(token),
            shell_quote(&format!("notify={notify}")),
        ),
        (Some(thread_ts), _, _, _) => format!(
            "exec env PINGME_SESSION_NAME={} PINGME_THREAD_TS={} codex",
            shell_quote(&session.session_name),
            shell_quote(thread_ts),
        ),
        (None, _, _, _) => "exec codex".to_owned(),
    }
}

fn notify_arg() -> Option<String> {
    let executable = env::current_exe().ok()?.display().to_string();
    serde_json::to_string(&vec![executable, "notify".to_owned()]).ok()
}

pub fn terminal_prompts<'a>(
    notification: &'a Value,
    last_slack_prompt_fingerprint: Option<&str>,
    already_mirrored: usize,
) -> Vec<&'a str> {
    let Some(messages) = notification.get("input-messages").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut prompts = Vec::new();
    // `input-messages` accumulates the Agent CLI's whole terminal input history,
    // not just this turn's; only the entries past what was already mirrored are new.
    for message in messages.iter().skip(already_mirrored) {
        let Some(prompt) = message
            .as_str()
            .map(str::trim)
            .filter(|prompt| !prompt.is_empty())
        else {
            continue;
        };
        if Some(prompt_fingerprint(prompt).as_str()) != last_slack_prompt_fingerprint {
            prompts.push(prompt);
        }
    }
    prompts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt_fingerprint;

    #[test]
    fn codex_notification_mirrors_all_terminal_input() {
        let notification = serde_json::json!({
            "type": "agent-turn-complete",
            "input-messages": ["first terminal prompt", "second terminal prompt"],
            "last-assistant-message": "done"
        });

        assert_eq!(
            vec!["first terminal prompt", "second terminal prompt"],
            terminal_prompts(&notification, None, 0)
        );
        assert_eq!(
            vec!["first terminal prompt"],
            terminal_prompts(
                &notification,
                Some(&prompt_fingerprint("second terminal prompt")),
                0
            )
        );
    }

    #[test]
    fn codex_notification_does_not_remirror_already_mirrored_terminal_input() {
        // input-messages accumulates the whole terminal history across turns;
        // a later turn's notification must not re-post what an earlier turn
        // already mirrored (this is what caused the reported spam).
        let first_turn = serde_json::json!({
            "type": "agent-turn-complete",
            "input-messages": ["first terminal prompt"],
            "last-assistant-message": "done"
        });
        assert_eq!(
            vec!["first terminal prompt"],
            terminal_prompts(&first_turn, None, 0)
        );

        let second_turn = serde_json::json!({
            "type": "agent-turn-complete",
            "input-messages": ["first terminal prompt", "second terminal prompt"],
            "last-assistant-message": "done"
        });
        assert_eq!(
            vec!["second terminal prompt"],
            terminal_prompts(&second_turn, None, 1)
        );

        // A repeat notification carrying the same history mirrors nothing new.
        assert_eq!(Vec::<&str>::new(), terminal_prompts(&second_turn, None, 2));
    }
}

use pingme_session::AgentSession;
use serde_json::Value;

use crate::codex::terminal_prompts;
use crate::grok::grok_final_response;

/// The end of an Agent CLI turn, read from its completion notification.
pub struct FinishedTurn<'a> {
    pub response: &'a str,
    /// Prompts typed in the terminal since the last mirrored one (Codex CLI only).
    pub terminal_prompts: Vec<&'a str>,
    pub terminal_input_count: usize,
}

/// Returns None for notifications that do not end a top-level turn.
pub fn finished_turn<'a>(
    session: &AgentSession,
    notification: &'a Value,
) -> Option<FinishedTurn<'a>> {
    if session.agent_cli == "grok" {
        return Some(FinishedTurn {
            response: grok_final_response(notification)?,
            terminal_prompts: Vec::new(),
            terminal_input_count: session.mirrored_terminal_count,
        });
    }
    if notification.get("type").and_then(Value::as_str) != Some("agent-turn-complete") {
        return None;
    }
    Some(FinishedTurn {
        response: notification
            .get("last-assistant-message")
            .and_then(Value::as_str)
            .unwrap_or("(no final response)"),
        terminal_prompts: terminal_prompts(
            notification,
            session.last_slack_prompt.as_deref(),
            session.mirrored_terminal_count,
        ),
        terminal_input_count: notification
            .get("input-messages")
            .and_then(Value::as_array)
            .map_or(session.mirrored_terminal_count, Vec::len),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_turn_reads_each_agent_cli_completion() {
        let mut codex =
            pingme_session::testing::test_session("codex-project", "%7", Some("100.1"), "C1", "U1");
        codex.mirrored_terminal_count = 1;
        let started = serde_json::json!({ "type": "agent-turn-started" });
        assert!(finished_turn(&codex, &started).is_none());

        let completed = serde_json::json!({
            "type": "agent-turn-complete",
            "input-messages": ["old", "new"],
            "last-assistant-message": "done"
        });
        let turn = finished_turn(&codex, &completed).unwrap();
        assert_eq!("done", turn.response);
        assert_eq!(vec!["new"], turn.terminal_prompts);
        assert_eq!(2, turn.terminal_input_count);

        let mut grok = codex.clone();
        grok.agent_cli = "grok".to_owned();
        let stop = serde_json::json!({
            "hookEventName": "Stop",
            "reason": "end_turn",
            "lastAssistantMessage": "hi"
        });
        let turn = finished_turn(&grok, &stop).unwrap();
        assert_eq!("hi", turn.response);
        assert!(turn.terminal_prompts.is_empty());
        assert_eq!(1, turn.terminal_input_count);
    }
}

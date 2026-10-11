use pingme_session::{AgentSession, SessionStatus};
use pingme_setting::SlackSetting;

use crate::photo::{SlackFile, accepted_slack_photos};
use crate::update::session_update_reject;

#[derive(Clone)]
pub struct ThreadMessage<'a> {
    pub channel_id: &'a str,
    pub thread_ts: Option<&'a str>,
    pub user_id: Option<&'a str>,
    pub text: &'a str,
    pub has_subtype: bool,
    pub is_bot: bool,
    pub event_ts: Option<&'a str>,
    pub files: Vec<SlackFile>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ThreadAction {
    Prompt(String),
    Stop,
    MarkUnavailable,
    Name,
    Rename(String),
    Ignore,
    Reject(String),
}

pub struct RoutedThreadAction<'a> {
    pub session: &'a AgentSession,
    pub action: ThreadAction,
}

/// Finds the Agent Session that owns a Control Channel thread and decides what the message asks.
pub fn route_thread_message<'session>(
    message: &ThreadMessage<'_>,
    sessions: &'session [AgentSession],
    slack: &SlackSetting,
) -> Option<RoutedThreadAction<'session>> {
    if message.channel_id != slack.control_channel_id {
        return None;
    }
    let thread_ts = message.thread_ts?;
    let session = sessions.iter().find(|session| {
        session.thread_ts.as_deref() == Some(thread_ts)
            && session.control_channel == slack.control_channel_id
    })?;
    let action = thread_action(message, &slack.operator_id, &session.agent_cli);
    Some(RoutedThreadAction { session, action })
}

pub(crate) fn thread_action(
    message: &ThreadMessage<'_>,
    operator_id: &str,
    agent_cli: &str,
) -> ThreadAction {
    if message.is_bot {
        return ThreadAction::Ignore;
    }
    if message.has_subtype && message.files.is_empty() {
        return ThreadAction::Ignore;
    }
    if message.user_id != Some(operator_id) {
        return ThreadAction::Reject(session_update_reject(agent_cli, "unauthorized"));
    }

    let text = message.text.trim();
    if !message.files.is_empty() {
        return match accepted_slack_photos(&message.files) {
            Ok(photos) if photos.is_empty() => ThreadAction::Ignore,
            Ok(_) => ThreadAction::Prompt(text.to_owned()),
            Err(reason) => ThreadAction::Reject(reason),
        };
    }
    match text {
        "pingme stop" => ThreadAction::Stop,
        "pingme name" => ThreadAction::Name,
        _ => {
            if let Some(name) = text.strip_prefix("pingme rename ") {
                let name = name.trim();
                if name.is_empty() {
                    ThreadAction::Reject("usage: pingme rename <new-session-name>".to_owned())
                } else {
                    ThreadAction::Rename(name.to_owned())
                }
            } else {
                ThreadAction::Prompt(text.to_owned())
            }
        }
    }
}

pub fn apply_host_routing_policy(
    message: &ThreadMessage<'_>,
    session: &AgentSession,
    action: ThreadAction,
    started_at: f64,
) -> ThreadAction {
    if matches!(action, ThreadAction::Ignore) {
        return action;
    }
    if message
        .event_ts
        .and_then(|event_ts| event_ts.parse::<f64>().ok())
        .is_none_or(|event_ts| event_ts < started_at)
    {
        return ThreadAction::Reject(session_update_reject(&session.agent_cli, "stale"));
    }
    match action {
        ThreadAction::Prompt(_) if session.available && session.status == SessionStatus::Busy => {
            ThreadAction::Reject(session_update_reject(&session.agent_cli, "busy"))
        }
        ThreadAction::Prompt(_) | ThreadAction::Stop if !session.available => {
            ThreadAction::MarkUnavailable
        }
        action => action,
    }
}

#[cfg(test)]
mod tests {
    use pingme_setting::SlackSetting;

    use super::*;
    use crate::update::session_update_reject;

    fn slack_setting(control_channel_id: &str, operator_id: &str) -> SlackSetting {
        SlackSetting {
            control_channel_id: control_channel_id.to_owned(),
            operator_id: operator_id.to_owned(),
        }
    }

    #[test]
    fn routes_each_session_thread_to_its_own_agent_session() {
        let slack = slack_setting("C_TEST_CONTROL", "U_TEST_OPERATOR");
        let sessions = vec![
            pingme_session::testing::test_session(
                "first",
                "%1",
                Some("100.1"),
                "C_TEST_CONTROL",
                "U_TEST_OPERATOR",
            ),
            pingme_session::testing::test_session(
                "second",
                "%2",
                Some("100.2"),
                "C_TEST_CONTROL",
                "U_TEST_OPERATOR",
            ),
        ];
        let message = ThreadMessage {
            channel_id: "C_TEST_CONTROL",
            thread_ts: Some("100.2"),
            user_id: Some("U_TEST_OPERATOR"),
            text: "continue",
            has_subtype: false,
            is_bot: false,
            event_ts: Some("101.0"),
            files: Vec::new(),
        };

        let routed = route_thread_message(&message, &sessions, &slack).unwrap();

        assert_eq!(Some("%2"), routed.session.pane_id.as_deref());
        assert_eq!(ThreadAction::Prompt("continue".to_owned()), routed.action);
        for stray in [
            ThreadMessage {
                thread_ts: Some("100.9"),
                ..message.clone()
            },
            ThreadMessage {
                channel_id: "C_OTHER",
                ..message.clone()
            },
        ] {
            assert!(route_thread_message(&stray, &sessions, &slack).is_none());
        }
    }

    #[test]
    fn routes_only_operator_messages_in_mapped_thread() {
        let message = ThreadMessage {
            channel_id: "C_TEST_CONTROL",
            thread_ts: Some("1757221923.123456"),
            user_id: Some("U_TEST_OPERATOR"),
            text: "hello codex",
            has_subtype: false,
            is_bot: false,
            event_ts: Some("1757221924.000000"),
            files: Vec::new(),
        };

        assert_eq!(
            ThreadAction::Prompt("hello codex".to_owned()),
            thread_action(&message, "U_TEST_OPERATOR", "codex",)
        );

        let wrong_user = ThreadMessage {
            user_id: Some("UOTHER"),
            ..message.clone()
        };
        assert_eq!(
            ThreadAction::Reject(session_update_reject("codex", "unauthorized")),
            thread_action(&wrong_user, "U_TEST_OPERATOR", "codex",)
        );

        let bot_message = ThreadMessage {
            has_subtype: true,
            ..message.clone()
        };
        assert_eq!(
            ThreadAction::Ignore,
            thread_action(&bot_message, "U_TEST_OPERATOR", "codex",)
        );

        let bot_without_subtype = ThreadMessage {
            user_id: Some("UBOT"),
            text: "not executed: unauthorized Slack user",
            is_bot: true,
            ..message.clone()
        };
        assert_eq!(
            ThreadAction::Ignore,
            thread_action(&bot_without_subtype, "U_TEST_OPERATOR", "codex",)
        );
    }

    #[test]
    fn recognizes_thread_controls_exactly() {
        let base = ThreadMessage {
            channel_id: "C_TEST_CONTROL",
            thread_ts: Some("1757221923.123456"),
            user_id: Some("U_TEST_OPERATOR"),
            text: "",
            has_subtype: false,
            is_bot: false,
            event_ts: Some("1757221924.000000"),
            files: Vec::new(),
        };

        assert_eq!(
            ThreadAction::Stop,
            thread_action(
                &ThreadMessage {
                    text: "pingme stop",
                    ..base.clone()
                },
                "U_TEST_OPERATOR",
                "codex",
            )
        );
        assert_eq!(
            ThreadAction::Name,
            thread_action(
                &ThreadMessage {
                    text: "pingme name",
                    ..base.clone()
                },
                "U_TEST_OPERATOR",
                "codex",
            )
        );
        assert_eq!(
            ThreadAction::Rename("better-name".to_owned()),
            thread_action(
                &ThreadMessage {
                    text: "pingme rename better-name",
                    ..base.clone()
                },
                "U_TEST_OPERATOR",
                "codex",
            )
        );
        assert_eq!(
            ThreadAction::Prompt("please write about pingme stop".to_owned()),
            thread_action(
                &ThreadMessage {
                    text: "please write about pingme stop",
                    ..base.clone()
                },
                "U_TEST_OPERATOR",
                "codex",
            )
        );
        assert_eq!(
            ThreadAction::Prompt("cli-stop".to_owned()),
            thread_action(
                &ThreadMessage {
                    text: "cli-stop",
                    ..base.clone()
                },
                "U_TEST_OPERATOR",
                "codex",
            )
        );
    }

    #[test]
    fn received_session_update_from_the_bot_is_ignored() {
        let message = ThreadMessage {
            channel_id: "C_TEST_CONTROL",
            thread_ts: Some("1757221923.123456"),
            user_id: Some("U_BRIDGE_BOT"),
            text: "(ping grok)",
            has_subtype: false,
            is_bot: true,
            event_ts: Some("1757221925.000000"),
            files: Vec::new(),
        };

        assert_eq!(
            ThreadAction::Ignore,
            thread_action(&message, "U_TEST_OPERATOR", "codex",)
        );
    }
}

use pingme_routing::photo::SlackFile;
use pingme_routing::thread::ThreadMessage;
use serde_json::Value;

pub fn thread_message_from_envelope(envelope: &Value) -> Option<ThreadMessage<'_>> {
    let event = envelope.get("payload")?.get("event")?;
    if event.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    Some(ThreadMessage {
        channel_id: event.get("channel").and_then(Value::as_str).unwrap_or(""),
        thread_ts: event.get("thread_ts").and_then(Value::as_str),
        user_id: event.get("user").and_then(Value::as_str),
        text: event.get("text").and_then(Value::as_str).unwrap_or(""),
        has_subtype: event.get("subtype").is_some(),
        is_bot: event.get("bot_id").is_some(),
        event_ts: event
            .get("event_ts")
            .or_else(|| event.get("ts"))
            .and_then(Value::as_str),
        files: slack_files_from_event(event),
    })
}

fn slack_files_from_event(event: &Value) -> Vec<SlackFile> {
    let Some(files) = event.get("files").and_then(Value::as_array) else {
        return Vec::new();
    };
    files.iter().filter_map(slack_file_from_value).collect()
}

fn slack_file_from_value(value: &Value) -> Option<SlackFile> {
    let id = value.get("id").and_then(Value::as_str)?.to_owned();
    let download_url = value
        .get("url_private_download")
        .or_else(|| value.get("url_private"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    Some(SlackFile {
        id,
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        mimetype: value
            .get("mimetype")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        download_url,
        size: value.get("size").and_then(Value::as_u64).unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use pingme_routing::thread::{ThreadAction, apply_host_routing_policy, route_thread_message};
    use pingme_routing::update::session_update_reject;
    use pingme_session::SessionStatus;
    use pingme_session::testing::test_session;
    use serde_json::Value;

    use super::*;

    fn slack_setting(control_channel_id: &str, operator_id: &str) -> pingme_setting::SlackSetting {
        pingme_setting::SlackSetting {
            control_channel_id: control_channel_id.to_owned(),
            operator_id: operator_id.to_owned(),
        }
    }

    fn recorded_slack_envelopes() -> Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/slack-message-envelopes.json"
        ))
        .unwrap()
    }

    #[test]
    fn recorded_operator_envelope_routes_as_a_prompt() {
        let envelopes = recorded_slack_envelopes();
        let message = thread_message_from_envelope(&envelopes["operator"]).unwrap();
        let session = test_session(
            "codex-project",
            "%7",
            Some("1757221923.123456"),
            "C_CONTROL",
            "U_OPERATOR",
        );
        let slack = slack_setting("C_CONTROL", "U_OPERATOR");
        let sessions = [session];

        let routed = route_thread_message(&message, &sessions, &slack).unwrap();
        let action = apply_host_routing_policy(&message, routed.session, routed.action, 0.0);

        assert_eq!(
            ThreadAction::Prompt("inspect the failure".to_owned()),
            action
        );
    }

    #[test]
    fn recorded_operator_prompt_is_rejected_while_session_is_busy() {
        let envelopes = recorded_slack_envelopes();
        let message = thread_message_from_envelope(&envelopes["operator"]).unwrap();
        let mut session = test_session(
            "codex-project",
            "%7",
            Some("1757221923.123456"),
            "C_CONTROL",
            "U_OPERATOR",
        );
        session.status = SessionStatus::Busy;
        let slack = slack_setting("C_CONTROL", "U_OPERATOR");
        let sessions = [session];
        let routed = route_thread_message(&message, &sessions, &slack).unwrap();

        let action = apply_host_routing_policy(&message, routed.session, routed.action, 0.0);

        assert_eq!(
            ThreadAction::Reject(session_update_reject("codex", "busy")),
            action
        );
    }

    #[test]
    fn recorded_stale_operator_prompt_is_rejected() {
        let envelopes = recorded_slack_envelopes();
        let message = thread_message_from_envelope(&envelopes["operator"]).unwrap();
        let session = test_session(
            "codex-project",
            "%7",
            Some("1757221923.123456"),
            "C_CONTROL",
            "U_OPERATOR",
        );
        let slack = slack_setting("C_CONTROL", "U_OPERATOR");
        let sessions = [session];
        let routed = route_thread_message(&message, &sessions, &slack).unwrap();

        let action =
            apply_host_routing_policy(&message, routed.session, routed.action, 1757221925.0);

        assert_eq!(
            ThreadAction::Reject(session_update_reject("codex", "stale")),
            action
        );
    }

    #[test]
    fn recorded_operator_prompt_is_rejected_for_unavailable_session() {
        let envelopes = recorded_slack_envelopes();
        let message = thread_message_from_envelope(&envelopes["operator"]).unwrap();
        let mut session = test_session(
            "codex-project",
            "%7",
            Some("1757221923.123456"),
            "C_CONTROL",
            "U_OPERATOR",
        );
        session.available = false;
        let slack = slack_setting("C_CONTROL", "U_OPERATOR");
        let sessions = [session];
        let routed = route_thread_message(&message, &sessions, &slack).unwrap();

        let action = apply_host_routing_policy(&message, routed.session, routed.action, 0.0);

        assert_eq!(ThreadAction::MarkUnavailable, action);
    }

    #[test]
    fn recorded_pingme_stop_decides_to_stop_without_ending_the_session() {
        let envelopes = recorded_slack_envelopes();
        let message = thread_message_from_envelope(&envelopes["operator_stop"]).unwrap();
        let session = test_session(
            "codex-project",
            "%7",
            Some("1757221923.123456"),
            "C_CONTROL",
            "U_OPERATOR",
        );
        let slack = slack_setting("C_CONTROL", "U_OPERATOR");
        let sessions = [session];
        let routed = route_thread_message(&message, &sessions, &slack).unwrap();

        let action = apply_host_routing_policy(&message, routed.session, routed.action, 0.0);

        assert_eq!(ThreadAction::Stop, action);
        assert!(routed.session.available);
        assert_eq!(SessionStatus::Idle, routed.session.status);
    }

    #[test]
    fn recorded_other_member_envelope_is_rejected() {
        let envelopes = recorded_slack_envelopes();
        let message = thread_message_from_envelope(&envelopes["other_member"]).unwrap();
        let session = test_session(
            "codex-project",
            "%7",
            Some("1757221923.123456"),
            "C_CONTROL",
            "U_OPERATOR",
        );
        let slack = slack_setting("C_CONTROL", "U_OPERATOR");
        let sessions = [session];

        let routed = route_thread_message(&message, &sessions, &slack).unwrap();

        assert_eq!(
            ThreadAction::Reject(session_update_reject("codex", "unauthorized")),
            routed.action
        );
    }

    #[test]
    fn recorded_non_operator_envelopes_never_route_as_prompts() {
        let envelopes = recorded_slack_envelopes();
        let session = test_session(
            "codex-project",
            "%7",
            Some("1757221923.123456"),
            "C_CONTROL",
            "U_OPERATOR",
        );
        let slack = slack_setting("C_CONTROL", "U_OPERATOR");
        let sessions = [session];

        for (fixture, expected) in [
            (
                "missing_identity",
                Some(ThreadAction::Reject(session_update_reject(
                    "codex",
                    "unauthorized",
                ))),
            ),
            ("bot_message", Some(ThreadAction::Ignore)),
            ("message_changed", None),
        ] {
            let message = thread_message_from_envelope(&envelopes[fixture]).unwrap();
            let action =
                route_thread_message(&message, &sessions, &slack).map(|routed| routed.action);

            assert_eq!(expected, action, "{fixture}");
        }
    }

    #[test]
    fn recorded_operator_png_file_share_routes_as_a_prompt() {
        let envelopes = recorded_slack_envelopes();
        let message = thread_message_from_envelope(&envelopes["operator_png_file_share"]).unwrap();
        let session = test_session(
            "codex-project",
            "%7",
            Some("1757221923.123456"),
            "C_CONTROL",
            "U_OPERATOR",
        );
        let slack = slack_setting("C_CONTROL", "U_OPERATOR");
        let sessions = [session];

        let routed = route_thread_message(&message, &sessions, &slack).unwrap();

        assert_eq!(ThreadAction::Prompt(String::new()), routed.action);
        assert_eq!("F123PNG", message.files[0].id);
        assert_eq!("image/png", message.files[0].mimetype);
    }

    #[test]
    fn recorded_operator_jpeg_caption_routes_as_a_prompt() {
        let envelopes = recorded_slack_envelopes();
        let message = thread_message_from_envelope(&envelopes["operator_jpeg_caption"]).unwrap();
        let session = test_session(
            "codex-project",
            "%7",
            Some("1757221923.123456"),
            "C_CONTROL",
            "U_OPERATOR",
        );
        let slack = slack_setting("C_CONTROL", "U_OPERATOR");
        let sessions = [session];

        let routed = route_thread_message(&message, &sessions, &slack).unwrap();

        assert_eq!(
            ThreadAction::Prompt("what is wrong here".to_owned()),
            routed.action
        );
        assert_eq!("image/jpeg", message.files[0].mimetype);
    }

    #[test]
    fn recorded_operator_pdf_is_rejected() {
        let envelopes = recorded_slack_envelopes();
        let message = thread_message_from_envelope(&envelopes["operator_pdf"]).unwrap();
        let session = test_session(
            "codex-project",
            "%7",
            Some("1757221923.123456"),
            "C_CONTROL",
            "U_OPERATOR",
        );
        let slack = slack_setting("C_CONTROL", "U_OPERATOR");
        let sessions = [session];

        let routed = route_thread_message(&message, &sessions, &slack).unwrap();

        assert_eq!(
            ThreadAction::Reject("not executed: unsupported file type: notes.pdf".to_owned()),
            routed.action
        );
    }
}

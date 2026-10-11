use pingme_session::{AgentSession, SessionStatus};

pub fn agent_display_name(agent_cli: &str) -> &str {
    match agent_cli {
        "codex" => "Codex",
        "grok" => "Grok",
        _ => agent_cli,
    }
}

pub fn render_session_status_card(session: &AgentSession) -> String {
    let status = if session.available {
        session.status
    } else {
        SessionStatus::Unavailable
    };
    format!(
        "PingMe Session\nAgent: {}\nSession: {}\nStatus: {}\nHost: {}\nProject: {}\ncwd: {}\nPane: {}\nThread: {}",
        session.agent_cli,
        session.session_name,
        status.as_str(),
        session.host,
        session.project,
        session.cwd,
        session.pane_id.as_deref().unwrap_or("pending"),
        session.thread_ts.as_deref().unwrap_or("pending"),
    )
}

pub fn session_update_after_prompt_paste(paste: Result<(), String>, agent_cli: &str) -> String {
    match paste {
        Ok(()) => format!("(ping {agent_cli})"),
        Err(error) => format!(
            "Not executed: failed to send prompt to {}: {error}",
            agent_display_name(agent_cli)
        ),
    }
}

pub fn session_update_after_agent_turn(agent_cli: &str, response: &str) -> String {
    format!("(pong {agent_cli})\n{response}")
}

pub fn session_update_reject(agent_cli: &str, reason: &str) -> String {
    format!("(reject {agent_cli} {reason})")
}

#[cfg(test)]
mod tests {
    use pingme_session::{AgentSession, PanePlacement, SessionStatus};

    use super::*;

    #[test]
    fn renders_status_cards_for_session_states() {
        let launch = AgentSession::for_launch(
            "codex-unknown",
            "codex",
            "linux",
            "unknown",
            "/tmp/worktree",
            "C_TEST_CONTROL",
            "U_TEST_OPERATOR",
            PanePlacement::DetachedWindow,
        );

        let starting = render_session_status_card(&launch);
        let mut session = launch;
        session.thread_ts = Some("1757221923.123456".to_owned());
        session.pane_id = Some("%7".to_owned());
        session.status = SessionStatus::Idle;
        let idle = render_session_status_card(&session);
        session.status = SessionStatus::Busy;
        let busy = render_session_status_card(&session);
        session.status = SessionStatus::Unavailable;
        let unavailable = render_session_status_card(&session);

        assert!(starting.contains("Status: starting"));
        assert!(starting.contains("Project: unknown"));
        assert!(starting.contains("cwd: /tmp/worktree"));
        assert!(starting.contains("Pane: pending"));
        assert!(starting.contains("Thread: pending"));
        assert!(idle.contains("Status: idle"));
        assert!(idle.contains("Pane: %7"));
        assert!(idle.contains("Thread: 1757221923.123456"));
        assert!(busy.contains("Status: busy"));
        assert!(unavailable.contains("Status: unavailable"));
    }

    #[test]
    fn successful_prompt_paste_posts_received_session_update() {
        assert_eq!(
            "(ping codex)",
            session_update_after_prompt_paste(Ok(()), "codex")
        );
        assert_eq!(
            "(ping grok)",
            session_update_after_prompt_paste(Ok(()), "grok")
        );
    }

    #[test]
    fn agent_turn_posts_pong_session_update() {
        assert_eq!(
            "(pong grok)\nhello",
            session_update_after_agent_turn("grok", "hello")
        );
        assert_eq!(
            "(pong codex)\nhello",
            session_update_after_agent_turn("codex", "hello")
        );
    }

    #[test]
    fn failed_prompt_paste_posts_not_executed_without_received_update() {
        let update = session_update_after_prompt_paste(Err("pane is dead".to_owned()), "codex");
        assert_eq!(
            "Not executed: failed to send prompt to Codex: pane is dead",
            update
        );
        assert!(!update.contains("(ping"));
    }
}

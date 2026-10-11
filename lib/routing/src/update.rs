use std::time::Duration;

use pingme_session::{AgentSession, SessionStatus};

const PANE_TAIL_LINES: usize = 40;

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

pub fn session_update_queued(agent_cli: &str, replaced: bool) -> String {
    if replaced {
        format!("(queued {agent_cli}) replaced the waiting prompt")
    } else {
        format!("(queued {agent_cli})")
    }
}

fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m", secs / 60)
    }
}

/// The `(ping <agent>)` receipt, edited in place while the turn runs and when it ends.
pub fn session_update_turn(agent_cli: &str, state: &str, elapsed: Duration) -> String {
    format!("(ping {agent_cli}) {state} {}", format_elapsed(elapsed))
}

/// A new Session Thread message, so the phone notifies, when the pane stops changing.
pub fn session_update_waiting(pane_text: &str, max_chars: usize) -> String {
    let header = "⏸ waiting on you? Answer with `pingme keys <keys>`.\n";
    format!(
        "{header}{}",
        screen_block(pane_text, max_chars - header.chars().count())
    )
}

/// The bottom of the pane, where a prompt or menu is, in at most `max_chars`.
pub fn screen_block(pane_text: &str, max_chars: usize) -> String {
    let mut used = "```\n\n```".len();
    let mut tail: Vec<&str> = Vec::new();
    for line in pane_text
        .lines()
        .map(str::trim_end)
        .rev()
        .skip_while(|line| line.is_empty())
        .take(PANE_TAIL_LINES)
    {
        used += line.chars().count() + 1;
        if used > max_chars {
            break;
        }
        tail.push(line);
    }
    while tail.last().is_some_and(|line| line.trim().is_empty()) {
        tail.pop();
    }
    tail.reverse();
    format!("```\n{}\n```", tail.join("\n").replace("```", "'''"))
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
    fn queued_prompt_names_a_replacement() {
        assert_eq!("(queued grok)", session_update_queued("grok", false));
        assert_eq!(
            "(queued codex) replaced the waiting prompt",
            session_update_queued("codex", true)
        );
    }

    #[test]
    fn receipt_shows_the_turn_state_and_elapsed_time() {
        assert_eq!(
            "(ping grok) ⏳ working 45s",
            session_update_turn("grok", "⏳ working", Duration::from_secs(45))
        );
        assert_eq!(
            "(ping codex) ✓ done 2m",
            session_update_turn("codex", "✓ done", Duration::from_secs(150))
        );
    }

    #[test]
    fn screen_block_keeps_the_bottom_within_the_limit() {
        let pane = "\n  \n".to_owned()
            + &(0..41)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n")
            + "\n\n   \n";
        assert_eq!("```\nprompt\n```", screen_block("\n\n   \nprompt\n", 3_500));
        let block = screen_block(&pane, 3_500);
        assert!(block.starts_with("```\n1\n"));
        assert!(block.ends_with("\n40\n```"));

        // 54 rows of a 100-column TUI do not fit; the prompt at the bottom must.
        let tall = std::iter::repeat_n("─".repeat(100), 53)
            .chain(["3 (○) Yes, proceed".to_owned()])
            .collect::<Vec<_>>()
            .join("\n");
        let block = screen_block(&tall, 1_000);
        assert!(block.chars().count() <= 1_000);
        assert!(block.ends_with("3 (○) Yes, proceed\n```"));

        let waiting = session_update_waiting(&tall, 3_500);
        assert!(waiting.chars().count() <= 3_500);
        assert!(waiting.contains("pingme keys"));
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

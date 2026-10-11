// ponytail: O(n^2) over local Sessions; index by pane ID if a Host reaches hundreds.
use pingme_routing::update::agent_display_name;
use pingme_session::{AgentSession, SessionStatus};
use pingme_slack::api::{delete_thread_root, post_thread_reply};

fn ended_agent_sessions(known: &[AgentSession], current: &[AgentSession]) -> Vec<AgentSession> {
    let mut ended: Vec<AgentSession> = known
        .iter()
        .filter(|known| {
            known.available
                && known.status != SessionStatus::Unavailable
                && current
                    .iter()
                    .find(|current| current.pane_id == known.pane_id)
                    .is_none_or(|current| !current.available)
        })
        .cloned()
        .collect();
    ended.extend(
        current
            .iter()
            .filter(|current| {
                !current.available
                    && current.status != SessionStatus::Unavailable
                    && !known.iter().any(|known| {
                        known.pane_id == current.pane_id
                            && (known.available || known.status == SessionStatus::Unavailable)
                    })
            })
            .cloned(),
    );
    ended
}

pub(crate) fn monitor_agent_sessions(
    bot_token: &str,
    known: &mut Vec<AgentSession>,
) -> Result<(), String> {
    let mut current = AgentSession::list_from_registry()?;
    let ended = ended_agent_sessions(known, &current);
    for ended_session in &ended {
        let mut session = ended_session.clone();
        if current
            .iter()
            .any(|current| current.pane_id == session.pane_id)
        {
            session.write_status(SessionStatus::Unavailable)?;
        } else {
            session.mark_unavailable();
        }
        session.available = false;
        publish_ended_agent_session(
            &session,
            |session| {
                post_thread_reply(
                    bot_token,
                    session,
                    &format!("{} TUI ended.", agent_display_name(&session.agent_cli)),
                )
            },
            |session| delete_thread_root(bot_token, session),
        )?;
        record_unavailable(known, &session);
    }
    for session in &mut current {
        if !session.available {
            session.status = SessionStatus::Unavailable;
        }
    }
    for session in &ended {
        record_unavailable(&mut current, session);
    }
    *known = current;
    Ok(())
}

fn record_unavailable(sessions: &mut Vec<AgentSession>, ended: &AgentSession) {
    if let Some(session) = sessions
        .iter_mut()
        .find(|session| session.pane_id == ended.pane_id)
    {
        session.mark_unavailable();
    } else {
        let mut ended = ended.clone();
        ended.mark_unavailable();
        sessions.push(ended);
    }
}

fn publish_ended_agent_session(
    session: &AgentSession,
    mut post_tui_ended: impl FnMut(&AgentSession) -> Result<(), String>,
    mut delete_thread_root: impl FnMut(&AgentSession) -> Result<(), String>,
) -> Result<(), String> {
    // An Unbridged CLI Session has no Session Thread to tell.
    if session.thread_ts.is_none() {
        return Ok(());
    }
    post_tui_ended(session)?;
    delete_thread_root(session)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use pingme_session::testing::test_session;

    use super::*;

    #[test]
    fn tui_ended_deletes_the_session_thread_root_afterward() {
        let session = test_session("codex-project", "%7", Some("100.1"), "C1", "U1");
        let events = RefCell::new(Vec::new());

        publish_ended_agent_session(
            &session,
            |session| {
                events.borrow_mut().push(format!(
                    "tui-ended:{}:{}",
                    session.control_channel,
                    session.thread_ts.as_deref().unwrap_or_default()
                ));
                Ok(())
            },
            |session| {
                events.borrow_mut().push(format!(
                    "delete-root:{}:{}",
                    session.control_channel,
                    session.thread_ts.as_deref().unwrap_or_default()
                ));
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(
            vec!["tui-ended:C1:100.1", "delete-root:C1:100.1"],
            events.into_inner()
        );
    }

    #[test]
    fn tui_ended_without_a_thread_posts_nothing() {
        let session = test_session("codex-project", "%7", None, "C1", "U1");
        let touched = RefCell::new(false);

        publish_ended_agent_session(
            &session,
            |_| Err("Agent Session codex-project has no Session Thread".to_owned()),
            |_| {
                *touched.borrow_mut() = true;
                Ok(())
            },
        )
        .unwrap();

        assert!(!*touched.borrow());
    }

    #[test]
    fn failed_tui_ended_preserves_the_session_thread_root() {
        let session = test_session("codex-project", "%7", Some("100.1"), "C1", "U1");
        let deleted = RefCell::new(false);

        let result = publish_ended_agent_session(
            &session,
            |_| Err("Slack API error: ratelimited".to_owned()),
            |_| {
                *deleted.borrow_mut() = true;
                Ok(())
            },
        );

        assert_eq!(Err("Slack API error: ratelimited".to_owned()), result);
        assert!(!*deleted.borrow());
    }

    #[test]
    fn pane_monitor_detects_an_ended_agent_session_once() {
        let live = test_session("codex-project", "%7", Some("100.1"), "C1", "U1");
        let mut unavailable = live.clone();
        unavailable.available = false;
        let mut already_reported = live.clone();
        already_reported.mark_unavailable();

        assert_eq!(
            1,
            ended_agent_sessions(
                std::slice::from_ref(&live),
                std::slice::from_ref(&unavailable)
            )
            .len()
        );
        assert_eq!(
            1,
            ended_agent_sessions(&[], std::slice::from_ref(&unavailable)).len()
        );
        assert!(
            ended_agent_sessions(&[already_reported], std::slice::from_ref(&unavailable))
                .is_empty()
        );
    }

    #[test]
    fn pane_monitor_records_each_success_before_the_next_publication() {
        let first = test_session("first", "%7", Some("100.1"), "C1", "U1");
        let second = test_session("second", "%8", Some("100.2"), "C1", "U1");
        let mut known = vec![first.clone(), second];

        record_unavailable(&mut known, &first);

        assert!(!known[0].available);
        assert_eq!(SessionStatus::Unavailable, known[0].status);
        assert!(known[1].available);
    }
}

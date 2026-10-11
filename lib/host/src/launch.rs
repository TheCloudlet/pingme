use pingme_session::{AgentSession, PanePlacement, SessionStatus};

use crate::services::LocalServices;

pub fn pane_placement(local_operator_launch: bool, inside_tmux: bool) -> PanePlacement {
    match (local_operator_launch, inside_tmux) {
        (true, true) => PanePlacement::ReuseCurrentPane,
        (true, false) => PanePlacement::DetachedSession { attach: true },
        (false, true) => PanePlacement::DetachedWindow,
        (false, false) => PanePlacement::DetachedSession { attach: false },
    }
}

pub struct StartedSession {
    pub session: AgentSession,
    pub status_warning: Option<String>,
    pub registration_error: Option<String>,
}

/// Spawns an Agent CLI for a Session whose Session Thread already exists.
pub fn start_agent_session(
    session: AgentSession,
    services: &mut dyn LocalServices,
) -> Result<StartedSession, String> {
    let mut session = services.spawn_agent(session)?;
    session.status = SessionStatus::Idle;
    let status_warning = services
        .update_session_thread(&session)
        .err()
        .map(|error| format!("failed to update Slack Session Thread: {error}\n"));
    let registration_error = services.register_session(&session).err().map(|error| {
        format!(
            "failed to start Slack daemon: {error}\nSlack control is unavailable for this session\n"
        )
    });
    Ok(StartedSession {
        session,
        status_warning,
        registration_error,
    })
}

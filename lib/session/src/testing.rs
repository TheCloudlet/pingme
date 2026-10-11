use crate::{AgentSession, PanePlacement, SessionStatus};

pub fn test_session(
    name: &str,
    pane_id: &str,
    thread_ts: Option<&str>,
    control_channel: &str,
    operator: &str,
) -> AgentSession {
    let session = AgentSession::for_launch(
        name,
        "codex",
        "linux",
        "project",
        "/work/project",
        control_channel,
        operator,
        PanePlacement::DetachedWindow,
    );
    let mut session = session;
    if let Some(thread_ts) = thread_ts {
        session.thread_ts = Some(thread_ts.to_owned());
    }
    session.pane_id = Some(pane_id.to_owned());
    session.tmux_session = Some("bridge".to_owned());
    session.tmux_window = Some("bridge:3".to_owned());
    session.notify_socket = Some("/tmp/bridge.sock".to_owned());
    session.notify_token = Some("secret".to_owned());
    session.status = SessionStatus::Idle;
    session
}

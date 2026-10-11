use pingme_agent::agent_command;
use pingme_session::{AgentSession, PanePlacement};
use pingme_support::tmux::tmux;

use crate::pane::current_tmux_pane;

pub fn spawn_agent_tui(session: AgentSession) -> Result<AgentSession, String> {
    // Leave the pane unpublished until write_to_registry; Session Name is the last field.
    match session
        .placement
        .ok_or_else(|| "launch Session has no pane placement".to_owned())?
    {
        PanePlacement::ReuseCurrentPane => {
            let pane_id = current_tmux_pane()
                .ok_or_else(|| "current terminal is not the configured TMUX_PANE".to_owned())?;
            let mut session = session;
            session.pane_id = Some(pane_id);
            Ok(session)
        }
        PanePlacement::DetachedWindow => {
            let pane_id = tmux(&[
                "new-window",
                "-d",
                "-P",
                "-F",
                "#{pane_id}\t#{window_id}",
                "-n",
                &session.session_name,
                "-c",
                &session.cwd,
                &agent_command(&session),
            ])?;
            let (pane_id, window_id) = parse_tmux_window(&pane_id)?;
            let pane_id = pane_id.to_owned();
            tmux(&["set-option", "-w", "-t", &pane_id, "remain-on-exit", "on"])?;
            let mut session = session;
            session.pane_id = Some(pane_id);
            session.tmux_window = Some(window_id.to_owned());
            Ok(session)
        }
        PanePlacement::DetachedSession { .. } => {
            let pane_id = tmux(&[
                "new-session",
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "-s",
                &session.session_name,
                "-n",
                &session.agent_cli,
                "-c",
                &session.cwd,
                &agent_command(&session),
            ])?;
            let pane_id = pane_id.trim().to_owned();
            tmux(&["set-option", "-w", "-t", &pane_id, "remain-on-exit", "on"])?;
            let tmux_session = session.session_name.clone();
            let mut session = session;
            session.pane_id = Some(pane_id);
            session.tmux_session = Some(tmux_session);
            Ok(session)
        }
    }
}

fn parse_tmux_window(output: &str) -> Result<(&str, &str), String> {
    output
        .trim()
        .split_once('\t')
        .ok_or_else(|| "tmux new-window did not return pane and window IDs".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_new_window_output_keeps_pane_and_window_targets_distinct() {
        assert_eq!(("%7", "@3"), parse_tmux_window("%7\t@3\n").unwrap());
    }
}

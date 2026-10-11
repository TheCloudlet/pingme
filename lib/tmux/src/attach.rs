use std::process::{Command, Stdio};

use pingme_agent::agent_command;
use pingme_session::{AgentSession, PanePlacement};
use pingme_support::shell::shell_quote;

pub fn attach_or_switch_agent_tui(session: &AgentSession) -> Result<(), String> {
    match session.placement {
        Some(PanePlacement::DetachedSession { attach: true }) => attach_tmux_session(
            session
                .tmux_session
                .as_deref()
                .unwrap_or(&session.session_name),
        ),
        Some(PanePlacement::ReuseCurrentPane) => run_agent_in_current_pane(session),
        Some(PanePlacement::DetachedWindow | PanePlacement::DetachedSession { attach: false })
        | None => Ok(()),
    }
}

fn run_agent_in_current_pane(session: &AgentSession) -> Result<(), String> {
    let pane_id = session.pane()?;
    let command = agent_command(session);
    let ran = Command::new("sh")
        .arg("-c")
        .arg(current_pane_shell_command(pane_id, &command))
        .current_dir(&session.cwd)
        .status();
    // The shell is still this pane's process; leftover metadata would keep Slack driving it.
    let _ = AgentSession::clear_registry_name(pane_id);
    ran.map(|_| ())
        .map_err(|error| format!("failed to start Agent CLI: {error}"))
}

fn current_pane_shell_command(pane_id: &str, command: &str) -> String {
    // Keep a shell so EXIT still unsets pane metadata if the Agent CLI is interrupted.
    let without_exec = command.replace("; exec ", "; ");
    let without_exec = without_exec
        .strip_prefix("exec ")
        .map(str::to_owned)
        .unwrap_or(without_exec);
    format!(
        "trap {} EXIT; {}",
        shell_quote(&AgentSession::clear_registry_name_command(pane_id)),
        without_exec
    )
}

pub fn attach_tmux_session(target: &str) -> Result<(), String> {
    let status = Command::new("tmux")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .args(["attach-session", "-t", target])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| format!("failed to attach tmux session: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("tmux attach-session exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_pane_shell_keeps_wait_for_and_unsets_metadata_on_exit() {
        let command = current_pane_shell_command(
            "%7",
            "tmux wait-for 'pingme-start-secret'; exec env PINGME_SESSION_NAME='codex-pingme' codex",
        );

        assert_eq!(
            format!(
                "trap {} EXIT; tmux wait-for 'pingme-start-secret'; env PINGME_SESSION_NAME='codex-pingme' codex",
                shell_quote(&AgentSession::clear_registry_name_command("%7"))
            ),
            command
        );
    }
}

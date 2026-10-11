//! Codex CLI and Grok Build: launch commands, completion hooks, and turn
//! notifications.

use std::hash::{DefaultHasher, Hash, Hasher};

use pingme_session::AgentSession;
use pingme_support::shell::shell_quote;

use crate::codex::codex_command;
use crate::grok::grok_command;
pub mod codex;
pub mod grok;
pub mod turn;

/// The Agent CLIs pingme can launch, as typed after `pingme` or `/pingme`.
pub const AGENT_CLIS: [&str; 2] = ["codex", "grok"];

pub fn agent_command(session: &AgentSession) -> String {
    let notify_socket_path = session.notify_socket.as_deref();
    let notify_token = session.notify_token.as_deref();
    let command = if session.agent_cli == "grok" {
        grok_command(session, notify_socket_path, notify_token)
    } else {
        codex_command(session, notify_socket_path, notify_token)
    };
    let command = append_agent_args(command, &session.agent_args);
    match notify_token {
        Some(token) => format!(
            "tmux wait-for {}; {command}",
            shell_quote(&agent_start_channel(token))
        ),
        None => command,
    }
}

fn append_agent_args(mut command: String, args: &[String]) -> String {
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    command
}

pub fn agent_start_channel(token: &str) -> String {
    format!("pingme-start-{token}")
}

pub fn prompt_fingerprint(prompt: &str) -> String {
    let mut hasher = DefaultHasher::new();
    prompt.trim().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use pingme_session::{AgentSession, PanePlacement};

    use super::*;

    #[test]
    fn bridged_agent_waits_for_session_registration_before_starting() {
        let session = AgentSession::for_launch(
            "codex-pingme",
            "codex",
            "linux",
            "project",
            "/work/pingme",
            "C1",
            "U1",
            PanePlacement::DetachedWindow,
        );
        let mut session = session;
        session.thread_ts = Some("100.1".to_owned());
        session.notify_socket = Some("/tmp/pingme.sock".to_owned());
        session.notify_token = Some("secret".to_owned());
        let command = agent_command(&session);

        assert!(command.starts_with("tmux wait-for 'pingme-start-secret'; exec env "));
    }

    #[test]
    fn grok_launch_command_appends_resume_args() {
        let mut session = AgentSession::for_launch(
            "grok-pingme",
            "grok",
            "linux",
            "project",
            "/work/pingme",
            "C1",
            "U1",
            PanePlacement::DetachedWindow,
        );
        session.thread_ts = Some("100.1".to_owned());
        session.agent_args = vec![
            "--resume".to_owned(),
            "01a090da-d3ba-7bb1-8cc0-aab11beccab6".to_owned(),
        ];
        session.notify_socket = Some("/tmp/pingme.sock".to_owned());
        session.notify_token = Some("secret".to_owned());
        let command = agent_command(&session);

        assert!(
            command.ends_with(" grok '--resume' '01a090da-d3ba-7bb1-8cc0-aab11beccab6'"),
            "{command}"
        );
        assert!(command.starts_with("tmux wait-for 'pingme-start-secret'; exec env "));
    }

    #[test]
    fn unbridged_grok_launch_command_appends_resume_args() {
        let mut session = AgentSession::for_launch(
            "grok-unbridged",
            "grok",
            "",
            "unknown",
            "/work/pingme",
            "",
            "",
            PanePlacement::ReuseCurrentPane,
        );
        session.agent_args = vec![
            "--resume".to_owned(),
            "01a090da-d3ba-7bb1-8cc0-aab11beccab6".to_owned(),
        ];
        let command = agent_command(&session);

        assert_eq!(
            "exec grok '--resume' '01a090da-d3ba-7bb1-8cc0-aab11beccab6'",
            command
        );
    }

    #[test]
    fn unbridged_codex_launch_command_forwards_trailing_args() {
        let mut session = AgentSession::for_launch(
            "codex-pingme",
            "codex",
            "linux",
            "project",
            "/work/pingme",
            "C1",
            "U1",
            PanePlacement::DetachedWindow,
        );
        session.agent_args = vec!["resume".to_owned(), "session-id".to_owned()];
        let command = agent_command(&session);

        assert_eq!("exec codex 'resume' 'session-id'", command);
    }

    #[test]
    fn bridged_codex_forwards_trailing_args_after_notify() {
        let mut session = AgentSession::for_launch(
            "codex-pingme",
            "codex",
            "linux",
            "project",
            "/work/pingme",
            "C1",
            "U1",
            PanePlacement::DetachedWindow,
        );
        session.thread_ts = Some("100.1".to_owned());
        session.agent_args = vec!["resume".to_owned(), "session-id".to_owned()];
        session.notify_socket = Some("/tmp/pingme.sock".to_owned());
        session.notify_token = Some("secret".to_owned());
        let command = agent_command(&session);

        assert!(command.contains(" codex -c "), "{command}");
        assert!(command.ends_with(" 'resume' 'session-id'"), "{command}");
        assert!(command.starts_with("tmux wait-for 'pingme-start-secret'; exec env "));
    }
}

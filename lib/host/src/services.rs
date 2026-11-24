use std::env;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use pingme_agent::agent_start_channel;
use pingme_agent::grok::install_grok_hook;
use pingme_session::AgentSession;
use pingme_slack::api::{
    delete_thread_root, delete_thread_root_at, list_session_thread_roots, post_session_thread_root,
    slack_ok, update_status_card,
};
use pingme_slack::probe::{ControlChannelProbe, control_channel_probe};
use pingme_support::tmux::tmux;
use pingme_tmux::attach::attach_tmux_session;
use pingme_tmux::spawn::spawn_agent_tui;
use serde_json::Value;

use crate::socket::{ensure_host_daemon, host_notify_socket_path};

pub trait LocalServices {
    fn available_session_name(&mut self, base: &str) -> Result<String, String>;
    fn create_session_thread(&mut self, session: AgentSession) -> Result<AgentSession, String>;
    fn update_session_thread(&mut self, session: &AgentSession) -> Result<(), String>;
    fn spawn_agent(&mut self, session: AgentSession) -> Result<AgentSession, String>;
    /// Register an already-spawned Agent Session and ensure the Host daemon is running.
    /// Failure leaves the local Agent CLI usable without Slack control.
    fn register_session(&mut self, session: &AgentSession) -> Result<(), String>;
    fn list_sessions(&mut self) -> Result<Vec<AgentSession>, String>;
    fn attach_session(&mut self, session_name: &str) -> Result<(), String>;
    fn cleanup_sessions(&mut self, channel_id: &str) -> Result<usize, String>;
    /// Bot token scopes reported by Slack `auth.test`.
    fn bot_token_scopes(&mut self) -> Result<Vec<String>, String>;
    fn probe_app_token(&mut self, app_token: &str) -> Result<(), String>;
    fn probe_control_channel(&mut self, channel_id: &str) -> Result<ControlChannelProbe, String>;
    fn project_dir_exists(&mut self, cwd: &str) -> bool;
    fn host_daemon_running(&mut self) -> bool;
}

pub struct RealServices {
    bot_token: String,
}

impl RealServices {
    pub fn new(bot_token: String) -> Self {
        Self { bot_token }
    }
}

impl LocalServices for RealServices {
    fn available_session_name(&mut self, base: &str) -> Result<String, String> {
        available_session_name(base)
    }

    fn create_session_thread(&mut self, session: AgentSession) -> Result<AgentSession, String> {
        ensure_host_daemon()?;
        let ts = post_session_thread_root(&self.bot_token, &session)?;
        let mut session = session;
        session.thread_ts = Some(ts);
        Ok(session)
    }

    fn update_session_thread(&mut self, session: &AgentSession) -> Result<(), String> {
        update_status_card(&self.bot_token, session)
    }

    fn spawn_agent(&mut self, mut session: AgentSession) -> Result<AgentSession, String> {
        if session.agent_cli == "grok" {
            install_grok_hook()?;
        }
        // A bridged Agent CLI reports finished turns to the Host daemon's notify socket.
        if let Some(thread_ts) = &session.thread_ts {
            session.notify_socket = Some(host_notify_socket_path()?);
            session.notify_token = Some(format!(
                "{}-{}-{thread_ts}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_nanos())
                    .unwrap_or_default()
            ));
        }
        let session = spawn_agent_tui(session)?;
        if session.thread_ts.is_none() {
            session.write_to_registry()?;
        }
        Ok(session)
    }

    fn register_session(&mut self, session: &AgentSession) -> Result<(), String> {
        let registered = session
            .write_to_registry()
            .and_then(|_| ensure_host_daemon());
        if let Some(token) = session.notify_token.as_deref() {
            let signaled = tmux(&["wait-for", "-S", &agent_start_channel(token)]);
            if registered.is_ok() {
                signaled?;
            }
        }
        registered
    }

    fn list_sessions(&mut self) -> Result<Vec<AgentSession>, String> {
        AgentSession::list_from_registry()
    }

    fn attach_session(&mut self, session_name: &str) -> Result<(), String> {
        let session = AgentSession::list_from_registry()?
            .into_iter()
            .find(|session| session.session_name == session_name)
            .ok_or_else(|| "unknown session".to_owned())?;
        if !session.available {
            return Err("session is unavailable".to_owned());
        }
        if env::var_os("TMUX").is_some() {
            tmux(&["select-window", "-t", session.tmux_window()?])?;
            tmux(&["select-pane", "-t", session.pane()?]).map(drop)
        } else {
            tmux(&["select-pane", "-t", session.pane()?])?;
            tmux(&["select-window", "-t", session.tmux_window()?])?;
            attach_tmux_session(session.tmux_session()?)
        }
    }

    fn cleanup_sessions(&mut self, channel_id: &str) -> Result<usize, String> {
        cleanup_agent_sessions(&self.bot_token, channel_id)
    }

    fn bot_token_scopes(&mut self) -> Result<Vec<String>, String> {
        let response = reqwest::blocking::Client::new()
            .post("https://slack.com/api/auth.test")
            .bearer_auth(&self.bot_token)
            .send()
            .map_err(|error| format!("Slack auth.test failed: {error}"))?;
        let scopes = response
            .headers()
            .get("x-oauth-scopes")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .split(',')
            .map(|scope| scope.trim().to_owned())
            .filter(|scope| !scope.is_empty())
            .collect();
        let body: Value = response
            .json()
            .map_err(|error| format!("Slack auth.test returned invalid JSON: {error}"))?;
        slack_ok(&body)?;
        Ok(scopes)
    }

    fn probe_app_token(&mut self, app_token: &str) -> Result<(), String> {
        // Asks for a Socket Mode URL without connecting, so the daemon's connection is untouched.
        let body: Value = reqwest::blocking::Client::new()
            .post("https://slack.com/api/apps.connections.open")
            .bearer_auth(app_token)
            .send()
            .map_err(|error| format!("Slack apps.connections.open failed: {error}"))?
            .json()
            .map_err(|error| {
                format!("Slack apps.connections.open returned invalid JSON: {error}")
            })?;
        slack_ok(&body)
    }

    fn probe_control_channel(&mut self, channel_id: &str) -> Result<ControlChannelProbe, String> {
        let body: Value = reqwest::blocking::Client::new()
            .get("https://slack.com/api/conversations.info")
            .bearer_auth(&self.bot_token)
            .query(&[("channel", channel_id)])
            .send()
            .map_err(|error| format!("Slack conversations.info failed: {error}"))?
            .json()
            .map_err(|error| format!("Slack conversations.info returned invalid JSON: {error}"))?;
        control_channel_probe(&body)
    }

    fn project_dir_exists(&mut self, cwd: &str) -> bool {
        Path::new(cwd).is_dir()
    }

    fn host_daemon_running(&mut self) -> bool {
        host_notify_socket_path().is_ok_and(|path| StdUnixStream::connect(path).is_ok())
    }
}

pub fn available_session_name(base: &str) -> Result<String, String> {
    if !AgentSession::name_exists(base)? {
        return Ok(base.to_owned());
    }
    for suffix in 2..100 {
        let candidate = format!("{base}-{suffix}");
        if !AgentSession::name_exists(&candidate)? {
            return Ok(candidate);
        }
    }
    Err(format!("no available session name for {base}"))
}

fn cleanup_unavailable_sessions(
    sessions: impl IntoIterator<Item = AgentSession>,
    mut delete_thread_root: impl FnMut(&AgentSession) -> Result<(), String>,
    mut remove_local_mapping: impl FnMut(&AgentSession) -> Result<(), String>,
) -> Result<usize, String> {
    let mut cleaned = 0;
    for session in sessions.into_iter().filter(|session| !session.available) {
        if session.thread_ts.is_some() {
            delete_thread_root(&session)?;
        }
        remove_local_mapping(&session)?;
        cleaned += 1;
    }
    Ok(cleaned)
}

fn cleanup_agent_sessions(bot_token: &str, channel_id: &str) -> Result<usize, String> {
    let sessions = AgentSession::list_from_registry()?;
    let mut cleaned = cleanup_unavailable_sessions(
        sessions.iter().cloned(),
        |session| delete_thread_root(bot_token, session),
        |session| tmux(&["kill-window", "-t", session.tmux_window()?]).map(drop),
    )?;
    cleaned += reconcile_slack_session_threads(bot_token, channel_id, &sessions)?;
    Ok(cleaned)
}

pub(crate) fn reconcile_slack_session_threads(
    bot_token: &str,
    channel_id: &str,
    sessions: &[AgentSession],
) -> Result<usize, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before Unix epoch: {error}"))?
        .as_secs();
    reconcile_session_thread_roots(
        sessions,
        channel_id,
        now,
        |channel| list_session_thread_roots(bot_token, channel),
        |channel, ts| delete_thread_root_at(bot_token, channel, ts),
    )
}

pub(crate) fn reconcile_session_thread_roots(
    sessions: &[AgentSession],
    channel_id: &str,
    now: u64,
    list_roots: impl FnOnce(&str) -> Result<Vec<String>, String>,
    mut delete_root: impl FnMut(&str, &str) -> Result<(), String>,
) -> Result<usize, String> {
    let roots = list_roots(channel_id)?;
    let mut cleaned = 0;
    let mut errors = Vec::new();
    for ts in orphaned_session_thread_roots(&roots, sessions, now) {
        match delete_root(channel_id, ts) {
            Ok(()) => cleaned += 1,
            Err(error) => errors.push(format!("{ts}: {error}")),
        }
    }
    if errors.is_empty() {
        Ok(cleaned)
    } else {
        Err(errors.join("; "))
    }
}

fn orphaned_session_thread_roots<'a>(
    roots: &'a [String],
    sessions: &[AgentSession],
    now: u64,
) -> Vec<&'a str> {
    const REGISTRATION_GRACE_SECS: u64 = 600;
    roots
        .iter()
        .filter(|ts| {
            let created = ts
                .split_once('.')
                .and_then(|(seconds, _)| seconds.parse::<u64>().ok());
            match sessions
                .iter()
                .find(|session| session.thread_ts.as_ref() == Some(ts))
            {
                Some(session) => !session.available,
                None => created
                    .is_some_and(|created| now.saturating_sub(created) > REGISTRATION_GRACE_SECS),
            }
        })
        .map(String::as_str)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use pingme_session::testing::test_session;
    use pingme_slack::api::slack_delete_ok;

    use super::*;

    #[test]
    fn cleanup_deletes_session_thread_root_before_local_mapping() {
        let mut session = test_session("codex-project", "%7", Some("100.1"), "C1", "U1");
        session.mark_unavailable();
        let events = RefCell::new(Vec::new());

        let cleaned = cleanup_unavailable_sessions(
            vec![session],
            |session| {
                events.borrow_mut().push(format!(
                    "delete-root:{}:{}",
                    session.control_channel,
                    session.thread_ts.as_deref().unwrap_or_default()
                ));
                Ok(())
            },
            |session| {
                events.borrow_mut().push(format!(
                    "remove-mapping:{}",
                    session.tmux_window.as_deref().unwrap_or_default()
                ));
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(1, cleaned);
        assert_eq!(
            vec!["delete-root:C1:100.1", "remove-mapping:bridge:3"],
            events.into_inner()
        );
    }

    #[test]
    fn cleanup_accepts_an_already_missing_session_thread_root() {
        let mut session = test_session("codex-project", "%7", Some("100.1"), "C1", "U1");
        session.mark_unavailable();
        let removed = RefCell::new(Vec::new());

        let cleaned = cleanup_unavailable_sessions(
            vec![session],
            |_| {
                slack_delete_ok(&serde_json::json!({
                    "ok": false,
                    "error": "message_not_found"
                }))
            },
            |session| {
                removed
                    .borrow_mut()
                    .push(session.tmux_window.clone().unwrap_or_default());
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(1, cleaned);
        assert_eq!(vec!["bridge:3"], removed.into_inner());
    }

    #[test]
    fn cleanup_preserves_local_mapping_when_thread_root_deletion_fails() {
        let mut session = test_session("codex-project", "%7", Some("100.1"), "C1", "U1");
        session.mark_unavailable();
        let removed = RefCell::new(Vec::new());

        let result = cleanup_unavailable_sessions(
            vec![session],
            |_| Err("Slack API error: ratelimited".to_owned()),
            |session| {
                removed
                    .borrow_mut()
                    .push(session.tmux_window.clone().unwrap_or_default());
                Ok(())
            },
        );

        assert_eq!(Err("Slack API error: ratelimited".to_owned()), result);
        assert!(removed.into_inner().is_empty());
    }

    #[test]
    fn orphan_cleanup_preserves_live_and_recent_roots() {
        let now = 1_000_000;
        let live = test_session("codex-live", "%7", Some("100.1"), "C1", "U1");
        let roots = vec![
            "100.1".to_owned(),
            "900000.2".to_owned(),
            "999900.3".to_owned(),
        ];

        assert_eq!(
            vec!["900000.2"],
            orphaned_session_thread_roots(&roots, &[live], now)
        );
    }

    #[test]
    fn session_root_reconciliation_retries_after_slack_failure() {
        let now = 1_000_000;
        let mut dead = test_session("codex-ended", "%7", Some("999999.1"), "C1", "U1");
        dead.mark_unavailable();
        let live = test_session("codex-live", "%8", Some("100.1"), "C1", "U1");
        let sessions = [dead, live];
        let roots = vec![
            "999999.1".to_owned(),
            "900000.2".to_owned(),
            "100.1".to_owned(),
            "999900.3".to_owned(),
        ];
        let deleted = RefCell::new(Vec::new());

        let failed = reconcile_session_thread_roots(
            &sessions,
            "C1",
            now,
            |_| Ok(roots.clone()),
            |_, ts| Err(format!("Slack chat.delete failed for {ts}")),
        );
        assert_eq!(
            Err("999999.1: Slack chat.delete failed for 999999.1; 900000.2: Slack chat.delete failed for 900000.2".to_owned()),
            failed
        );

        let cleaned = reconcile_session_thread_roots(
            &sessions,
            "C1",
            now,
            |_| Ok(roots.clone()),
            |channel, ts| {
                deleted.borrow_mut().push(format!("{channel}:{ts}"));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(2, cleaned);
        assert_eq!(vec!["C1:999999.1", "C1:900000.2"], deleted.into_inner());
    }

    #[test]
    fn session_root_reconciliation_continues_after_one_delete_fails() {
        let roots = vec!["900000.1".to_owned(), "900000.2".to_owned()];
        let deleted = RefCell::new(Vec::new());

        let result = reconcile_session_thread_roots(
            &[],
            "C1",
            1_000_000,
            |_| Ok(roots),
            |_, ts| {
                if ts == "900000.1" {
                    return Err("Slack API error: cant_delete_message".to_owned());
                }
                deleted.borrow_mut().push(ts.to_owned());
                Ok(())
            },
        );

        assert_eq!(
            Err("900000.1: Slack API error: cant_delete_message".to_owned()),
            result
        );
        assert_eq!(vec!["900000.2"], deleted.into_inner());
    }
}

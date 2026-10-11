//! The Host daemon: one per Host. Handles Slack envelopes, Agent CLI turn
//! notifications, and ended panes.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::process::ExitCode;
use std::{env, fs};

use pingme_session::{AgentSession, SessionStatus};
use pingme_setting::{HostSettings, setting_path};
use pingme_slack::api::update_status_card;
use pingme_slack::socket::SlackSocketMode;
use serde_json::Value;
use tokio::net::UnixListener;
use tokio::task::block_in_place;
use tokio::time::MissedTickBehavior;

use crate::daemon::control::handle_control_command;
use crate::daemon::monitor::monitor_agent_sessions;
use crate::daemon::notify::{handle_host_notification, read_agent_notification};
use crate::daemon::runtime::LiveSessions;
use crate::daemon::thread::handle_slack_event;
use crate::socket::host_notify_socket_path;

mod control;
pub mod monitor;
pub mod notify;
mod runtime;
mod thread;

pub fn run_daemon_process() -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("failed to start async runtime: {error}");
            return ExitCode::from(2);
        }
    };

    match runtime.block_on(run_host_daemon()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

async fn run_host_daemon() -> Result<(), String> {
    let setting = read_file_setting()?;
    let app_token = env::var("SLACK_APP_TOKEN")
        .map_err(|_| "missing environment variable SLACK_APP_TOKEN".to_owned())?;
    let bot_token = env::var("SLACK_BOT_TOKEN")
        .map_err(|_| "missing environment variable SLACK_BOT_TOKEN".to_owned())?;
    // Handlers below are synchronous (blocking Slack Web API and tmux calls);
    // only the Slack socket and the notify socket are async.
    block_in_place(|| recover_agent_sessions(&bot_token))?;
    let notify_socket_path = host_notify_socket_path()?;
    let _daemon_lock = try_lock_host_daemon(&notify_socket_path)?;
    if StdUnixStream::connect(&notify_socket_path).is_ok() {
        return Err("Host daemon is already running".to_owned());
    }
    let _ = fs::remove_file(&notify_socket_path);
    let notify_listener = UnixListener::bind(&notify_socket_path)
        .map_err(|error| format!("failed to bind notify socket: {error}"))?;
    fs::set_permissions(&notify_socket_path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("failed to protect notify socket: {error}"))?;

    // Notify socket is the Host's local presence. Slack Socket Mode connects
    // in the background so a downed Slack handshake cannot take the daemon
    // (or Agent CLI notify) down with it.
    let mut slack = SlackSocketMode::connect(reqwest::Client::new(), app_token);
    let mut known_sessions = block_in_place(AgentSession::list_from_registry)?;
    let mut live = LiveSessions::default();
    let mut pane_poll = tokio::time::interval(std::time::Duration::from_millis(1_000));
    pane_poll.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            incoming = slack.recv() => {
                let Some(envelope) = incoming else {
                    eprintln!("Slack Socket Mode driver ended ({:?})", slack.state());
                    return Ok(());
                };
                let started_at = slack.epoch();
                if let Err(error) = block_in_place(|| {
                    handle_envelope(
                        &bot_token,
                        &setting,
                        started_at,
                        &envelope,
                        &mut live,
                    )
                }) {
                    eprintln!("failed to handle Slack envelope: {error}");
                }
            }
            accepted = notify_listener.accept() => {
                let (stream, _) = accepted
                    .map_err(|error| format!("failed to accept Agent CLI notification: {error}"))?;
                let notification = match read_agent_notification(stream).await {
                    Ok(Some(notification)) => notification,
                    Ok(None) => continue,
                    Err(error) => {
                        eprintln!("rejected Agent CLI notification: {error}");
                        continue;
                    }
                };
                if let Err(error) = block_in_place(|| {
                    handle_host_notification(
                        &bot_token,
                        &setting,
                        &notification,
                        &mut live,
                    )
                }) {
                    eprintln!("failed to handle Agent CLI notification: {error}");
                }
            }
            _ = pane_poll.tick() => {
                if let Err(error) = block_in_place(|| {
                    monitor_agent_sessions(
                        &bot_token,
                        &mut known_sessions,
                        &mut live,
                    )
                }) {
                    eprintln!("failed to monitor Agent Sessions: {error}");
                }
            }
        }
    }
}

/// Socket Mode envelopes are typed; anything but a slash command or an event is not ours.
fn handle_envelope(
    bot_token: &str,
    setting: &HostSettings,
    started_at: f64,
    envelope: &Value,
    live: &mut LiveSessions,
) -> Result<(), String> {
    match envelope.get("type").and_then(Value::as_str) {
        Some("slash_commands") => handle_control_command(bot_token, setting, envelope),
        Some("events_api") => handle_slack_event(bot_token, setting, started_at, envelope, live),
        _ => Ok(()),
    }
}

fn try_lock_host_daemon(socket_path: &str) -> Result<File, String> {
    let daemon_lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(format!("{socket_path}.lock"))
        .map_err(|error| format!("failed to open Host daemon lock: {error}"))?;
    daemon_lock
        .try_lock()
        .map_err(|_| "Host daemon is already running".to_owned())?;
    Ok(daemon_lock)
}

fn read_file_setting() -> Result<HostSettings, String> {
    HostSettings::parse(
        &fs::read_to_string(setting_path())
            .map_err(|error| format!("failed to read setting.toml: {error}"))?,
    )
}

/// Marks Session Threads whose panes died while the daemon was down.
fn recover_agent_sessions(bot_token: &str) -> Result<(), String> {
    for mut session in AgentSession::list_from_registry()? {
        if !session.available && session.thread_ts.is_some() {
            session.status = SessionStatus::Unavailable;
            update_status_card(bot_token, &session)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_daemon_lock_rejects_a_second_owner() {
        let socket_path = env::temp_dir()
            .join(format!("pingme-lock-test-{}", std::process::id()))
            .display()
            .to_string();
        let first = try_lock_host_daemon(&socket_path).unwrap();

        assert!(try_lock_host_daemon(&socket_path).is_err());

        drop(first);
        let _ = fs::remove_file(format!("{socket_path}.lock"));
    }
}

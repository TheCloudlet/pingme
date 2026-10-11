use std::env;
use std::io::Write;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::process::ExitCode;

use pingme_agent::turn::{FinishedTurn, finished_turn};
use pingme_routing::update::{session_update_after_agent_turn, session_update_turn};
use pingme_session::{AgentSession, SessionStatus};
use pingme_setting::HostSettings;
use pingme_slack::api::{
    post_session_text, post_thread_reply, update_status_card, update_thread_reply,
};
use serde_json::Value;
use tokio::io::AsyncReadExt;

use super::runtime::{EndedTurn, LiveSessions};

pub fn run_notify_process(args: &[String]) -> ExitCode {
    match notify_daemon(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

fn notify_daemon(args: &[String]) -> Result<(), String> {
    let socket_path = env::var("PINGME_NOTIFY_SOCKET")
        .map_err(|_| "missing environment variable PINGME_NOTIFY_SOCKET".to_owned())?;
    let token = env::var("PINGME_NOTIFY_TOKEN")
        .map_err(|_| "missing environment variable PINGME_NOTIFY_TOKEN".to_owned())?;
    let agent_cli = args.get(2).map(String::as_str).unwrap_or("codex");
    let notification = if agent_cli == "grok" {
        let mut input = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
            .map_err(|error| format!("failed to read Grok notification: {error}"))?;
        input
    } else {
        args.get(2)
            .ok_or_else(|| "notify payload is missing".to_owned())?
            .to_owned()
    };
    let mut stream = StdUnixStream::connect(socket_path)
        .map_err(|error| format!("failed to connect daemon notify socket: {error}"))?;
    stream
        .write_all(
            serde_json::json!({ "token": token, "agent_cli": agent_cli, "notification": notification })
                .to_string()
                .as_bytes(),
        )
        .map_err(|error| format!("failed to write daemon notification: {error}"))
}

pub(crate) async fn read_agent_notification(
    stream: tokio::net::UnixStream,
) -> Result<Option<Value>, String> {
    const MAX_NOTIFICATION_BYTES: usize = 1_048_576;
    let mut input = Vec::new();
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        stream
            .take((MAX_NOTIFICATION_BYTES + 1) as u64)
            .read_to_end(&mut input),
    )
    .await
    .map_err(|_| "notification timed out".to_owned())?
    .map_err(|error| format!("failed to read notification: {error}"))?;
    if read > MAX_NOTIFICATION_BYTES {
        return Err("notification exceeded 1 MiB".to_owned());
    }
    if input.is_empty() {
        return Ok(None);
    }
    serde_json::from_slice(&input)
        .map(Some)
        .map_err(|error| format!("notification was invalid JSON: {error}"))
}

pub(crate) fn handle_host_notification(
    bot_token: &str,
    setting: &HostSettings,
    notification: &Value,
    live: &mut LiveSessions,
) -> Result<(), String> {
    let Some(token) = notification.get("token").and_then(Value::as_str) else {
        return Ok(());
    };
    let sessions = AgentSession::list_from_registry()?;
    let Some(session) = sessions.iter().find(|session| {
        session.control_channel == setting.slack.control_channel_id
            && session.notify_token.as_deref() == Some(token)
    }) else {
        return Ok(());
    };
    let mut session = session.clone();
    if let Err(error) = handle_agent_notification(bot_token, &mut session, notification, live) {
        let _ = post_thread_reply(bot_token, &session, &format!("Bridge error: {error}"));
        return Err(error);
    }
    Ok(())
}

fn handle_agent_notification(
    bot_token: &str,
    session: &mut AgentSession,
    notification: &Value,
    live: &mut LiveSessions,
) -> Result<(), String> {
    if notification.get("token").and_then(Value::as_str) != session.notify_token.as_deref() {
        return Ok(());
    }
    let Some(raw_notification) = notification.get("notification").and_then(Value::as_str) else {
        return Ok(());
    };
    let notification: Value = serde_json::from_str(raw_notification).map_err(|error| {
        format!(
            "invalid {} notification payload JSON: {error}",
            session.agent_cli
        )
    })?;
    let Some(turn) = finished_turn(session, &notification) else {
        return Ok(());
    };

    let pane = session.pane()?.to_owned();
    let ended = live.finish_turn(&pane);
    // Taken before anything can fail, so a queued prompt never waits behind an Idle Session.
    let queued = live.take_queued(&pane);
    session.write_last_slack_prompt(None)?;
    session.write_mirrored_terminal_count(turn.terminal_input_count)?;
    session.write_status(SessionStatus::Idle)?;
    let posted = post_turn(bot_token, session, &turn, ended);
    if let Some(prompt) = queued {
        super::thread::submit_prompt(bot_token, session, prompt, &[], live)?;
    }
    posted
}

fn post_turn(
    bot_token: &str,
    session: &AgentSession,
    turn: &FinishedTurn<'_>,
    ended: Option<EndedTurn>,
) -> Result<(), String> {
    update_status_card(bot_token, session)?;
    if let Some(ended) = ended
        && let Some(ts) = ended.receipt_ts
    {
        let text = session_update_turn(&session.agent_cli, "✓ done", ended.elapsed);
        let _ = update_thread_reply(bot_token, session, &ts, &text);
    }
    for prompt in &turn.terminal_prompts {
        post_session_text(
            bot_token,
            session,
            &format!("*You (terminal)*\n{prompt}"),
            "terminal.txt",
        )?;
    }
    post_session_text(
        bot_token,
        session,
        &session_update_after_agent_turn(&session.agent_cli, turn.response),
        "answer.txt",
    )
}

use std::path::Path;

use pingme_agent::prompt_fingerprint;
use pingme_routing::photo::{
    SlackFile, accepted_slack_photos, prompt_with_local_photos, slack_photo_store_path,
};
use pingme_routing::thread::{
    ThreadAction, ThreadMessage, apply_host_routing_policy, route_thread_message,
};
use pingme_routing::update::{
    screen_block, session_update_after_prompt_paste, session_update_queued, session_update_reject,
    session_update_turn,
};
use pingme_session::{AgentSession, SessionStatus};
use pingme_setting::HostSettings;
use pingme_slack::api::SLACK_TEXT_LIMIT;
use pingme_slack::envelope::thread_message_from_envelope;
use serde_json::Value;

use super::effects::{Effects, reply};
use super::runtime::LiveSessions;

pub(crate) fn handle_slack_event(
    host: &mut impl Effects,
    setting: &HostSettings,
    started_at: f64,
    envelope: &Value,
    live: &mut LiveSessions,
) -> Result<(), String> {
    let Some(message) = thread_message_from_envelope(envelope) else {
        return Ok(());
    };
    let sessions = host.sessions()?;
    let Some(routed) = route_thread_message(&message, &sessions, &setting.slack) else {
        return Ok(());
    };
    let queued = routed
        .session
        .pane_id
        .as_deref()
        .and_then(|pane| live.queued(pane))
        .map(str::to_owned);
    let action = apply_host_routing_policy(
        &message,
        routed.session,
        routed.action,
        started_at,
        queued.as_deref(),
    );
    let mut session = routed.session.clone();
    if let Err(error) = act_on_thread_message(host, &mut session, &message, action, live) {
        let _ = reply(host, &session, &format!("Bridge error: {error}"));
        return Err(error);
    }
    Ok(())
}

fn act_on_thread_message(
    host: &mut impl Effects,
    session: &mut AgentSession,
    message: &ThreadMessage<'_>,
    action: ThreadAction,
    live: &mut LiveSessions,
) -> Result<(), String> {
    match action {
        ThreadAction::Prompt(prompt) => submit_prompt(host, session, prompt, &message.files, live),
        ThreadAction::Enqueue { prompt, replaced } => {
            if !host.alive(session.pane()?) {
                return mark_unavailable(host, session, live);
            }
            let prompt = match materialize_thread_prompt(host, session, prompt, &message.files) {
                Ok(prompt) => prompt,
                Err(error) => return reply(host, session, &format!("Not executed: {error}")),
            };
            let pane = session.pane()?.to_owned();
            live.enqueue(&pane, prompt);
            reply(
                host,
                session,
                &session_update_queued(&session.agent_cli, replaced),
            )
        }
        ThreadAction::Stop => {
            if !host.alive(session.pane()?) {
                return mark_unavailable(host, session, live);
            }
            let pane = session.pane()?.to_owned();
            let cleared = live.take_queued(&pane).is_some();
            if let Some(ended) = live.finish_turn(&pane, host.now())
                && let Some(ts) = ended.receipt_ts
            {
                let text = session_update_turn(&session.agent_cli, "■ stopped", ended.elapsed);
                let _ = host.edit(session, &ts, &text);
            }
            host.keys(&pane, "Escape")?;
            session.status = SessionStatus::Idle;
            host.save(session)?;
            let mut text = "Stop sent (Escape).".to_owned();
            if cleared {
                text.push_str(" Queued prompt cleared.");
            }
            reply(host, session, &text)
        }
        ThreadAction::Screen => {
            if !host.alive(session.pane()?) {
                return mark_unavailable(host, session, live);
            }
            let captured = host.capture(session.pane()?)?;
            reply(host, session, &screen_block(&captured, SLACK_TEXT_LIMIT))
        }
        ThreadAction::Keys(keys) => {
            if !host.alive(session.pane()?) {
                return mark_unavailable(host, session, live);
            }
            host.keys(session.pane()?, &keys)?;
            reply(host, session, "Keys sent.")
        }
        ThreadAction::MarkUnavailable => mark_unavailable(host, session, live),
        ThreadAction::Name => reply(
            host,
            session,
            &format!("Session Name: {}", session.session_name),
        ),
        ThreadAction::Rename(name) => {
            if host
                .sessions()?
                .iter()
                .any(|session| session.session_name == name)
            {
                return reply(
                    host,
                    session,
                    &format!("Not renamed: `{name}` already exists."),
                );
            }
            if !host.alive(session.pane()?) {
                return mark_unavailable(host, session, live);
            }
            host.rename(session, &name)?;
            host.paste(session.pane()?, &format!("/rename {name}"))?;
            host.save(session)?;
            reply(host, session, &format!("Renamed to `{name}`."))
        }
        ThreadAction::Reject(reason) => reply(host, session, &reason),
        ThreadAction::Ignore => Ok(()),
    }
}

pub(crate) fn submit_prompt(
    host: &mut impl Effects,
    session: &mut AgentSession,
    prompt: String,
    files: &[SlackFile],
    live: &mut LiveSessions,
) -> Result<(), String> {
    if !host.alive(session.pane()?) {
        return mark_unavailable(host, session, live);
    }
    let prompt = match materialize_thread_prompt(host, session, prompt, files) {
        Ok(prompt) => prompt,
        Err(error) => return reply(host, session, &format!("Not executed: {error}")),
    };
    session.last_slack_prompt = Some(prompt_fingerprint(&prompt));
    session.status = SessionStatus::Busy;
    if let Err(error) = host.save(session) {
        session.last_slack_prompt = None;
        session.status = SessionStatus::Idle;
        let _ = host.save(session);
        return reply(
            host,
            session,
            &format!("Not executed: failed to mark session busy: {error}"),
        );
    }
    let pane = session.pane()?.to_owned();
    let pasted = host.paste(&pane, &prompt);
    if pasted.is_err() {
        session.last_slack_prompt = None;
        session.status = SessionStatus::Idle;
        let _ = host.save(session);
    }
    let receipt = session_update_after_prompt_paste(pasted.clone(), &session.agent_cli);
    let posted = host.post(session, &receipt);
    if pasted.is_err() {
        return posted.map(drop);
    }
    // The receipt becomes the turn's status line; without it the turn still watches the pane.
    let receipt_ts = posted
        .inspect_err(|error| eprintln!("failed to post prompt receipt: {error}"))
        .ok();
    let screen = host.capture(&pane).unwrap_or_default();
    let now = host.now();
    live.begin_turn(&pane, now, &screen, receipt_ts);
    Ok(())
}

fn materialize_thread_prompt(
    host: &mut impl Effects,
    session: &AgentSession,
    prompt: String,
    files: &[SlackFile],
) -> Result<String, String> {
    if files.is_empty() {
        return Ok(prompt);
    }
    let photos = accepted_slack_photos(files)?;
    let mut paths = Vec::new();
    for photo in photos {
        let dest = slack_photo_store_path(session.thread()?, photo)?;
        host.download(Path::new(&dest), photo)?;
        paths.push(dest);
    }
    Ok(prompt_with_local_photos(&prompt, &paths))
}

pub fn mark_unavailable(
    host: &mut impl Effects,
    session: &mut AgentSession,
    live: &mut LiveSessions,
) -> Result<(), String> {
    if let Ok(pane) = session.pane() {
        live.drop_pane(pane);
    }
    session.status = SessionStatus::Unavailable;
    host.save(session)?;
    reply(
        host,
        session,
        &session_update_reject(&session.agent_cli, "unavailable"),
    )
}

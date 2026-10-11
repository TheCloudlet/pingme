use std::path::Path;
use std::time::SystemTime;

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
use pingme_slack::api::{
    SLACK_TEXT_LIMIT, download_slack_photo, post_thread_reply, post_thread_reply_ts,
    update_status_card, update_thread_reply,
};
use pingme_slack::envelope::thread_message_from_envelope;
use pingme_support::tmux::tmux;
use pingme_tmux::pane::{capture_pane, pane_available, paste_into_pane, send_keys};
use serde_json::Value;

use super::runtime::LiveSessions;

pub(crate) fn handle_slack_event(
    bot_token: &str,
    setting: &HostSettings,
    started_at: f64,
    envelope: &Value,
    live: &mut LiveSessions,
) -> Result<(), String> {
    let Some(message) = thread_message_from_envelope(envelope) else {
        return Ok(());
    };
    let sessions = AgentSession::list_from_registry()?;
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
    if let Err(error) = act_on_thread_message(bot_token, &mut session, &message, action, live) {
        let _ = post_thread_reply(bot_token, &session, &format!("Bridge error: {error}"));
        return Err(error);
    }
    Ok(())
}

fn act_on_thread_message(
    bot_token: &str,
    session: &mut AgentSession,
    message: &ThreadMessage<'_>,
    action: ThreadAction,
    live: &mut LiveSessions,
) -> Result<(), String> {
    match action {
        ThreadAction::Prompt(prompt) => {
            submit_prompt(bot_token, session, prompt, &message.files, live)
        }
        ThreadAction::Enqueue { prompt, replaced } => {
            if !pane_available(session.pane()?) {
                return mark_unavailable(bot_token, session, live);
            }
            let prompt = match materialize_thread_prompt(bot_token, session, prompt, &message.files)
            {
                Ok(prompt) => prompt,
                Err(error) => {
                    return post_thread_reply(
                        bot_token,
                        session,
                        &format!("Not executed: {error}"),
                    );
                }
            };
            let pane = session.pane()?.to_owned();
            live.enqueue(&pane, prompt);
            post_thread_reply(
                bot_token,
                session,
                &session_update_queued(&session.agent_cli, replaced),
            )
        }
        ThreadAction::Stop => {
            if !pane_available(session.pane()?) {
                return mark_unavailable(bot_token, session, live);
            }
            let pane = session.pane()?.to_owned();
            let cleared = live.take_queued(&pane).is_some();
            if let Some(ended) = live.finish_turn(&pane)
                && let Some(ts) = ended.receipt_ts
            {
                let text = session_update_turn(&session.agent_cli, "■ stopped", ended.elapsed);
                let _ = update_thread_reply(bot_token, session, &ts, &text);
            }
            tmux(&["send-keys", "-t", &pane, "Escape"])?;
            session.write_status(SessionStatus::Idle)?;
            update_status_card(bot_token, session)?;
            let mut text = "Stop sent (Escape).".to_owned();
            if cleared {
                text.push_str(" Queued prompt cleared.");
            }
            post_thread_reply(bot_token, session, &text)
        }
        ThreadAction::Screen => {
            if !pane_available(session.pane()?) {
                return mark_unavailable(bot_token, session, live);
            }
            let captured = capture_pane(session.pane()?)?;
            post_thread_reply(
                bot_token,
                session,
                &screen_block(&captured, SLACK_TEXT_LIMIT),
            )
        }
        ThreadAction::Keys(keys) => {
            if !pane_available(session.pane()?) {
                return mark_unavailable(bot_token, session, live);
            }
            send_keys(session.pane()?, &keys)?;
            post_thread_reply(bot_token, session, "Keys sent.")
        }
        ThreadAction::MarkUnavailable => mark_unavailable(bot_token, session, live),
        ThreadAction::Name => post_thread_reply(
            bot_token,
            session,
            &format!("Session Name: {}", session.session_name),
        ),
        ThreadAction::Rename(name) => {
            if AgentSession::name_exists(&name)? {
                return post_thread_reply(
                    bot_token,
                    session,
                    &format!("Not renamed: `{name}` already exists."),
                );
            }
            if !pane_available(session.pane()?) {
                return mark_unavailable(bot_token, session, live);
            }
            rename_session(bot_token, session, &name)?;
            post_thread_reply(bot_token, session, &format!("Renamed to `{name}`."))
        }
        ThreadAction::Reject(reason) => post_thread_reply(bot_token, session, &reason),
        ThreadAction::Ignore => Ok(()),
    }
}

pub(crate) fn submit_prompt(
    bot_token: &str,
    session: &mut AgentSession,
    prompt: String,
    files: &[SlackFile],
    live: &mut LiveSessions,
) -> Result<(), String> {
    if !pane_available(session.pane()?) {
        return mark_unavailable(bot_token, session, live);
    }
    let prompt = match materialize_thread_prompt(bot_token, session, prompt, files) {
        Ok(prompt) => prompt,
        Err(error) => {
            return post_thread_reply(bot_token, session, &format!("Not executed: {error}"));
        }
    };
    session.write_last_slack_prompt(Some(prompt_fingerprint(&prompt)))?;
    if let Err(error) = session.write_status(SessionStatus::Busy) {
        let _ = session.write_last_slack_prompt(None);
        return post_thread_reply(
            bot_token,
            session,
            &format!("Not executed: failed to mark session busy: {error}"),
        );
    }
    if let Err(error) = update_status_card(bot_token, session) {
        let _ = session.write_last_slack_prompt(None);
        session.write_status(SessionStatus::Idle)?;
        return post_thread_reply(
            bot_token,
            session,
            &format!("Not executed: failed to mark session busy: {error}"),
        );
    }
    let pane = session.pane()?.to_owned();
    let pasted = paste_into_pane(&pane, &prompt);
    if pasted.is_err() {
        let _ = session.write_last_slack_prompt(None);
        session.write_status(SessionStatus::Idle)?;
        let _ = update_status_card(bot_token, session);
    }
    let receipt = session_update_after_prompt_paste(pasted.clone(), &session.agent_cli);
    let posted = post_thread_reply_ts(bot_token, session, &receipt);
    if pasted.is_err() {
        return posted.map(drop);
    }
    // The receipt becomes the turn's status line; without it the turn still watches the pane.
    let receipt_ts = posted
        .inspect_err(|error| eprintln!("failed to post prompt receipt: {error}"))
        .ok();
    let screen = capture_pane(&pane).unwrap_or_default();
    live.begin_turn(&pane, SystemTime::now(), &screen, receipt_ts);
    Ok(())
}

fn materialize_thread_prompt(
    bot_token: &str,
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
        download_slack_photo(bot_token, Path::new(&dest), photo)?;
        paths.push(dest);
    }
    Ok(prompt_with_local_photos(&prompt, &paths))
}

pub fn mark_unavailable(
    bot_token: &str,
    session: &mut AgentSession,
    live: &mut LiveSessions,
) -> Result<(), String> {
    if let Ok(pane) = session.pane() {
        live.drop_pane(pane);
    }
    session.write_status(SessionStatus::Unavailable)?;
    update_status_card(bot_token, session)?;
    post_thread_reply(
        bot_token,
        session,
        &session_update_reject(&session.agent_cli, "unavailable"),
    )
}

fn rename_session(bot_token: &str, session: &mut AgentSession, name: &str) -> Result<(), String> {
    tmux(&["rename-window", "-t", session.pane()?, name])?;
    session.write_name(name)?;
    paste_into_pane(session.pane()?, &format!("/rename {name}"))?;
    update_status_card(bot_token, session)
}

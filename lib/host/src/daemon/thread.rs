use std::path::Path;

use pingme_agent::prompt_fingerprint;
use pingme_routing::photo::{
    SlackFile, accepted_slack_photos, prompt_with_local_photos, slack_photo_store_path,
};
use pingme_routing::thread::{
    ThreadAction, ThreadMessage, apply_host_routing_policy, route_thread_message,
};
use pingme_routing::update::{session_update_after_prompt_paste, session_update_reject};
use pingme_session::{AgentSession, SessionStatus};
use pingme_setting::HostSettings;
use pingme_slack::api::{download_slack_photo, post_thread_reply, update_status_card};
use pingme_slack::envelope::thread_message_from_envelope;
use pingme_support::tmux::tmux;
use pingme_tmux::pane::{pane_available, paste_into_pane};
use serde_json::Value;

pub(crate) fn handle_slack_event(
    bot_token: &str,
    setting: &HostSettings,
    started_at: f64,
    envelope: &Value,
) -> Result<(), String> {
    let Some(message) = thread_message_from_envelope(envelope) else {
        return Ok(());
    };
    let sessions = AgentSession::list_from_registry()?;
    let Some(routed) = route_thread_message(&message, &sessions, &setting.slack) else {
        return Ok(());
    };
    let action = apply_host_routing_policy(&message, routed.session, routed.action, started_at);
    let mut session = routed.session.clone();
    if let Err(error) = act_on_thread_message(bot_token, &mut session, &message, action) {
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
) -> Result<(), String> {
    match action {
        ThreadAction::Prompt(prompt) => {
            if !pane_available(session.pane()?) {
                return mark_unavailable(bot_token, session);
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
            let pasted = paste_into_pane(session.pane()?, &prompt);
            if pasted.is_err() {
                let _ = session.write_last_slack_prompt(None);
                session.write_status(SessionStatus::Idle)?;
                let _ = update_status_card(bot_token, session);
            }
            let receipt = session_update_after_prompt_paste(pasted.clone(), &session.agent_cli);
            match post_thread_reply(bot_token, session, &receipt) {
                Err(error) if pasted.is_ok() => {
                    eprintln!("failed to post prompt receipt: {error}");
                    Ok(())
                }
                posted => posted,
            }
        }
        ThreadAction::Stop => {
            if !pane_available(session.pane()?) {
                return mark_unavailable(bot_token, session);
            }
            tmux(&["send-keys", "-t", session.pane()?, "Escape"])?;
            session.write_status(SessionStatus::Idle)?;
            update_status_card(bot_token, session)?;
            post_thread_reply(bot_token, session, "Stop sent (Escape).")
        }
        ThreadAction::MarkUnavailable => mark_unavailable(bot_token, session),
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
                return mark_unavailable(bot_token, session);
            }
            rename_session(bot_token, session, &name)?;
            post_thread_reply(bot_token, session, &format!("Renamed to `{name}`."))
        }
        ThreadAction::Reject(reason) => post_thread_reply(bot_token, session, &reason),
        ThreadAction::Ignore => Ok(()),
    }
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

pub fn mark_unavailable(bot_token: &str, session: &mut AgentSession) -> Result<(), String> {
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

use std::fs;
use std::path::Path;

use pingme_routing::photo::{MAX_SLACK_PHOTO_BYTES, SlackFile, slack_file_url_is_downloadable};
use pingme_routing::update::render_session_status_card;
use pingme_session::{AgentSession, SessionStatus};
use serde_json::Value;

/// Calls a Slack Web API `chat.*` method, which takes a JSON body.
pub fn slack_chat(bot_token: &str, method: &str, body: Value) -> Result<Value, String> {
    reqwest::blocking::Client::new()
        .post(format!("https://slack.com/api/{method}"))
        .bearer_auth(bot_token)
        .json(&body)
        .send()
        .map_err(|error| format!("Slack {method} failed: {error}"))?
        .json()
        .map_err(|error| format!("Slack {method} returned invalid JSON: {error}"))
}

pub fn slack_ok(response: &Value) -> Result<(), String> {
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(format!(
            "Slack API error: {}",
            response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown_error")
        ))
    }
}

pub fn slack_delete_ok(response: &Value) -> Result<(), String> {
    if response.get("error").and_then(Value::as_str) == Some("message_not_found") {
        Ok(())
    } else {
        slack_ok(response)
    }
}

pub fn post_thread_reply(
    bot_token: &str,
    session: &AgentSession,
    text: &str,
) -> Result<(), String> {
    slack_ok(&slack_chat(
        bot_token,
        "chat.postMessage",
        serde_json::json!({
            "channel": &session.control_channel,
            "thread_ts": session.thread()?,
            "text": text,
        }),
    )?)
}

pub fn update_status_card(bot_token: &str, session: &AgentSession) -> Result<(), String> {
    slack_ok(&slack_chat(
        bot_token,
        "chat.update",
        serde_json::json!({
            "channel": &session.control_channel,
            "ts": session.thread()?,
            "text": render_session_status_card(session),
        }),
    )?)
}

pub fn update_status_card_to(
    bot_token: &str,
    mut session: AgentSession,
    status: SessionStatus,
) -> Result<(), String> {
    session.status = status;
    update_status_card(bot_token, &session)
}

pub fn delete_thread_root(bot_token: &str, session: &AgentSession) -> Result<(), String> {
    slack_delete_ok(&slack_chat(
        bot_token,
        "chat.delete",
        serde_json::json!({
            "channel": &session.control_channel,
            "ts": session.thread()?,
        }),
    )?)
}

pub fn post_control_response(bot_token: &str, payload: &Value, text: &str) -> Result<(), String> {
    if let Some(response_url) = payload.get("response_url").and_then(Value::as_str) {
        reqwest::blocking::Client::new()
            .post(response_url)
            .json(&serde_json::json!({ "response_type": "ephemeral", "text": text }))
            .send()
            .map_err(|error| format!("Slack response_url post failed: {error}"))?;
        return Ok(());
    }
    let channel_id = payload
        .get("channel_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "Slack command payload omitted channel_id".to_owned())?;
    slack_ok(&slack_chat(
        bot_token,
        "chat.postMessage",
        serde_json::json!({ "channel": channel_id, "text": text }),
    )?)
}

pub fn download_slack_photo(bot_token: &str, dest: &Path, photo: &SlackFile) -> Result<(), String> {
    if !slack_file_url_is_downloadable(&photo.download_url) {
        return Err("photo URL is not a Slack file URL".to_owned());
    }
    if dest.exists() {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create photo directory: {error}"))?;
    }
    let response = reqwest::blocking::Client::new()
        .get(&photo.download_url)
        .bearer_auth(bot_token)
        .send()
        .map_err(|error| format!("failed to download photo: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Slack photo download returned {}",
            response.status()
        ));
    }
    let bytes = response
        .bytes()
        .map_err(|error| format!("failed to read photo: {error}"))?;
    if bytes.len() as u64 > MAX_SLACK_PHOTO_BYTES {
        return Err("photo exceeds 20 MiB".to_owned());
    }
    fs::write(dest, bytes).map_err(|error| format!("failed to store photo: {error}"))
}

pub fn slack_text(text: &str) -> String {
    const LIMIT: usize = 3_500;
    if text.chars().count() <= LIMIT {
        text.to_owned()
    } else {
        format!(
            "{}\n[truncated by pingme]",
            text.chars().take(LIMIT).collect::<String>()
        )
    }
}

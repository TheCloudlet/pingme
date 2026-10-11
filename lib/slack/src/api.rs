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
    post_thread_reply_ts(bot_token, session, text).map(drop)
}

pub fn post_thread_reply_ts(
    bot_token: &str,
    session: &AgentSession,
    text: &str,
) -> Result<String, String> {
    let response = slack_chat(
        bot_token,
        "chat.postMessage",
        serde_json::json!({
            "channel": &session.control_channel,
            "thread_ts": session.thread()?,
            "text": text,
        }),
    )?;
    slack_ok(&response)?;
    response["ts"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "Slack chat.postMessage response did not include ts".to_owned())
}

pub fn update_thread_reply(
    bot_token: &str,
    session: &AgentSession,
    ts: &str,
    text: &str,
) -> Result<(), String> {
    slack_ok(&slack_chat(
        bot_token,
        "chat.update",
        serde_json::json!({
            "channel": &session.control_channel,
            "ts": ts,
            "text": text,
        }),
    )?)
}

/// Posts `text`, and when it does not fit, uploads the full text in the same thread.
pub fn post_session_text(
    bot_token: &str,
    session: &AgentSession,
    text: &str,
    filename: &str,
) -> Result<(), String> {
    let outbound = outbound_text(text);
    post_thread_reply(bot_token, session, &outbound.message)?;
    if let Some(body) = outbound.file_body {
        upload_thread_file(bot_token, session, filename, &body)?;
    }
    Ok(())
}

fn upload_thread_file(
    bot_token: &str,
    session: &AgentSession,
    filename: &str,
    body: &str,
) -> Result<(), String> {
    let bytes = body.as_bytes();
    let granted = slack_form(
        bot_token,
        "files.getUploadURLExternal",
        &[("filename", filename), ("length", &bytes.len().to_string())],
    )?;
    slack_ok(&granted)?;
    let upload_url = granted["upload_url"]
        .as_str()
        .ok_or_else(|| "Slack files.getUploadURLExternal omitted upload_url".to_owned())?;
    let file_id = granted["file_id"]
        .as_str()
        .ok_or_else(|| "Slack files.getUploadURLExternal omitted file_id".to_owned())?;
    let uploaded = reqwest::blocking::Client::new()
        .post(upload_url)
        .body(bytes.to_vec())
        .send()
        .map_err(|error| format!("Slack file upload failed: {error}"))?;
    if !uploaded.status().is_success() {
        return Err(format!("Slack file upload returned {}", uploaded.status()));
    }
    slack_ok(&slack_chat(
        bot_token,
        "files.completeUploadExternal",
        serde_json::json!({
            "files": [{ "id": file_id, "title": filename }],
            "channel_id": &session.control_channel,
            "thread_ts": session.thread()?,
        }),
    )?)
}

fn slack_form(bot_token: &str, method: &str, form: &[(&str, &str)]) -> Result<Value, String> {
    reqwest::blocking::Client::new()
        .post(format!("https://slack.com/api/{method}"))
        .bearer_auth(bot_token)
        .form(&form)
        .send()
        .map_err(|error| format!("Slack {method} failed: {error}"))?
        .json()
        .map_err(|error| format!("Slack {method} returned invalid JSON: {error}"))
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

pub const SLACK_TEXT_LIMIT: usize = 3_500;

pub struct OutboundText {
    pub message: String,
    pub file_body: Option<String>,
}

/// The Slack message, plus the original text when it does not fit in one message.
pub fn outbound_text(text: &str) -> OutboundText {
    if text.chars().count() <= SLACK_TEXT_LIMIT {
        return OutboundText {
            message: text.to_owned(),
            file_body: None,
        };
    }
    let note = "\n\nFull text is attached.";
    let budget = SLACK_TEXT_LIMIT - note.chars().count();
    let mut taken = String::new();
    for line in text.split_inclusive('\n') {
        let next = taken.chars().count() + line.chars().count();
        if next > budget {
            break;
        }
        taken.push_str(line);
    }
    if taken.is_empty() {
        taken = text.chars().take(budget).collect();
    }
    let taken = taken.trim_end_matches('\n');
    OutboundText {
        message: format!("{taken}{note}"),
        file_body: Some(text.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_the_whole_message() {
        let outbound = outbound_text("hello");
        assert_eq!("hello", outbound.message);
        assert!(outbound.file_body.is_none());
    }

    #[test]
    fn long_text_keeps_the_opening_and_the_full_body() {
        let text = "line\n".repeat(1_000);
        let outbound = outbound_text(&text);
        assert!(outbound.message.contains("Full text is attached."));
        assert!(outbound.message.chars().count() <= SLACK_TEXT_LIMIT);
        assert!(outbound.message.starts_with("line\n"));
        assert!(!outbound.message.contains("[truncated by pingme]"));
        assert_eq!(Some(text), outbound.file_body);
    }
}

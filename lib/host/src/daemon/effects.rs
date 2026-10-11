//! What the daemon's thread, turn, and progress handlers read (the clock, the
//! registry, the pane) and do (save, paste, keys, post, edit, upload). They
//! touch tmux and Slack only through this, so a test can replay what the phone
//! sends and read back, in order, what it would see.

use std::path::Path;
use std::time::SystemTime;

use pingme_routing::photo::SlackFile;
use pingme_session::AgentSession;
use pingme_slack::api::{
    download_slack_photo, outbound_text, post_thread_reply_ts, update_status_card,
    update_thread_reply, upload_thread_file,
};
use pingme_support::tmux::tmux;
use pingme_tmux::pane::{capture_pane, pane_available, paste_into_pane, send_keys};

pub(crate) trait Effects {
    fn now(&mut self) -> SystemTime;
    fn sessions(&mut self) -> Result<Vec<AgentSession>, String>;
    /// Writes the Session's turn state to the registry, then redraws its status card.
    fn save(&mut self, session: &AgentSession) -> Result<(), String>;
    fn alive(&mut self, pane: &str) -> bool;
    fn capture(&mut self, pane: &str) -> Result<String, String>;
    fn paste(&mut self, pane: &str, text: &str) -> Result<(), String>;
    fn keys(&mut self, pane: &str, keys: &str) -> Result<(), String>;
    /// Renames the tmux window and the Session in the registry.
    fn rename(&mut self, session: &mut AgentSession, name: &str) -> Result<(), String>;
    /// Posts in the Session Thread and returns the message ts.
    fn post(&mut self, session: &AgentSession, text: &str) -> Result<String, String>;
    fn edit(&mut self, session: &AgentSession, ts: &str, text: &str) -> Result<(), String>;
    fn upload(&mut self, session: &AgentSession, filename: &str, body: &str) -> Result<(), String>;
    fn download(&mut self, dest: &Path, photo: &SlackFile) -> Result<(), String>;
}

pub(crate) fn reply(
    host: &mut impl Effects,
    session: &AgentSession,
    text: &str,
) -> Result<(), String> {
    host.post(session, text).map(drop)
}

/// Posts `text`, and when it does not fit, uploads the full text in the same thread.
pub(crate) fn post_long(
    host: &mut impl Effects,
    session: &AgentSession,
    text: &str,
    filename: &str,
) -> Result<(), String> {
    let outbound = outbound_text(text);
    host.post(session, &outbound.message)?;
    if let Some(body) = outbound.file_body {
        host.upload(session, filename, &body)?;
    }
    Ok(())
}

pub(crate) struct TmuxSlack<'a> {
    pub bot_token: &'a str,
}

impl Effects for TmuxSlack<'_> {
    fn now(&mut self) -> SystemTime {
        SystemTime::now()
    }

    fn sessions(&mut self) -> Result<Vec<AgentSession>, String> {
        AgentSession::list_from_registry()
    }

    fn save(&mut self, session: &AgentSession) -> Result<(), String> {
        session.write_turn_state()?;
        update_status_card(self.bot_token, session)
    }

    fn alive(&mut self, pane: &str) -> bool {
        pane_available(pane)
    }

    fn capture(&mut self, pane: &str) -> Result<String, String> {
        capture_pane(pane)
    }

    fn paste(&mut self, pane: &str, text: &str) -> Result<(), String> {
        paste_into_pane(pane, text)
    }

    fn keys(&mut self, pane: &str, keys: &str) -> Result<(), String> {
        send_keys(pane, keys)
    }

    fn rename(&mut self, session: &mut AgentSession, name: &str) -> Result<(), String> {
        tmux(&["rename-window", "-t", session.pane()?, name])?;
        session.write_name(name)
    }

    fn post(&mut self, session: &AgentSession, text: &str) -> Result<String, String> {
        post_thread_reply_ts(self.bot_token, session, text)
    }

    fn edit(&mut self, session: &AgentSession, ts: &str, text: &str) -> Result<(), String> {
        update_thread_reply(self.bot_token, session, ts, text)
    }

    fn upload(&mut self, session: &AgentSession, filename: &str, body: &str) -> Result<(), String> {
        upload_thread_file(self.bot_token, session, filename, body)
    }

    fn download(&mut self, dest: &Path, photo: &SlackFile) -> Result<(), String> {
        download_slack_photo(self.bot_token, dest, photo)
    }
}

//! What the phone sees, in order, for what the phone sends. Each story replays
//! a recorded Slack envelope and a Grok Stop notification through the daemon
//! handlers, with tmux and Slack replaced by a log.

use std::path::Path;
use std::time::{Duration, SystemTime};

use pingme_routing::photo::SlackFile;
use pingme_session::testing::test_session;
use pingme_session::{AgentSession, SessionStatus};
use pingme_setting::HostSettings;
use serde_json::Value;

use super::effects::Effects;
use super::monitor::refresh_busy_progress;
use super::notify::handle_host_notification;
use super::runtime::LiveSessions;
use super::thread::handle_slack_event;
use Effect::*;

#[derive(Debug, PartialEq, Eq)]
enum Effect {
    Paste(String),
    Keys(String),
    Post(String),
    Edit(String, String),
    Upload(String, String),
    Status(SessionStatus),
}

struct Phone {
    sessions: Vec<AgentSession>,
    clock: SystemTime,
    screen: String,
    posted: usize,
    /// Posts starting with this fail, as a Slack outage would.
    failing_post: Option<&'static str>,
    log: Vec<Effect>,
    live: LiveSessions,
    setting: HostSettings,
}

impl Phone {
    fn new() -> Self {
        let mut session = test_session(
            "grok-project",
            "%7",
            Some("1757221923.123456"),
            "C_CONTROL",
            "U_OPERATOR",
        );
        session.agent_cli = "grok".to_owned();
        Self {
            sessions: vec![session],
            clock: SystemTime::UNIX_EPOCH + Duration::from_secs(1_757_221_924),
            screen: String::new(),
            posted: 0,
            failing_post: None,
            log: Vec::new(),
            live: LiveSessions::default(),
            setting: HostSettings::parse(
                "[host]\nname = \"linux\"\n[slack]\ncontrol_channel_id = \"C_CONTROL\"\noperator_id = \"U_OPERATOR\"\n",
            )
            .unwrap(),
        }
    }

    /// The Operator types `text` in the Session Thread.
    fn send(&mut self, text: &str) -> Result<(), String> {
        let mut envelope: Value = serde_json::from_str(include_str!(
            "../../../slack/tests/fixtures/slack-message-envelopes.json"
        ))
        .unwrap();
        let mut envelope = envelope["operator"].take();
        envelope["payload"]["event"]["text"] = text.into();
        let mut live = std::mem::take(&mut self.live);
        let setting = self.setting.clone();
        let handled = handle_slack_event(self, &setting, 0.0, &envelope, &mut live);
        self.live = live;
        handled
    }

    /// Grok's Stop hook fires `after` the last event with `answer`.
    fn finish(&mut self, after: Duration, answer: &str) -> Result<(), String> {
        self.clock += after;
        let stop = serde_json::json!({
            "hookEventName": "Stop",
            "reason": "end_turn",
            "lastAssistantMessage": answer,
        });
        let notification = serde_json::json!({
            "token": "secret",
            "agent_cli": "grok",
            "notification": stop.to_string(),
        });
        let mut live = std::mem::take(&mut self.live);
        let setting = self.setting.clone();
        let handled = handle_host_notification(self, &setting, &notification, &mut live);
        self.live = live;
        handled
    }

    /// One daemon pane poll, `after` the last event, with `screen` in the pane.
    fn poll(&mut self, after: Duration, screen: &str) {
        self.clock += after;
        self.screen = screen.to_owned();
        let sessions = self.sessions.clone();
        let mut live = std::mem::take(&mut self.live);
        refresh_busy_progress(self, &sessions, &mut live);
        self.live = live;
    }

    fn seen(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.log)
    }
}

impl Effects for Phone {
    fn now(&mut self) -> SystemTime {
        self.clock
    }

    fn sessions(&mut self) -> Result<Vec<AgentSession>, String> {
        Ok(self.sessions.clone())
    }

    fn save(&mut self, session: &AgentSession) -> Result<(), String> {
        for saved in &mut self.sessions {
            if saved.pane_id == session.pane_id {
                *saved = session.clone();
            }
        }
        self.log.push(Status(session.status));
        Ok(())
    }

    fn alive(&mut self, _pane: &str) -> bool {
        true
    }

    fn capture(&mut self, _pane: &str) -> Result<String, String> {
        Ok(self.screen.clone())
    }

    fn paste(&mut self, _pane: &str, text: &str) -> Result<(), String> {
        self.log.push(Paste(text.to_owned()));
        Ok(())
    }

    fn keys(&mut self, _pane: &str, keys: &str) -> Result<(), String> {
        self.log.push(Keys(keys.to_owned()));
        Ok(())
    }

    fn rename(&mut self, session: &mut AgentSession, name: &str) -> Result<(), String> {
        session.session_name = name.to_owned();
        Ok(())
    }

    fn post(&mut self, _session: &AgentSession, text: &str) -> Result<String, String> {
        if self
            .failing_post
            .is_some_and(|prefix| text.starts_with(prefix))
        {
            return Err("Slack API error: ratelimited".to_owned());
        }
        self.posted += 1;
        self.log.push(Post(text.to_owned()));
        Ok(format!("ts{}", self.posted))
    }

    fn edit(&mut self, _session: &AgentSession, ts: &str, text: &str) -> Result<(), String> {
        self.log.push(Edit(ts.to_owned(), text.to_owned()));
        Ok(())
    }

    fn upload(
        &mut self,
        _session: &AgentSession,
        filename: &str,
        body: &str,
    ) -> Result<(), String> {
        self.log.push(Upload(filename.to_owned(), body.to_owned()));
        Ok(())
    }

    fn download(&mut self, _dest: &Path, _photo: &SlackFile) -> Result<(), String> {
        Err("no photos in these stories".to_owned())
    }
}

fn post(text: &str) -> Effect {
    Post(text.to_owned())
}

fn paste(text: &str) -> Effect {
    Paste(text.to_owned())
}

fn edit(ts: &str, text: &str) -> Effect {
    Edit(ts.to_owned(), text.to_owned())
}

#[test]
fn a_prompt_while_idle_is_pasted_once_and_answered_in_the_thread() {
    let mut phone = Phone::new();

    phone.send("inspect the failure").unwrap();
    assert_eq!(
        vec![
            Status(SessionStatus::Busy),
            paste("inspect the failure"),
            post("(ping grok)"),
        ],
        phone.seen()
    );

    // Grok's spinner turns every poll: the receipt is edited, nothing new is posted.
    for (poll, spinner) in ["⠴", "⠙", "⠴", "⠙", "⠴"].iter().enumerate() {
        let after = if poll == 0 { 20 } else { 25 };
        phone.poll(
            Duration::from_secs(after),
            &format!("{spinner} Reading the CI log… {poll}.0s"),
        );
    }
    assert_eq!(
        vec![
            edit("ts1", "(ping grok) ⏳ working 20s"),
            edit("ts1", "(ping grok) ⏳ working 45s"),
            edit("ts1", "(ping grok) ⏳ working 1m"),
            edit("ts1", "(ping grok) ⏳ working 1m"),
            edit("ts1", "(ping grok) ⏳ working 2m"),
        ],
        phone.seen()
    );

    phone
        .finish(Duration::from_secs(30), "The test fails on CI only.")
        .unwrap();
    assert_eq!(
        vec![
            Status(SessionStatus::Idle),
            edit("ts1", "(ping grok) ✓ done 2m"),
            post("(pong grok)\nThe test fails on CI only."),
        ],
        phone.seen()
    );
}

#[test]
fn a_prompt_while_busy_waits_for_the_turn_to_end() {
    let mut phone = Phone::new();
    phone.send("inspect the failure").unwrap();
    phone.seen();

    phone.send("then fix it").unwrap();
    assert_eq!(vec![post("(queued grok)")], phone.seen());

    phone.finish(Duration::from_secs(30), "Found it.").unwrap();
    assert_eq!(
        vec![
            Status(SessionStatus::Idle),
            edit("ts1", "(ping grok) ✓ done 30s"),
            post("(pong grok)\nFound it."),
            Status(SessionStatus::Busy),
            paste("then fix it"),
            post("(ping grok)"),
        ],
        phone.seen()
    );
}

#[test]
fn a_failed_answer_post_still_runs_the_queued_prompt() {
    let mut phone = Phone::new();
    phone.send("inspect the failure").unwrap();
    phone.send("then fix it").unwrap();
    phone.seen();
    phone.failing_post = Some("(pong");

    assert!(phone.finish(Duration::from_secs(30), "Found it.").is_err());
    assert_eq!(
        vec![
            Status(SessionStatus::Idle),
            edit("ts1", "(ping grok) ✓ done 30s"),
            Status(SessionStatus::Busy),
            paste("then fix it"),
            post("(ping grok)"),
            post("Bridge error: Slack API error: ratelimited"),
        ],
        phone.seen()
    );
}

#[test]
fn a_second_waiting_prompt_replaces_the_first() {
    let mut phone = Phone::new();
    phone.send("inspect the failure").unwrap();
    phone.seen();

    phone.send("then fix it").unwrap();
    phone.send("no, just explain it").unwrap();
    assert_eq!(
        vec![
            post("(queued grok)"),
            post("(queued grok) replaced the waiting prompt"),
        ],
        phone.seen()
    );

    phone.finish(Duration::from_secs(30), "Found it.").unwrap();
    let pasted: Vec<Effect> = phone
        .seen()
        .into_iter()
        .filter(|effect| matches!(effect, Paste(_)))
        .collect();
    assert_eq!(vec![paste("no, just explain it")], pasted);
}

#[test]
fn a_long_answer_keeps_its_opening_and_attaches_the_full_text() {
    let mut phone = Phone::new();
    phone.send("inspect the failure").unwrap();
    phone.seen();
    let answer = "line\n".repeat(1_000);

    phone.finish(Duration::from_secs(30), &answer).unwrap();
    let seen = phone.seen();
    assert!(matches!(&seen[2], Post(text) if text.starts_with("(pong grok)\nline\n")));
    assert_eq!(
        &Upload("answer.txt".to_owned(), format!("(pong grok)\n{answer}")),
        &seen[3]
    );
}

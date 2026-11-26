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
    CreateThread(String),
    DeleteRoot(String),
}

struct Phone {
    sessions: Vec<AgentSession>,
    clock: SystemTime,
    screen: String,
    posted: usize,
    created_threads: usize,
    pane_alive: bool,
    fail_bind_thread: bool,
    /// Posts starting with this fail, as a Slack outage would.
    failing_post: Option<&'static str>,
    log: Vec<Effect>,
    posts: Vec<(String, String)>,
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
            created_threads: 0,
            pane_alive: true,
            fail_bind_thread: false,
            failing_post: None,
            log: Vec::new(),
            posts: Vec::new(),
            live: LiveSessions::default(),
            setting: HostSettings::parse(
                "[host]\nname = \"linux\"\n[slack]\ncontrol_channel_id = \"C_CONTROL\"\noperator_id = \"U_OPERATOR\"\n",
            )
            .unwrap(),
        }
    }

    /// The Operator types `text` in the Session Thread.
    fn send(&mut self, text: &str) -> Result<(), String> {
        self.send_in("1757221923.123456", text)
    }

    /// The Operator types `text` in a specific thread.
    fn send_in(&mut self, thread_ts: &str, text: &str) -> Result<(), String> {
        let mut envelope: Value = serde_json::from_str(include_str!(
            "../../../slack/tests/fixtures/slack-message-envelopes.json"
        ))
        .unwrap();
        let mut envelope = envelope["operator"].take();
        envelope["payload"]["event"]["thread_ts"] = thread_ts.into();
        envelope["payload"]["event"]["text"] = text.into();
        let mut live = std::mem::take(&mut self.live);
        let setting = self.setting.clone();
        let handled = handle_slack_event(self, &setting, 0.0, &envelope, &mut live);
        self.live = live;
        handled
    }

    fn posts_in(&self, thread_ts: &str) -> Vec<&str> {
        self.posts
            .iter()
            .filter(|(ts, _)| ts == thread_ts)
            .map(|(_, text)| text.as_str())
            .collect()
    }

    /// Codex's notify hook fires `after` the last event with `input_messages` and `answer`.
    fn finish_codex(
        &mut self,
        after: Duration,
        input_messages: &[&str],
        answer: &str,
    ) -> Result<(), String> {
        self.clock += after;
        let completed = serde_json::json!({
            "type": "agent-turn-complete",
            "input-messages": input_messages,
            "last-assistant-message": answer,
        });
        let notification = serde_json::json!({
            "token": "secret",
            "agent_cli": "codex",
            "notification": completed.to_string(),
        });
        let mut live = std::mem::take(&mut self.live);
        let setting = self.setting.clone();
        let handled = handle_host_notification(self, &setting, &notification, &mut live);
        self.live = live;
        handled
    }

    /// Antigravity's Stop hook fires `after` the last event with `last_user_input` and `answer`.
    fn finish_agy(
        &mut self,
        after: Duration,
        termination_reason: &str,
        last_user_input: &str,
        answer: &str,
    ) -> Result<(), String> {
        self.clock += after;
        let stop = serde_json::json!({
            "hookEventName": "Stop",
            "terminationReason": termination_reason,
            "lastUserInput": last_user_input,
            "lastAssistantMessage": answer,
        });
        let notification = serde_json::json!({
            "token": "secret",
            "agent_cli": "agy",
            "notification": stop.to_string(),
        });
        let mut live = std::mem::take(&mut self.live);
        let setting = self.setting.clone();
        let handled = handle_host_notification(self, &setting, &notification, &mut live);
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
        self.pane_alive
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

    fn create_thread(&mut self, _session: &AgentSession) -> Result<String, String> {
        self.created_threads += 1;
        let ts = format!("1757221999.{:06}", self.created_threads);
        self.log.push(CreateThread(ts.clone()));
        Ok(ts)
    }

    fn bind_thread(&mut self, session: &mut AgentSession, thread_ts: &str) -> Result<(), String> {
        if self.fail_bind_thread {
            return Err("tmux set-option failed".to_owned());
        }
        session.thread_ts = Some(thread_ts.to_owned());
        session.thread_permalink = None;
        for saved in &mut self.sessions {
            if saved.pane_id == session.pane_id {
                saved.thread_ts = Some(thread_ts.to_owned());
                saved.thread_permalink = None;
            }
        }
        Ok(())
    }

    fn delete_root(&mut self, session: &AgentSession) -> Result<(), String> {
        self.log.push(DeleteRoot(session.thread()?.to_owned()));
        Ok(())
    }

    fn post(&mut self, session: &AgentSession, text: &str) -> Result<String, String> {
        if self
            .failing_post
            .is_some_and(|prefix| text.starts_with(prefix))
        {
            return Err("Slack API error: ratelimited".to_owned());
        }
        self.posted += 1;
        self.posts.push((
            session.thread_ts.clone().unwrap_or_default(),
            text.to_owned(),
        ));
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

#[test]
fn rethread_rotates_to_a_fresh_session_thread_and_routes_future_messages_there() {
    let mut phone = Phone::new();
    let old_ts = "1757221923.123456";
    let new_ts = "1757221999.000001";

    phone.send_in(old_ts, "pingme rethread").unwrap();
    assert_eq!(
        vec![
            CreateThread(new_ts.to_owned()),
            Status(SessionStatus::Idle),
            post("Moved to a new Session Thread."),
            DeleteRoot(old_ts.to_owned()),
        ],
        phone.seen()
    );
    assert_eq!(
        vec!["Moved to a new Session Thread."],
        phone.posts_in(old_ts)
    );
    assert!(phone.posts_in(new_ts).is_empty());

    // Messages sent in the old thread are no longer routed.
    phone.send_in(old_ts, "stray prompt").unwrap();
    assert!(phone.seen().is_empty());

    // Messages in the new thread reach the same running pane and answer in the new thread.
    phone.send_in(new_ts, "continue our task").unwrap();
    phone
        .finish(Duration::from_secs(10), "Still have full context.")
        .unwrap();
    assert_eq!(
        vec!["(ping grok)", "(pong grok)\nStill have full context."],
        phone.posts_in(new_ts)
    );
}

#[test]
fn rethread_while_busy_stops_editing_old_receipt_and_answers_in_the_new_thread() {
    let mut phone = Phone::new();
    let old_ts = "1757221923.123456";
    let new_ts = "1757221999.000001";

    phone.send_in(old_ts, "inspect the failure").unwrap();
    phone.send_in(old_ts, "then fix it").unwrap();
    phone.seen();

    phone.send_in(old_ts, "pingme rethread").unwrap();
    assert_eq!(
        vec![
            CreateThread(new_ts.to_owned()),
            Status(SessionStatus::Busy),
            post("Moved to a new Session Thread."),
            DeleteRoot(old_ts.to_owned()),
        ],
        phone.seen()
    );

    // Polling past the 20s edit window must not edit the old thread's receipt.
    phone.poll(Duration::from_secs(25), "⠴ Reading the CI log… 25.0s");
    assert!(phone.seen().is_empty());

    // When the turn completes, no old receipt is edited; the answer and queued prompt land in the new thread.
    phone.finish(Duration::from_secs(10), "Found it.").unwrap();
    assert_eq!(
        vec![
            Status(SessionStatus::Idle),
            post("(pong grok)\nFound it."),
            Status(SessionStatus::Busy),
            paste("then fix it"),
            post("(ping grok)"),
        ],
        phone.seen()
    );
    assert_eq!(
        vec!["(pong grok)\nFound it.", "(ping grok)"],
        phone.posts_in(new_ts)
    );
}

#[test]
fn rethread_rolls_back_the_new_root_when_binding_the_pane_fails() {
    let mut phone = Phone::new();
    let old_ts = "1757221923.123456";
    let new_ts = "1757221999.000001";
    phone.fail_bind_thread = true;

    assert!(phone.send_in(old_ts, "pingme rethread").is_err());
    assert_eq!(
        vec![
            CreateThread(new_ts.to_owned()),
            DeleteRoot(new_ts.to_owned()),
            post("Bridge error: tmux set-option failed"),
        ],
        phone.seen()
    );
    assert_eq!(
        vec!["Bridge error: tmux set-option failed"],
        phone.posts_in(old_ts)
    );

    // The old thread remains active after rollback.
    phone.send_in(old_ts, "still here").unwrap();
    assert_eq!(
        vec![
            Status(SessionStatus::Busy),
            paste("still here"),
            post("(ping grok)"),
        ],
        phone.seen()
    );
}

#[test]
fn rethread_on_a_dead_pane_marks_the_session_unavailable() {
    let mut phone = Phone::new();
    phone.pane_alive = false;

    phone.send("pingme rethread").unwrap();
    assert_eq!(
        vec![
            Status(SessionStatus::Unavailable),
            post("(reject grok unavailable)"),
        ],
        phone.seen()
    );
}

#[test]
fn shorter_codex_notification_after_rethread_does_not_replay_history() {
    let mut phone = Phone::new();
    phone.sessions[0].agent_cli = "codex".to_owned();
    phone.sessions[0].mirrored_terminal_count = 3;
    let old_ts = "1757221923.123456";
    let new_ts = "1757221999.000001";

    phone.send_in(old_ts, "pingme rethread").unwrap();
    phone.seen();

    // A Codex side-thread notification arrives with a shorter input-messages array.
    phone
        .finish_codex(
            Duration::from_secs(5),
            &["side-thread summary"],
            "{\"summary\":\"ok\"}",
        )
        .unwrap();
    phone.seen();

    // The next normal Slack turn arrives with the full 4-message main history.
    phone.send_in(new_ts, "summarize the PR").unwrap();
    phone
        .finish_codex(
            Duration::from_secs(10),
            &["old 1", "old 2", "old 3", "summarize the PR"],
            "Here is the summary.",
        )
        .unwrap();
    assert_eq!(
        vec![
            "(pong codex)\n{\"summary\":\"ok\"}",
            "(ping codex)",
            "(pong codex)\nHere is the summary.",
        ],
        phone.posts_in(new_ts)
    );
}

#[test]
fn a_failed_terminal_mirror_still_posts_the_codex_answer() {
    let mut phone = Phone::new();
    phone.sessions[0].agent_cli = "codex".to_owned();
    phone.failing_post = Some("*You (terminal)*");

    assert!(
        phone
            .finish_codex(Duration::from_secs(5), &["typed in tmux"], "Done.")
            .is_err()
    );
    assert_eq!(
        vec![
            "(pong codex)\nDone.",
            "Bridge error: Slack API error: ratelimited"
        ],
        phone.posts_in("1757221923.123456")
    );
}

#[test]
fn resumed_or_lagging_codex_session_does_not_replay_prior_history_into_new_thread() {
    let mut phone = Phone::new();
    phone.sessions[0].agent_cli = "codex".to_owned();
    phone.sessions[0].mirrored_terminal_count = 0;
    let old_ts = "1757221923.123456";
    let new_ts = "1757221999.000001";

    phone.send_in(old_ts, "pingme rethread").unwrap();
    phone.seen();

    // First turn in the new thread comes from Slack while Codex reports full prior history.
    phone.send_in(new_ts, "request descent fl 210").unwrap();
    phone
        .finish_codex(
            Duration::from_secs(10),
            &["old 1", "old 2", "old 3", "request descent fl 210"],
            "Cleared FL210.",
        )
        .unwrap();
    assert_eq!(
        vec!["(ping codex)", "(pong codex)\nCleared FL210."],
        phone.posts_in(new_ts)
    );

    // Next turn typed in the terminal mirrors only that prompt.
    phone
        .finish_codex(
            Duration::from_secs(5),
            &[
                "old 1",
                "old 2",
                "old 3",
                "request descent fl 210",
                "is visibility Tower?",
            ],
            "Yes, Tower.",
        )
        .unwrap();
    assert_eq!(
        vec![
            "(ping codex)",
            "(pong codex)\nCleared FL210.",
            "*You (terminal)*\nis visibility Tower?",
            "(pong codex)\nYes, Tower.",
        ],
        phone.posts_in(new_ts)
    );
}

#[test]
fn agy_answers_slack_and_terminal_turns_and_ignores_canceled_stops() {
    let mut phone = Phone::new();
    phone.sessions[0].agent_cli = "agy".to_owned();
    let thread_ts = "1757221923.123456";

    // Slack prompt is not echoed back as *You (terminal)*.
    phone.send("check the build").unwrap();
    phone
        .finish_agy(
            Duration::from_secs(12),
            "NO_TOOL_CALL",
            "check the build",
            "Build is green.",
        )
        .unwrap();

    // Terminal-typed prompt is mirrored once beside its answer.
    phone
        .finish_agy(
            Duration::from_secs(8),
            "NO_TOOL_CALL",
            "run clippy too",
            "Zero warnings.",
        )
        .unwrap();

    // User-canceled stops produce no bogus pong.
    phone
        .finish_agy(
            Duration::from_secs(2),
            "USER_CANCELED",
            "aborted prompt",
            "partial",
        )
        .unwrap();

    assert_eq!(
        vec![
            "(ping agy)",
            "(pong agy)\nBuild is green.",
            "*You (terminal)*\nrun clippy too",
            "(pong agy)\nZero warnings.",
        ],
        phone.posts_in(thread_ts)
    );
}

//! Per-pane state that lives only as long as this Host daemon process.
//! A restart drops a queued prompt and the receipt being edited.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

const STATUS_EDIT_EVERY: Duration = Duration::from_secs(20);
const PANE_STALL: Duration = Duration::from_secs(60);

struct Turn {
    started_at: SystemTime,
    last_edit_at: SystemTime,
    screen_changed_at: SystemTime,
    screen: String,
    wait_posted: bool,
    receipt_ts: Option<String>,
}

/// What one pane poll asks the daemon to post.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Progress {
    /// Edit the receipt to show this much time working.
    pub edit: Option<(String, Duration)>,
    /// Post a new message: the screen has not changed for `PANE_STALL`.
    pub waiting: bool,
}

pub(crate) struct EndedTurn {
    pub receipt_ts: Option<String>,
    pub elapsed: Duration,
}

#[derive(Default)]
pub(crate) struct LiveSessions {
    turns: HashMap<String, Turn>,
    queued: HashMap<String, String>,
}

impl LiveSessions {
    pub(crate) fn queued(&self, pane: &str) -> Option<&str> {
        self.queued.get(pane).map(String::as_str)
    }

    pub(crate) fn enqueue(&mut self, pane: &str, prompt: String) {
        self.queued.insert(pane.to_owned(), prompt);
    }

    pub(crate) fn take_queued(&mut self, pane: &str) -> Option<String> {
        self.queued.remove(pane)
    }

    pub(crate) fn begin_turn(
        &mut self,
        pane: &str,
        now: SystemTime,
        screen: &str,
        receipt_ts: Option<String>,
    ) {
        self.turns.insert(
            pane.to_owned(),
            Turn {
                started_at: now,
                last_edit_at: now,
                screen_changed_at: now,
                screen: stall_signature(screen),
                wait_posted: false,
                receipt_ts,
            },
        );
    }

    pub(crate) fn finish_turn(&mut self, pane: &str) -> Option<EndedTurn> {
        let turn = self.turns.remove(pane)?;
        Some(EndedTurn {
            receipt_ts: turn.receipt_ts,
            elapsed: since(SystemTime::now(), turn.started_at),
        })
    }

    pub(crate) fn drop_pane(&mut self, pane: &str) {
        self.turns.remove(pane);
        self.queued.remove(pane);
    }

    pub(crate) fn has_turn(&self, pane: &str) -> bool {
        self.turns.contains_key(pane)
    }

    pub(crate) fn step(&mut self, pane: &str, now: SystemTime, screen: &str) -> Option<Progress> {
        let turn = self.turns.get_mut(pane)?;
        // ponytail: relies on the Agent CLI animating a spinner while it works.
        // One with a static busy line would read as waiting after a minute.
        let screen = stall_signature(screen);
        if screen != turn.screen {
            turn.screen = screen;
            turn.screen_changed_at = now;
            turn.wait_posted = false;
        }
        let waiting = !turn.wait_posted && since(now, turn.screen_changed_at) >= PANE_STALL;
        turn.wait_posted |= waiting;
        let mut edit = None;
        if let Some(ts) = &turn.receipt_ts
            && since(now, turn.last_edit_at) >= STATUS_EDIT_EVERY
        {
            turn.last_edit_at = now;
            edit = Some((ts.clone(), since(now, turn.started_at)));
        }
        Some(Progress { edit, waiting })
    }
}

/// The pane, with each line that shows a running timer (`2.7s`, `5m13s`)
/// cut to its first glyph. That glyph is a spinner while the Agent CLI works
/// and a static mark while it waits; the timer, its padding, and the label it
/// truncates change either way.
fn stall_signature(screen: &str) -> String {
    let mut signature = String::with_capacity(screen.len());
    for line in screen.lines() {
        if has_timer(line) {
            signature.extend(line.trim_start().chars().next());
        } else {
            signature.push_str(line);
        }
        signature.push('\n');
    }
    signature
}

fn has_timer(line: &str) -> bool {
    let chars: Vec<char> = line.chars().chain([' ']).collect();
    chars
        .windows(3)
        .any(|w| w[0].is_ascii_digit() && w[1] == 's' && !w[2].is_alphanumeric())
}

fn since(now: SystemTime, then: SystemTime) -> Duration {
    now.duration_since(then).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Recorded from Grok CLI: a running shell command, then an approval prompt.
    const RUNNING: [&str; 2] = [
        "⠴ Sleep 25 seconds then print done… 1.6s   4.0s [stop]",
        "⠙ Sleep 25 seconds then print done… 5.6s   8.0s [stop]",
    ];
    const APPROVAL: [&str; 2] = [
        "◆ Fetch example.com HTTP status cod… 5m9s   5m12s ⇣21.4k [stop]\n3 (○) Yes, proceed",
        "◆ Fetch example.com HTTP status co… 5m10s   5m13s ⇣21.4k [stop]\n3 (○) Yes, proceed",
    ];

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + secs)
    }

    fn live_turn(screen: &str) -> LiveSessions {
        let mut live = LiveSessions::default();
        live.begin_turn("%7", at(0), screen, Some("1.2".to_owned()));
        live
    }

    #[test]
    fn a_ticking_timer_beside_a_static_prompt_is_waiting() {
        let mut live = live_turn(APPROVAL[0]);
        assert!(!live.step("%7", at(59), APPROVAL[1]).unwrap().waiting);
        assert!(live.step("%7", at(60), APPROVAL[0]).unwrap().waiting);
        assert!(!live.step("%7", at(90), APPROVAL[1]).unwrap().waiting);
    }

    #[test]
    fn only_lines_with_a_running_timer_are_reduced() {
        assert!(has_timer("⠴ Sleep… 1.6s   4.0s [stop]"));
        assert!(has_timer("Worked for 5m13s"));
        assert!(!has_timer("Sleep 25 seconds then print probe2.txt"));
        assert!(!has_timer("3 (○) Yes, proceed"));
    }

    #[test]
    fn an_animated_spinner_is_working() {
        let mut live = live_turn(RUNNING[0]);
        for secs in 1..=120 {
            let progress = live
                .step("%7", at(secs), RUNNING[secs as usize % 2])
                .unwrap();
            assert!(!progress.waiting, "{secs}s");
        }
    }

    #[test]
    fn receipt_edits_are_throttled() {
        let mut live = live_turn(RUNNING[0]);
        assert_eq!(None, live.step("%7", at(19), RUNNING[1]).unwrap().edit);
        assert_eq!(
            Some(("1.2".to_owned(), Duration::from_secs(20))),
            live.step("%7", at(20), RUNNING[0]).unwrap().edit
        );
        assert_eq!(None, live.step("%7", at(39), RUNNING[1]).unwrap().edit);
    }

    #[test]
    fn a_queued_prompt_is_replaced_and_dropped_with_its_pane() {
        let mut live = LiveSessions::default();
        live.enqueue("%7", "first".to_owned());
        live.enqueue("%7", "second".to_owned());
        assert_eq!(Some("second"), live.queued("%7"));
        live.drop_pane("%7");
        assert_eq!(None, live.take_queued("%7"));
    }
}

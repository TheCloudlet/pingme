use std::env;
use std::io::Write;
use std::process::{Command, Stdio};

use pingme_support::tmux::tmux;

pub fn pane_available(pane_id: &str) -> bool {
    tmux(&[
        "display-message",
        "-p",
        "-t",
        pane_id,
        "#{pane_id}\t#{pane_dead}",
    ])
    .is_ok_and(|output| output.trim() == format!("{pane_id}\t0"))
}

pub fn capture_pane(pane_id: &str) -> Result<String, String> {
    tmux(&["capture-pane", "-p", "-t", pane_id])
}

/// Sends tmux key names. Whitespace separates keys (`y`, `Enter`, `C-c`). No shell.
pub fn send_keys(pane_id: &str, keys: &str) -> Result<(), String> {
    let names: Vec<&str> = keys.split_whitespace().collect();
    if names.is_empty() {
        return Err("usage: pingme keys <keys>".to_owned());
    }
    let mut args = Vec::with_capacity(names.len() + 3);
    args.push("send-keys");
    args.push("-t");
    args.push(pane_id);
    args.extend(names);
    tmux(&args).map(drop)
}

pub fn paste_into_pane(pane_id: &str, text: &str) -> Result<(), String> {
    let buffer = format!("pingme-{}", std::process::id());
    let mut load = Command::new("tmux")
        .args(["load-buffer", "-b", &buffer, "-"])
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to load tmux buffer: {error}"))?;
    load.stdin
        .take()
        .ok_or_else(|| "failed to open tmux buffer stdin".to_owned())?
        .write_all(text.as_bytes())
        .map_err(|error| format!("failed to write tmux buffer: {error}"))?;
    let status = load
        .wait()
        .map_err(|error| format!("failed waiting for tmux load-buffer: {error}"))?;
    if !status.success() {
        return Err(format!("tmux load-buffer exited with {status}"));
    }
    tmux(&["paste-buffer", "-d", "-b", &buffer, "-t", pane_id])?;
    std::thread::sleep(std::time::Duration::from_millis(200));
    tmux(&["send-keys", "-t", pane_id, "Enter"]).map(drop)
}

pub fn current_tmux_pane() -> Option<String> {
    let pane_id = env::var("TMUX_PANE")
        .ok()
        .filter(|pane_id| !pane_id.is_empty())?;
    let current_tty = Command::new("tty")
        .stdin(Stdio::inherit())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !current_tty.status.success() {
        return None;
    }
    let current_tty = String::from_utf8(current_tty.stdout).ok()?;
    let pane_tty = tmux(&["display-message", "-p", "-t", &pane_id, "#{pane_tty}"]).ok()?;
    (current_tty.trim() == pane_tty.trim()).then_some(pane_id)
}

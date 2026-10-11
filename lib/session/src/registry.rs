use std::collections::BTreeMap;

use pingme_support::tmux::tmux;

use crate::{AgentSession, PanePlacement, SessionStatus};

/// Agent Session facts stored as `@pingme_<name>` tmux pane options, in write order.
/// `session_name` is last: it publishes the pane, so a crash mid-write leaves it unpublished.
const REGISTRY_FIELDS: [&str; 15] = [
    "agent_cli",
    "host",
    "project",
    "cwd",
    "status",
    "thread_ts",
    "thread_permalink",
    "control_channel",
    "operator",
    "placement",
    "last_slack_prompt",
    "mirrored_terminal_count",
    "notify_socket",
    "notify_token",
    "session_name",
];

fn registry_format() -> String {
    let mut format = "pane_id=#{pane_id}\ttmux_session=#{session_name}\ttmux_window=#{session_name}:#{window_index}\tpane_dead=#{pane_dead}".to_owned();
    for name in REGISTRY_FIELDS {
        format.push_str(&format!("\t{name}=#{{@pingme_{name}}}"));
    }
    format
}

impl AgentSession {
    pub fn from_registry_record(record: &str) -> Result<Option<Self>, String> {
        let mut fields = BTreeMap::new();
        for field in record.split('\t') {
            let (name, value) = field
                .split_once('=')
                .ok_or_else(|| format!("registry field has no name: {field}"))?;
            fields.insert(name, value);
        }
        let Some(session_name) = optional_field(&fields, "session_name") else {
            return Ok(None);
        };
        let pane_id = optional_field(&fields, "pane_id")
            .ok_or_else(|| format!("Agent Session {session_name} has no pane_id"))?;
        let agent_cli = optional_field(&fields, "agent_cli")
            .ok_or_else(|| format!("Agent Session {session_name} has no agent_cli"))?;
        let available = match fields.get("pane_dead").copied().unwrap_or("0") {
            "0" => true,
            "1" => false,
            value => {
                return Err(format!(
                    "Agent Session {session_name} has invalid pane_dead={value}"
                ));
            }
        };
        let status = SessionStatus::from_name(fields.get("status").copied().unwrap_or("idle"))?;
        let placement = PanePlacement::from_name(fields.get("placement").copied().unwrap_or(""))?;
        let mirrored_terminal_count = fields
            .get("mirrored_terminal_count")
            .copied()
            .filter(|value| !value.is_empty())
            .unwrap_or("0")
            .parse::<usize>()
            .map_err(|_| {
                format!("Agent Session {session_name} has invalid mirrored_terminal_count")
            })?;
        Ok(Some(Self {
            session_name,
            agent_cli,
            agent_args: Vec::new(),
            host: field(&fields, "host"),
            project: field(&fields, "project"),
            cwd: field(&fields, "cwd"),
            control_channel: field(&fields, "control_channel"),
            operator: field(&fields, "operator"),
            status,
            placement,
            thread_ts: optional_field(&fields, "thread_ts"),
            thread_permalink: optional_field(&fields, "thread_permalink"),
            pane_id: Some(pane_id),
            tmux_session: optional_field(&fields, "tmux_session"),
            tmux_window: optional_field(&fields, "tmux_window"),
            notify_socket: optional_field(&fields, "notify_socket"),
            notify_token: optional_field(&fields, "notify_token"),
            last_slack_prompt: optional_field(&fields, "last_slack_prompt"),
            mirrored_terminal_count,
            available,
        }))
    }

    pub fn list_from_registry() -> Result<Vec<Self>, String> {
        let records = match tmux(&["list-panes", "-a", "-F", &registry_format()]) {
            Err(error) if error.contains("no server running") => return Ok(Vec::new()),
            records => records?,
        };
        let mut sessions = Vec::new();
        for (index, record) in records.lines().enumerate() {
            if let Some(session) = Self::from_registry_record(record).map_err(|error| {
                format!("invalid tmux Agent Session record {}: {error}", index + 1)
            })? {
                sessions.push(session);
            }
        }
        Ok(sessions)
    }

    pub fn write_to_registry(&self) -> Result<(), String> {
        for (name, value) in self.registry_metadata() {
            self.write_field(name, &value)?;
        }
        Ok(())
    }

    pub fn name_exists(name: &str) -> Result<bool, String> {
        Ok(Self::list_from_registry()?
            .iter()
            .any(|session| session.session_name == name))
    }

    pub fn write_name(&mut self, name: &str) -> Result<(), String> {
        // Rename of an already-published Session; first publication goes through write_to_registry.
        self.write_field("session_name", name)?;
        self.session_name = name.to_owned();
        Ok(())
    }

    /// The fields a turn changes. Never `session_name`, which publishes the pane.
    pub fn write_turn_state(&self) -> Result<(), String> {
        self.write_field(
            "last_slack_prompt",
            self.last_slack_prompt.as_deref().unwrap_or(""),
        )?;
        self.write_field(
            "mirrored_terminal_count",
            &self.mirrored_terminal_count.to_string(),
        )?;
        self.write_field("status", self.status.as_str())
    }

    pub fn write_status(&mut self, status: SessionStatus) -> Result<(), String> {
        self.write_field("status", status.as_str())?;
        self.status = status;
        Ok(())
    }

    pub fn clear_registry_name(pane_id: &str) -> Result<(), String> {
        tmux(&[
            "set-option",
            "-p",
            "-u",
            "-t",
            pane_id,
            "@pingme_session_name",
        ])
        .map(drop)
    }

    pub fn clear_registry_name_command(pane_id: &str) -> String {
        format!("tmux set-option -p -u -t {pane_id} @pingme_session_name")
    }

    fn write_field(&self, name: &str, value: &str) -> Result<(), String> {
        let pane_id = self.pane()?;
        tmux(&[
            "set-option",
            "-p",
            "-t",
            pane_id,
            &format!("@pingme_{name}"),
            value,
        ])
        .map(drop)
    }

    fn registry_metadata(&self) -> [(&'static str, String); REGISTRY_FIELDS.len()] {
        let optional = |value: &Option<String>| value.clone().unwrap_or_default();
        REGISTRY_FIELDS.map(|name| {
            let value = match name {
                "agent_cli" => self.agent_cli.clone(),
                "host" => self.host.clone(),
                "project" => self.project.clone(),
                "cwd" => self.cwd.clone(),
                "status" => self.status.as_str().to_owned(),
                "thread_ts" => optional(&self.thread_ts),
                "thread_permalink" => optional(&self.thread_permalink),
                "control_channel" => self.control_channel.clone(),
                "operator" => self.operator.clone(),
                "placement" => self
                    .placement
                    .map(PanePlacement::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                "last_slack_prompt" => optional(&self.last_slack_prompt),
                "mirrored_terminal_count" => self.mirrored_terminal_count.to_string(),
                "notify_socket" => optional(&self.notify_socket),
                "notify_token" => optional(&self.notify_token),
                "session_name" => self.session_name.clone(),
                _ => unreachable!("unlisted registry field {name}"),
            };
            (name, value)
        })
    }
}

pub fn field(fields: &BTreeMap<&str, &str>, name: &str) -> String {
    fields.get(name).copied().unwrap_or_default().to_owned()
}

fn optional_field(fields: &BTreeMap<&str, &str>, name: &str) -> Option<String> {
    fields
        .get(name)
        .copied()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use crate::{AgentSession, PanePlacement, SessionStatus};

    #[test]
    fn registry_constructor_recovers_agent_session_facts() {
        let session = AgentSession::from_registry_record(
            "pane_id=%7\ttmux_session=bridge\ttmux_window=bridge:3\tpane_dead=0\tsession_name=codex-project\tagent_cli=codex\thost=linux\tproject=project\tcwd=/work/project\tstatus=busy\tthread_ts=100.1\tcontrol_channel=C1\toperator=U1\tnotify_socket=/tmp/bridge.sock\tnotify_token=secret\tlast_slack_prompt=abc123\tplacement=detached_window",
        )
        .unwrap()
        .unwrap();

        assert_eq!("codex-project", session.session_name);
        assert_eq!("codex", session.agent_cli);
        assert_eq!("linux", session.host);
        assert_eq!("project", session.project);
        assert_eq!("/work/project", session.cwd);
        assert_eq!(Some("%7"), session.pane_id.as_deref());
        assert_eq!(Some("bridge"), session.tmux_session.as_deref());
        assert_eq!(Some("bridge:3"), session.tmux_window.as_deref());
        assert_eq!(Some("100.1"), session.thread_ts.as_deref());
        assert_eq!("C1", session.control_channel);
        assert_eq!("U1", session.operator);
        assert_eq!(Some("/tmp/bridge.sock"), session.notify_socket.as_deref());
        assert_eq!(Some("secret"), session.notify_token.as_deref());
        assert_eq!(Some("abc123"), session.last_slack_prompt.as_deref());
        assert_eq!(SessionStatus::Busy, session.status);
        assert_eq!(Some(PanePlacement::DetachedWindow), session.placement);
        assert!(session.available);
    }

    #[test]
    fn registry_constructor_tolerates_optional_and_unknown_fields() {
        let session = AgentSession::from_registry_record(
            "pane_id=%7\tpane_dead=1\tsession_name=codex-project\tagent_cli=codex\tstatus=idle\tunknown_future_field=value",
        )
        .unwrap()
        .unwrap();

        assert_eq!("codex-project", session.session_name);
        assert_eq!("codex", session.agent_cli);
        assert_eq!(None, session.notify_token);
        assert_eq!(None, session.placement);
        assert_eq!(SessionStatus::Idle, session.status);
        assert!(!session.available);
        assert_ne!(SessionStatus::Unavailable, session.status);
    }

    #[test]
    fn registry_constructor_ignores_non_agent_panes() {
        assert!(
            AgentSession::from_registry_record("pane_id=%7\tpane_dead=0\tsession_name=")
                .unwrap()
                .is_none()
        );
        assert!(
            AgentSession::from_registry_record("session_name=broken\tstatus=wat")
                .unwrap_err()
                .contains("no pane_id")
        );
        assert!(
            AgentSession::from_registry_record(
                "pane_id=%7\tsession_name=broken\tagent_cli=codex\tstatus=wat",
            )
            .unwrap_err()
            .contains("invalid Agent Session status")
        );
        assert!(
            AgentSession::from_registry_record("pane_id=%7\tsession_name=codex-unbridged-2")
                .unwrap_err()
                .contains("no agent_cli")
        );
    }

    #[test]
    fn registry_write_publishes_session_name_last_and_reads_back() {
        let mut session = AgentSession::for_launch(
            "codex-project",
            "codex",
            "linux",
            "project",
            "/work/project",
            "C1",
            "U1",
            PanePlacement::DetachedWindow,
        );
        session.thread_ts = Some("100.1".to_owned());
        session.pane_id = Some("%7".to_owned());
        session.tmux_session = Some("bridge".to_owned());
        session.tmux_window = Some("bridge:3".to_owned());
        session.notify_token = Some("secret".to_owned());
        session.last_slack_prompt = Some("abc123".to_owned());
        session.mirrored_terminal_count = 2;
        session.status = SessionStatus::Busy;

        let metadata = session.registry_metadata();
        assert_eq!(Some("session_name"), metadata.last().map(|(name, _)| *name));

        let mut record =
            "pane_id=%7\ttmux_session=bridge\ttmux_window=bridge:3\tpane_dead=0".to_owned();
        for (name, value) in metadata {
            record.push_str(&format!("\t{name}={value}"));
        }
        assert_eq!(
            Some(session),
            AgentSession::from_registry_record(&record).unwrap()
        );
    }
}

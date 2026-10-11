//! The Agent Session and its tmux-backed registry.

pub mod registry;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanePlacement {
    ReuseCurrentPane,
    DetachedWindow,
    DetachedSession { attach: bool },
}

impl PanePlacement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReuseCurrentPane => "reuse_current_pane",
            Self::DetachedWindow => "detached_window",
            Self::DetachedSession { attach: true } => "detached_session_attach",
            Self::DetachedSession { attach: false } => "detached_session",
        }
    }

    fn from_name(name: &str) -> Result<Option<Self>, String> {
        match name {
            "" => Ok(None),
            "reuse_current_pane" => Ok(Some(Self::ReuseCurrentPane)),
            "detached_window" => Ok(Some(Self::DetachedWindow)),
            "detached_session_attach" => Ok(Some(Self::DetachedSession { attach: true })),
            "detached_session" => Ok(Some(Self::DetachedSession { attach: false })),
            _ => Err(format!("invalid Agent Session placement={name}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Starting,
    Idle,
    Busy,
    Unavailable,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Idle => "idle",
            Self::Busy => "busy",
            Self::Unavailable => "unavailable",
        }
    }
}

impl SessionStatus {
    fn from_name(name: &str) -> Result<Self, String> {
        match name {
            "starting" => Ok(Self::Starting),
            "" | "idle" => Ok(Self::Idle),
            "busy" => Ok(Self::Busy),
            "unavailable" => Ok(Self::Unavailable),
            _ => Err(format!("invalid Agent Session status={name}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSession {
    pub session_name: String,
    pub agent_cli: String,
    /// Launch-only Agent CLI arguments. Not stored in the live tmux registry.
    pub agent_args: Vec<String>,
    pub host: String,
    pub project: String,
    pub cwd: String,
    pub control_channel: String,
    pub operator: String,
    /// Registry-stored status. Dead panes may still store Idle; cards treat that as Unavailable.
    pub status: SessionStatus,
    pub placement: Option<PanePlacement>,
    pub thread_ts: Option<String>,
    pub thread_permalink: Option<String>,
    pub pane_id: Option<String>,
    pub tmux_session: Option<String>,
    pub tmux_window: Option<String>,
    pub notify_socket: Option<String>,
    pub notify_token: Option<String>,
    pub last_slack_prompt: Option<String>,
    /// Count of the Agent CLI's `input-messages` already mirrored to the
    /// Session Thread as terminal-typed prompts. Prevents re-mirroring the
    /// same terminal history on every subsequent turn notification.
    pub mirrored_terminal_count: usize,
    /// Live pane. False means cards and Busy routing treat the Session as Unavailable.
    pub available: bool,
}

impl AgentSession {
    #[allow(clippy::too_many_arguments)]
    pub fn for_launch(
        session_name: impl Into<String>,
        agent_cli: impl Into<String>,
        host: impl Into<String>,
        project: impl Into<String>,
        cwd: impl Into<String>,
        control_channel: impl Into<String>,
        operator: impl Into<String>,
        placement: PanePlacement,
    ) -> Self {
        Self {
            session_name: session_name.into(),
            agent_cli: agent_cli.into(),
            agent_args: Vec::new(),
            host: host.into(),
            project: project.into(),
            cwd: cwd.into(),
            control_channel: control_channel.into(),
            operator: operator.into(),
            status: SessionStatus::Starting,
            placement: Some(placement),
            thread_ts: None,
            thread_permalink: None,
            pane_id: None,
            tmux_session: None,
            tmux_window: None,
            notify_socket: None,
            notify_token: None,
            last_slack_prompt: None,
            mirrored_terminal_count: 0,
            available: true,
        }
    }

    pub fn pane(&self) -> Result<&str, String> {
        self.required(&self.pane_id, "tmux pane")
    }

    pub fn thread(&self) -> Result<&str, String> {
        self.required(&self.thread_ts, "Session Thread")
    }

    pub fn tmux_window(&self) -> Result<&str, String> {
        self.required(&self.tmux_window, "tmux window")
    }

    pub fn tmux_session(&self) -> Result<&str, String> {
        self.required(&self.tmux_session, "tmux session")
    }

    fn required<'a>(&self, value: &'a Option<String>, what: &str) -> Result<&'a str, String> {
        value
            .as_deref()
            .ok_or_else(|| format!("Agent Session {} has no {what}", self.session_name))
    }

    pub fn mark_unavailable(&mut self) {
        self.status = SessionStatus::Unavailable;
        self.available = false;
    }
}

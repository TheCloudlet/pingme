use pingme_host::services::LocalServices;
use pingme_routing::update::render_session_status_card;
use pingme_session::{AgentSession, PanePlacement};
use pingme_slack::probe::ControlChannelProbe;

use crate::cli::CliResult;
use crate::cli::doctor::REQUIRED_BOT_SCOPES;
use crate::{BTreeMap, run_cli_with_services};

pub(crate) const SETTING: &str = r#"
[host]
name = "linux"

[slack]
control_channel_id = "C_TEST_CONTROL"
operator_id = "U_TEST_OPERATOR"

[[projects]]
name = "pingme"
cwd = "/work/pingme"
"#;

#[derive(Default)]
pub(crate) struct FakeServices {
    pub(crate) calls: Vec<&'static str>,
    pub(crate) slack_error: Option<String>,
    pub(crate) update_error: Option<String>,
    pub(crate) daemon_error: Option<String>,
    pub(crate) without_permalink: bool,
    pub(crate) updated_card: Option<String>,
    pub(crate) registered_session: Option<AgentSession>,
    pub(crate) spawned_session: Option<AgentSession>,
    pub(crate) created_session: Option<AgentSession>,
    pub(crate) next_session_name: Option<String>,
    pub(crate) bot_scopes: Option<Vec<&'static str>>,
    pub(crate) bot_token_error: Option<String>,
    pub(crate) app_token_error: Option<String>,
    pub(crate) control_channel: Option<Result<ControlChannelProbe, String>>,
    pub(crate) daemon_down: bool,
    pub(crate) missing_cwd: Option<&'static str>,
}

impl LocalServices for FakeServices {
    fn available_session_name(&mut self, base: &str) -> Result<String, String> {
        Ok(self
            .next_session_name
            .clone()
            .unwrap_or_else(|| base.to_owned()))
    }

    fn create_session_thread(&mut self, session: AgentSession) -> Result<AgentSession, String> {
        self.calls.push("create_session_thread");
        self.created_session = Some(session.clone());
        match &self.slack_error {
            Some(error) => Err(error.clone()),
            None => {
                let mut session = session;
                session.thread_ts = Some("1757221923.123456".to_owned());
                session.thread_permalink = if self.without_permalink {
                    None
                } else {
                    Some("https://workspace.slack.com/archives/C/thread".to_owned())
                };
                Ok(session)
            }
        }
    }

    fn update_session_thread(&mut self, session: &AgentSession) -> Result<(), String> {
        self.calls.push("update_session_thread");
        assert_eq!("C_TEST_CONTROL", session.control_channel);
        assert_eq!(Some("1757221923.123456"), session.thread_ts.as_deref());
        self.updated_card = Some(render_session_status_card(session));
        match &self.update_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn spawn_agent(&mut self, session: AgentSession) -> Result<AgentSession, String> {
        self.calls.push("spawn_agent");
        self.spawned_session = Some(session.clone());
        let tmux_session = matches!(
            session.placement,
            Some(PanePlacement::DetachedSession { .. })
        )
        .then(|| session.session_name.clone());
        let mut session = session;
        session.pane_id = Some("%7".to_owned());
        session.tmux_session = tmux_session;
        session.notify_socket = Some("/tmp/pingme-test.sock".to_owned());
        session.notify_token = Some("notify-token".to_owned());
        Ok(session)
    }

    fn register_session(&mut self, session: &AgentSession) -> Result<(), String> {
        self.calls.push("register_session");
        self.registered_session = Some(session.clone());
        match &self.daemon_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn list_sessions(&mut self) -> Result<Vec<AgentSession>, String> {
        Ok(Vec::new())
    }

    fn attach_session(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }

    fn cleanup_sessions(&mut self) -> Result<usize, String> {
        self.calls.push("cleanup_sessions");
        Ok(2)
    }

    fn bot_token_scopes(&mut self) -> Result<Vec<String>, String> {
        self.calls.push("bot_token_scopes");
        if let Some(error) = &self.bot_token_error {
            return Err(error.clone());
        }
        let scopes = self
            .bot_scopes
            .clone()
            .unwrap_or(REQUIRED_BOT_SCOPES.to_vec());
        Ok(scopes.into_iter().map(str::to_owned).collect())
    }

    fn probe_app_token(&mut self, app_token: &str) -> Result<(), String> {
        self.calls.push("probe_app_token");
        assert_eq!("xapp-test", app_token);
        match &self.app_token_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn probe_control_channel(&mut self, channel_id: &str) -> Result<ControlChannelProbe, String> {
        self.calls.push("probe_control_channel");
        assert_eq!("C_TEST_CONTROL", channel_id);
        self.control_channel
            .clone()
            .unwrap_or(Ok(ControlChannelProbe {
                is_private: true,
                is_member: true,
            }))
    }

    fn project_dir_exists(&mut self, cwd: &str) -> bool {
        self.missing_cwd != Some(cwd)
    }

    fn host_daemon_running(&mut self) -> bool {
        !self.daemon_down
    }
}

pub(crate) fn env_with_tokens() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("SLACK_APP_TOKEN", "xapp-test"),
        ("SLACK_BOT_TOKEN", "xoxb-test"),
        ("PWD", "/work/pingme"),
    ])
}

pub(crate) fn run_cli(args: &[&str], setting_toml: &str, env: &BTreeMap<&str, &str>) -> CliResult {
    run_cli_with_services(args, setting_toml, env, &mut FakeServices::default())
}

pub(crate) fn doctor_with(services: &mut FakeServices, setting_toml: &str) -> CliResult {
    run_cli_with_services(
        &["pingme", "doctor"],
        setting_toml,
        &env_with_tokens(),
        services,
    )
}

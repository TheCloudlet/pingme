use pingme_agent::AGENT_CLIS;
use pingme_routing::control::{ControlAction, ControlCommand, control_action};
use pingme_session::{AgentSession, SessionStatus};
use pingme_setting::{HostSettings, ProjectSetting};
use pingme_slack::api::{post_control_response, update_status_card_to};
use pingme_tmux::pane::current_tmux_pane;
use serde_json::Value;

use crate::launch::{pane_placement, start_agent_session};
use crate::services::{LocalServices, RealServices};

pub(crate) fn handle_control_command(
    bot_token: &str,
    setting: &HostSettings,
    envelope: &Value,
) -> Result<(), String> {
    let Some(payload) = envelope.get("payload") else {
        return Ok(());
    };
    let field = |name| payload.get(name).and_then(Value::as_str).unwrap_or("");
    let action = control_action(
        &ControlCommand {
            command: field("command"),
            channel_id: field("channel_id"),
            user_id: field("user_id"),
            text: field("text"),
        },
        &setting.slack.control_channel_id,
        &setting.slack.operator_id,
    );
    let reply = match action {
        ControlAction::Ignore => return Ok(()),
        ControlAction::Reject(message) => message,
        ControlAction::NewSession {
            agent_cli,
            project_name,
            agent_args,
        } => remote_launch_reply(bot_token, setting, &agent_cli, &project_name, agent_args),
    };
    post_control_response(bot_token, payload, &reply)
}

fn remote_launch_reply(
    bot_token: &str,
    setting: &HostSettings,
    agent_cli: &str,
    project_name: &str,
    agent_args: Vec<String>,
) -> String {
    if !AGENT_CLIS.contains(&agent_cli) {
        return format!("Unknown Agent CLI: {agent_cli}");
    }
    let Some(project) = setting
        .projects
        .iter()
        .find(|project| project.name == project_name)
    else {
        return format!("Unknown Project: {project_name}");
    };
    match launch_remote_agent(bot_token, setting, project, agent_cli, agent_args) {
        Ok(launch) => {
            let warning = launch
                .warning
                .map(|warning| format!("\nWarning: {}", warning.trim()))
                .unwrap_or_default();
            format!("Created `{}`.{warning}", launch.session_name)
        }
        Err(error) => format!("Launch failed: {error}"),
    }
}

struct RemoteLaunch {
    session_name: String,
    warning: Option<String>,
}

fn launch_remote_agent(
    bot_token: &str,
    setting: &HostSettings,
    project: &ProjectSetting,
    agent_cli: &str,
    agent_args: Vec<String>,
) -> Result<RemoteLaunch, String> {
    let mut services = RealServices::new(bot_token.to_owned());
    let session_name = services.available_session_name(&format!("{agent_cli}-{}", project.name))?;
    let mut session = AgentSession::for_launch(
        session_name,
        agent_cli,
        &setting.host.name,
        &project.name,
        &project.cwd,
        &setting.slack.control_channel_id,
        &setting.slack.operator_id,
        pane_placement(false, current_tmux_pane().is_some()),
    );
    session.agent_args = agent_args;
    let session = services.create_session_thread(session)?;
    let failed_session = session.clone();
    let started = match start_agent_session(session, &mut services) {
        Ok(started) => started,
        Err(error) => {
            let _ = update_status_card_to(bot_token, failed_session, SessionStatus::Unavailable);
            return Err(error);
        }
    };
    if let Some(error) = started.registration_error {
        let _ = update_status_card_to(bot_token, started.session, SessionStatus::Unavailable);
        return Err(error);
    }
    let warning = if started.status_warning.is_some() {
        update_status_card_to(bot_token, started.session.clone(), SessionStatus::Idle).err()
    } else {
        None
    };
    Ok(RemoteLaunch {
        session_name: started.session.session_name,
        warning,
    })
}

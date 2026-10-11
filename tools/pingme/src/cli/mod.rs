//! The `pingme` commands, written against `LocalServices` so they test
//! without Slack or tmux.

use std::collections::BTreeMap;

use pingme_agent::AGENT_CLIS;
use pingme_host::services::LocalServices;
use pingme_session::AgentSession;
use pingme_setting::HostSettings;

use crate::cli::doctor::doctor;
use crate::cli::launch::launch_agent;
use crate::cli::manage::manage_or_defer;

mod doctor;
mod launch;
mod manage;
#[cfg(test)]
mod testing;

pub(crate) struct CliResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub attachment: Option<AgentSession>,
}

pub(crate) fn run_cli_with_services(
    args: &[&str],
    setting_toml: &str,
    env: &BTreeMap<&str, &str>,
    services: &mut dyn LocalServices,
) -> CliResult {
    let Some(command) = args.get(1).copied() else {
        return fail(usage());
    };

    match command {
        agent_cli if AGENT_CLIS.contains(&agent_cli) => {
            launch_agent(agent_cli, &args[2..], setting_toml, env, services)
        }
        "doctor" => doctor(setting_toml, env, services),
        "list" | "attach" | "cleanup" => {
            manage_or_defer(command, args, setting_toml, env, services)
        }
        other => fail(format!("unknown command: {other}\n{}", usage())),
    }
}

pub fn usage() -> String {
    "usage: pingme <codex|grok> [agent-args...] | pingme <list|attach <session-name>|cleanup|doctor>\n"
        .to_owned()
}

pub fn ok(stdout: String) -> CliResult {
    CliResult {
        exit_code: 0,
        stdout,
        stderr: String::new(),
        attachment: None,
    }
}

pub fn fail(stderr: String) -> CliResult {
    CliResult {
        exit_code: 2,
        stdout: String::new(),
        stderr,
        attachment: None,
    }
}

pub(crate) fn require_slack_tokens(env: &BTreeMap<&str, &str>) -> Result<(), String> {
    let mut missing = Vec::new();
    for name in ["SLACK_APP_TOKEN", "SLACK_BOT_TOKEN"] {
        if env.get(name).is_none_or(|value| value.is_empty()) {
            missing.push(format!("missing environment variable {name}"));
        }
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("{}\n", missing.join("\n")))
    }
}

pub fn current_dir(env: &BTreeMap<&str, &str>) -> String {
    env.get("PWD").copied().unwrap_or(".").to_owned()
}

pub(crate) fn parse_setting(setting_toml: &str) -> Result<HostSettings, String> {
    if setting_toml.trim().is_empty() {
        return Err("setting.toml is missing or empty\n".to_owned());
    }
    HostSettings::parse(setting_toml).map_err(|error| format!("{error}\n"))
}

pub(crate) fn setting_summary(setting: &HostSettings) -> String {
    let mut output = format!(
        "Host: {}\nControl Channel: {}\nOperator: {}\n",
        setting.host.name, setting.slack.control_channel_id, setting.slack.operator_id,
    );
    for project in &setting.projects {
        output.push_str(&format!("Project: {} -> {}\n", project.name, project.cwd));
    }
    output
}

#[cfg(test)]
mod tests {

    use crate::cli::testing::*;

    #[test]
    fn reports_unknown_commands_with_usage() {
        let result = run_cli(&["pingme", "wat"], SETTING, &env_with_tokens());

        assert_eq!(2, result.exit_code);
        assert!(result.stderr.contains("unknown command: wat"));
        assert!(result.stderr.contains("usage: pingme"));
    }
}

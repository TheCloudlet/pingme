use std::collections::BTreeMap;

use pingme_host::services::LocalServices;

use crate::cli::{CliResult, parse_setting};

pub(crate) const REQUIRED_BOT_SCOPES: [&str; 5] = [
    "chat:write",
    "commands",
    "files:read",
    "groups:history",
    "groups:read",
];

pub fn doctor(
    setting_toml: &str,
    env: &BTreeMap<&str, &str>,
    services: &mut dyn LocalServices,
) -> CliResult {
    let mut report = DoctorReport::default();
    let setting = match parse_setting(setting_toml) {
        Ok(setting) => {
            report.ok("setting.toml");
            Some(setting)
        }
        Err(error) => {
            report.fail(
                "setting.toml",
                error.trim(),
                "copy setting.example.toml to setting.toml, or set PINGME_HOME to its folder",
            );
            None
        }
    };
    if let Some(setting) = &setting {
        for project in &setting.projects {
            let check = format!("Project {}", project.name);
            if services.project_dir_exists(&project.cwd) {
                report.ok(&check);
            } else {
                report.fail(
                    &check,
                    &format!("{} is not a directory", project.cwd),
                    "fix [[projects]] cwd in setting.toml",
                );
            }
        }
    }

    let bot_token_present = check_token_present(&mut report, env, "SLACK_BOT_TOKEN", "xoxb-");
    let app_token = check_token_present(&mut report, env, "SLACK_APP_TOKEN", "xapp-")
        .then(|| env["SLACK_APP_TOKEN"]);

    let bot_token_valid = bot_token_present
        && match services.bot_token_scopes() {
            Ok(scopes) => {
                let missing: Vec<&str> = REQUIRED_BOT_SCOPES
                    .into_iter()
                    .filter(|scope| !scopes.iter().any(|granted| granted == scope))
                    .collect();
                if missing.is_empty() {
                    report.ok("bot token scopes");
                } else {
                    report.fail(
                        "bot token scopes",
                        &format!("missing {}", missing.join(", ")),
                        "add them under OAuth & Permissions, then click Reinstall to Workspace",
                    );
                }
                true
            }
            Err(error) => {
                report.fail(
                    "bot token",
                    &format!("Slack rejected SLACK_BOT_TOKEN: {error}"),
                    "copy Bot User OAuth Token from OAuth & Permissions",
                );
                false
            }
        };

    if let Some(app_token) = app_token {
        match services.probe_app_token(app_token) {
            Ok(()) => report.ok("app token"),
            Err(error) => report.fail(
                "app token",
                &format!("Slack rejected SLACK_APP_TOKEN: {error}"),
                "generate an App-Level Token with connections:write under Basic Information",
            ),
        }
    }

    if let Some(setting) = &setting
        && bot_token_valid
    {
        let channel = &setting.slack.control_channel_id;
        match services.probe_control_channel(channel) {
            Ok(probe) if !probe.is_private => report.fail(
                "Control Channel",
                &format!("{channel} is public; anyone in the workspace could see the session"),
                "use a private channel with only you and the bot",
            ),
            Ok(probe) if !probe.is_member => report.fail(
                "Control Channel",
                &format!("the bot is not in {channel}"),
                "in the channel type /invite @ping and pick the pingme app from the list",
            ),
            Ok(_) => report.ok("Control Channel"),
            Err(error) => report.fail(
                "Control Channel",
                &format!("cannot read {channel}: {error}"),
                "check control_channel_id, then invite the pingme app to the channel",
            ),
        }
    }

    if services.host_daemon_running() {
        report.ok("Host daemon");
    } else {
        report.fail(
            "Host daemon",
            "not running; Slack /pingme does nothing until it is",
            "run pingme daemon with the tokens exported",
        );
    }

    report.finish()
}

fn check_token_present(
    report: &mut DoctorReport,
    env: &BTreeMap<&str, &str>,
    name: &str,
    prefix: &str,
) -> bool {
    match env.get(name) {
        None => {
            report.fail(name, "missing", &format!("export {name}"));
            false
        }
        Some(value) if !value.starts_with(prefix) => {
            report.fail(
                name,
                &format!("does not start with {prefix}"),
                &format!("export the {prefix}... token as {name}"),
            );
            false
        }
        Some(_) => true,
    }
}

#[derive(Default)]
struct DoctorReport {
    output: String,
    failed: bool,
}

impl DoctorReport {
    fn ok(&mut self, check: &str) {
        self.output.push_str(&format!("ok   {check}\n"));
    }

    fn fail(&mut self, check: &str, reason: &str, fix: &str) {
        self.failed = true;
        self.output
            .push_str(&format!("FAIL {check}: {reason}\n     fix: {fix}\n"));
    }

    fn finish(self) -> CliResult {
        CliResult {
            exit_code: if self.failed { 2 } else { 0 },
            stdout: self.output,
            stderr: String::new(),
            attachment: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use pingme_slack::probe::ControlChannelProbe;

    use crate::cli::testing::*;
    use crate::{BTreeMap, run_cli_with_services};

    #[test]
    fn doctor_passes_when_every_piece_is_in_place() {
        let result = run_cli(&["pingme", "doctor"], SETTING, &env_with_tokens());

        assert_eq!(0, result.exit_code, "{}", result.stdout);
        assert!(!result.stdout.contains("FAIL"));
        assert!(result.stdout.contains("ok   setting.toml"));
        assert!(result.stdout.contains("ok   bot token scopes"));
        assert!(result.stdout.contains("ok   Control Channel"));
        assert!(result.stdout.contains("ok   Host daemon"));
    }

    #[test]
    fn doctor_reports_missing_tokens_without_leaking_values_and_skips_slack() {
        let mut services = FakeServices::default();
        let env = BTreeMap::from([("SLACK_APP_TOKEN", "xoxb-pasted-into-the-wrong-variable")]);
        let result = run_cli_with_services(&["pingme", "doctor"], SETTING, &env, &mut services);

        assert_eq!(2, result.exit_code);
        assert!(result.stdout.contains("FAIL SLACK_BOT_TOKEN: missing"));
        assert!(
            result
                .stdout
                .contains("FAIL SLACK_APP_TOKEN: does not start with xapp-")
        );
        assert!(!result.stdout.contains("pasted-into-the-wrong-variable"));
        assert!(!services.calls.contains(&"bot_token_scopes"));
        assert!(!services.calls.contains(&"probe_app_token"));
        assert!(!services.calls.contains(&"probe_control_channel"));
    }

    #[test]
    fn doctor_names_the_missing_scope_and_the_reinstall_step() {
        let mut services = FakeServices {
            bot_scopes: Some(vec![
                "chat:write",
                "commands",
                "groups:history",
                "groups:read",
            ]),
            ..FakeServices::default()
        };
        let result = doctor_with(&mut services, SETTING);

        assert_eq!(2, result.exit_code);
        assert!(
            result
                .stdout
                .contains("FAIL bot token scopes: missing files:read")
        );
        assert!(result.stdout.contains("Reinstall to Workspace"));
    }

    #[test]
    fn doctor_skips_the_channel_check_when_slack_rejects_the_bot_token() {
        let mut services = FakeServices {
            bot_token_error: Some("Slack API error: invalid_auth".to_owned()),
            ..FakeServices::default()
        };
        let result = doctor_with(&mut services, SETTING);

        assert_eq!(2, result.exit_code);
        assert!(result.stdout.contains(
            "FAIL bot token: Slack rejected SLACK_BOT_TOKEN: Slack API error: invalid_auth"
        ));
        assert!(!services.calls.contains(&"probe_control_channel"));
    }

    #[test]
    fn doctor_reports_a_rejected_app_token() {
        let mut services = FakeServices {
            app_token_error: Some("Slack API error: invalid_auth".to_owned()),
            ..FakeServices::default()
        };
        let result = doctor_with(&mut services, SETTING);

        assert_eq!(2, result.exit_code);
        assert!(result.stdout.contains("FAIL app token"));
        assert!(result.stdout.contains("connections:write"));
    }

    #[test]
    fn doctor_tells_the_operator_to_invite_the_bot() {
        let mut services = FakeServices {
            control_channel: Some(Ok(ControlChannelProbe {
                is_private: true,
                is_member: false,
            })),
            ..FakeServices::default()
        };
        let result = doctor_with(&mut services, SETTING);

        assert_eq!(2, result.exit_code);
        assert!(
            result
                .stdout
                .contains("FAIL Control Channel: the bot is not in C_TEST_CONTROL")
        );
        assert!(result.stdout.contains("pick the pingme app from the list"));
    }

    #[test]
    fn doctor_rejects_a_public_control_channel_before_asking_for_an_invite() {
        let mut services = FakeServices {
            control_channel: Some(Ok(ControlChannelProbe {
                is_private: false,
                is_member: false,
            })),
            ..FakeServices::default()
        };
        let result = doctor_with(&mut services, SETTING);

        assert_eq!(2, result.exit_code);
        assert!(
            result
                .stdout
                .contains("FAIL Control Channel: C_TEST_CONTROL is public")
        );
    }

    #[test]
    fn doctor_reports_a_project_cwd_that_does_not_exist() {
        let mut services = FakeServices {
            missing_cwd: Some("/work/pingme"),
            ..FakeServices::default()
        };
        let result = doctor_with(&mut services, SETTING);

        assert_eq!(2, result.exit_code);
        assert!(
            result
                .stdout
                .contains("FAIL Project pingme: /work/pingme is not a directory")
        );
    }

    #[test]
    fn doctor_reports_a_stopped_host_daemon() {
        let mut services = FakeServices {
            daemon_down: true,
            ..FakeServices::default()
        };
        let result = doctor_with(&mut services, SETTING);

        assert_eq!(2, result.exit_code);
        assert!(result.stdout.contains("FAIL Host daemon: not running"));
        assert!(result.stdout.contains("pingme daemon"));
    }

    #[test]
    fn doctor_keeps_checking_tokens_when_setting_toml_is_missing() {
        let mut services = FakeServices::default();
        let result = doctor_with(&mut services, "");

        assert_eq!(2, result.exit_code);
        assert!(
            result
                .stdout
                .contains("FAIL setting.toml: setting.toml is missing or empty")
        );
        assert!(result.stdout.contains("ok   bot token scopes"));
        assert!(result.stdout.contains("ok   app token"));
        assert!(!services.calls.contains(&"probe_control_channel"));
    }
}

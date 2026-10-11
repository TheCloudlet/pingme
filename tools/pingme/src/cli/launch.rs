use std::collections::BTreeMap;

use pingme_host::launch::{pane_placement, start_agent_session};
use pingme_host::services::LocalServices;
use pingme_routing::update::agent_display_name;
use pingme_session::AgentSession;
use pingme_setting::{HostSettings, ProjectSetting};

use crate::cli::{CliResult, current_dir, fail, parse_setting, require_slack_tokens};

pub(crate) fn launch_agent(
    agent_cli: &str,
    agent_args: &[&str],
    setting_toml: &str,
    env: &BTreeMap<&str, &str>,
    services: &mut dyn LocalServices,
) -> CliResult {
    let agent_args = match parse_agent_args(agent_args) {
        Ok(agent_args) => agent_args,
        Err(message) => return fail(message),
    };
    let setting = match parse_setting(setting_toml) {
        Ok(setting) => setting,
        Err(message) => {
            return launch_unbridged_agent(agent_cli, agent_args, env, services, message);
        }
    };

    if let Err(message) = require_slack_tokens(env) {
        return launch_unbridged_agent(agent_cli, agent_args, env, services, message);
    }

    let cwd = current_dir(env);
    let project = local_project(&setting, &cwd);
    let session_name =
        match services.available_session_name(&format!("{agent_cli}-{}", project.name)) {
            Ok(session_name) => session_name,
            Err(error) => return fail(format!("failed to choose Session Name: {error}\n")),
        };
    let mut session = AgentSession::for_launch(
        session_name,
        agent_cli,
        &setting.host.name,
        &project.name,
        cwd,
        &setting.slack.control_channel_id,
        &setting.slack.operator_id,
        pane_placement(true, env.contains_key("TMUX")),
    );
    session.agent_args.clone_from(&agent_args);

    let session = match services.create_session_thread(session) {
        Ok(session) => session,
        Err(error) => {
            return launch_unbridged_agent(
                agent_cli,
                agent_args,
                env,
                services,
                format!("failed to create Slack Session Thread: {error}\n"),
            );
        }
    };

    let started = match start_agent_session(session, services) {
        Ok(started) => started,
        Err(error) => {
            return fail(format!(
                "failed to launch {} CLI: {error}\n",
                agent_display_name(agent_cli)
            ));
        }
    };
    let stderr = format!(
        "{}{}",
        started.status_warning.as_deref().unwrap_or(""),
        started.registration_error.as_deref().unwrap_or("")
    );

    let thread_identity = thread_identity(&started.session);
    CliResult {
        exit_code: 0,
        stdout: format!(
            "Slack Session Thread: {}\nlaunching {} CLI\n",
            thread_identity,
            agent_display_name(agent_cli),
        ),
        stderr,
        attachment: Some(started.session),
    }
}

fn show_unbridged_agent(
    services: &mut dyn LocalServices,
    session: AgentSession,
    warning: String,
) -> CliResult {
    let agent_cli = session.agent_cli.clone();
    let session = match services.spawn_agent(session) {
        Ok(session) => session,
        Err(error) => {
            return fail(format!(
                "{warning}failed to launch {} CLI: {error}\n",
                agent_display_name(&agent_cli)
            ));
        }
    };

    CliResult {
        exit_code: 0,
        stdout: format!("launching {} CLI\n", agent_display_name(&session.agent_cli)),
        stderr: format!(
            "{warning}launching unbridged {} CLI; no Slack Thread was created\n",
            agent_display_name(&session.agent_cli)
        ),
        attachment: Some(session),
    }
}

fn thread_identity(session: &AgentSession) -> String {
    session.thread_permalink.clone().unwrap_or_else(|| {
        format!(
            "{} {}",
            session.control_channel,
            session.thread_ts.as_deref().unwrap_or("pending")
        )
    })
}

fn launch_unbridged_agent(
    agent_cli: &str,
    agent_args: Vec<String>,
    env: &BTreeMap<&str, &str>,
    services: &mut dyn LocalServices,
    warning: String,
) -> CliResult {
    let mut session = AgentSession::for_launch(
        services
            .available_session_name(&format!("{agent_cli}-unbridged"))
            .unwrap_or_else(|_| format!("{agent_cli}-unbridged")),
        agent_cli,
        "",
        "unknown",
        current_dir(env),
        "",
        "",
        pane_placement(true, env.contains_key("TMUX")),
    );
    session.agent_args = agent_args;

    show_unbridged_agent(services, session, warning)
}

fn parse_agent_args(args: &[&str]) -> Result<Vec<String>, String> {
    if args.iter().any(|arg| arg.contains(['\t', '\n', '\r'])) {
        Err("Agent CLI arguments contain unsupported control characters\n".to_owned())
    } else {
        Ok(args.iter().map(|arg| (*arg).to_owned()).collect())
    }
}

fn local_project(setting: &HostSettings, cwd: &str) -> ProjectSetting {
    setting
        .projects
        .iter()
        .find(|project| project.cwd == cwd)
        .cloned()
        .unwrap_or(ProjectSetting {
            name: "unknown".to_owned(),
            cwd: cwd.to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use pingme_host::launch::{pane_placement, start_agent_session};
    use pingme_session::{AgentSession, PanePlacement};

    use crate::cli::testing::*;
    use crate::{BTreeMap, run_cli_with_services};

    #[test]
    fn local_launch_inside_tmux_reuses_the_current_pane() {
        assert_eq!(PanePlacement::ReuseCurrentPane, pane_placement(true, true));

        let mut env = env_with_tokens();
        env.insert("TMUX", "/tmp/tmux-1000/default,1,0");
        env.insert("TMUX_PANE", "%7");
        for agent in ["codex", "grok"] {
            let mut services = FakeServices::default();
            let result = run_cli_with_services(&["pingme", agent], SETTING, &env, &mut services);

            assert_eq!(0, result.exit_code);
            assert_eq!(
                Some(PanePlacement::ReuseCurrentPane),
                result
                    .attachment
                    .as_ref()
                    .and_then(|session| session.placement)
            );
            assert_eq!(
                Some(PanePlacement::ReuseCurrentPane),
                services
                    .spawned_session
                    .as_ref()
                    .and_then(|session| session.placement)
            );
        }
    }

    #[test]
    fn local_launch_outside_tmux_attaches_the_new_session() {
        assert_eq!(
            PanePlacement::DetachedSession { attach: true },
            pane_placement(true, false)
        );

        let mut services = FakeServices::default();
        let result = run_cli_with_services(
            &["pingme", "codex"],
            SETTING,
            &env_with_tokens(),
            &mut services,
        );

        assert_eq!(0, result.exit_code);
        assert_eq!(
            Some(PanePlacement::DetachedSession { attach: true }),
            result
                .attachment
                .as_ref()
                .and_then(|session| session.placement)
        );
        assert_eq!(
            Some(PanePlacement::DetachedSession { attach: true }),
            services
                .spawned_session
                .as_ref()
                .and_then(|session| session.placement)
        );
    }

    #[test]
    fn unbridged_local_launch_inside_tmux_reuses_the_current_pane() {
        let env = BTreeMap::from([("TMUX", "/tmp/tmux-1000/default,1,0"), ("TMUX_PANE", "%7")]);
        let mut services = FakeServices::default();
        let result = run_cli_with_services(&["pingme", "codex"], SETTING, &env, &mut services);

        assert_eq!(0, result.exit_code);
        let session = result.attachment.unwrap();
        assert_eq!(Some(PanePlacement::ReuseCurrentPane), session.placement);
        assert_eq!(None, session.thread_ts);
    }

    #[test]
    fn accepts_local_launch_commands() {
        let codex = run_cli(&["pingme", "codex"], SETTING, &env_with_tokens());
        let grok = run_cli(&["pingme", "grok"], SETTING, &env_with_tokens());

        assert_eq!(0, codex.exit_code);
        assert!(codex.stdout.contains("launching Codex CLI"));
        assert_eq!(0, grok.exit_code);
        assert!(grok.stdout.contains("launching Grok CLI"));
    }

    #[test]
    fn local_codex_creates_slack_thread_before_launching_codex() {
        let mut services = FakeServices::default();
        let result = run_cli_with_services(
            &["pingme", "codex"],
            SETTING,
            &env_with_tokens(),
            &mut services,
        );

        assert_eq!(0, result.exit_code);
        assert_eq!(
            Some(PanePlacement::DetachedSession { attach: true }),
            result
                .attachment
                .as_ref()
                .and_then(|session| session.placement)
        );
        assert_eq!(
            vec![
                "create_session_thread",
                "spawn_agent",
                "update_session_thread",
                "register_session"
            ],
            services.calls
        );
        assert_eq!(
            "codex-pingme",
            services.created_session.as_ref().unwrap().session_name
        );
        assert_eq!(
            "codex-pingme",
            services.spawned_session.as_ref().unwrap().session_name
        );
        assert_eq!(
            "/work/pingme",
            services.spawned_session.as_ref().unwrap().cwd
        );
        assert_eq!(
            Some("1757221923.123456"),
            services
                .spawned_session
                .as_ref()
                .unwrap()
                .thread_ts
                .as_deref()
        );
        assert!(
            result
                .stdout
                .contains("https://workspace.slack.com/archives/C/thread")
        );
        let card = services.updated_card.unwrap();
        assert!(card.contains("Agent: codex"));
        assert!(card.contains("Session: codex-pingme"));
        assert!(card.contains("Status: idle"));
        assert!(card.contains("Host: linux"));
        assert!(card.contains("Project: pingme"));
        assert!(card.contains("cwd: /work/pingme"));
        assert!(card.contains("Pane: %7"));
        assert!(card.contains("Thread: 1757221923.123456"));
        let registration = services.registered_session.unwrap();
        assert_eq!("C_TEST_CONTROL", registration.control_channel);
        assert_eq!("U_TEST_OPERATOR", registration.operator);
        assert_eq!(Some("1757221923.123456"), registration.thread_ts.as_deref());
        assert_eq!("codex-pingme", registration.session_name);
        assert_eq!("linux", registration.host);
        assert_eq!("pingme", registration.project);
        assert_eq!("/work/pingme", registration.cwd);
        assert_eq!(Some("%7"), registration.pane_id.as_deref());
        assert_eq!(
            Some("/tmp/pingme-test.sock"),
            registration.notify_socket.as_deref()
        );
        assert_eq!(Some("notify-token"), registration.notify_token.as_deref());
    }

    #[test]
    fn local_grok_uses_the_same_slack_session_flow() {
        let mut services = FakeServices::default();
        let result = run_cli_with_services(
            &["pingme", "grok"],
            SETTING,
            &env_with_tokens(),
            &mut services,
        );

        assert_eq!(0, result.exit_code);
        assert_eq!(
            vec![
                "create_session_thread",
                "spawn_agent",
                "update_session_thread",
                "register_session"
            ],
            services.calls
        );
        assert_eq!("grok", services.created_session.as_ref().unwrap().agent_cli);
        assert_eq!(
            "grok-pingme",
            services.spawned_session.as_ref().unwrap().session_name
        );
        assert_eq!(
            "grok",
            services.registered_session.as_ref().unwrap().agent_cli
        );
        assert!(result.stdout.contains("launching Grok CLI"));
    }

    #[test]
    fn local_launch_uses_an_available_session_name() {
        let mut services = FakeServices {
            next_session_name: Some("codex-pingme-2".to_owned()),
            ..FakeServices::default()
        };

        let result = run_cli_with_services(
            &["pingme", "codex"],
            SETTING,
            &env_with_tokens(),
            &mut services,
        );

        assert_eq!(0, result.exit_code);
        assert_eq!(
            "codex-pingme-2",
            services.created_session.unwrap().session_name
        );
    }

    #[test]
    fn remote_agent_session_start_does_not_attach_a_local_terminal() {
        assert_eq!(PanePlacement::DetachedWindow, pane_placement(false, true));
        assert_eq!(
            PanePlacement::DetachedSession { attach: false },
            pane_placement(false, false)
        );

        for placement in [
            PanePlacement::DetachedWindow,
            PanePlacement::DetachedSession { attach: false },
        ] {
            let mut session = AgentSession::for_launch(
                "codex-pingme",
                "codex",
                "linux",
                "pingme",
                "/work/pingme",
                "C_TEST_CONTROL",
                "U_TEST_OPERATOR",
                placement,
            );
            session.thread_ts = Some("1757221923.123456".to_owned());
            let mut services = FakeServices::default();
            let started = start_agent_session(session, &mut services).unwrap();

            assert_eq!(
                vec!["spawn_agent", "update_session_thread", "register_session"],
                services.calls
            );
            assert_eq!(Some(placement), services.spawned_session.unwrap().placement);
            assert_eq!(Some(placement), started.session.placement);
            assert_ne!(
                Some(PanePlacement::ReuseCurrentPane),
                started.session.placement
            );
        }
    }

    #[test]
    fn local_launch_output_falls_back_to_channel_and_thread_ts() {
        let mut services = FakeServices {
            without_permalink: true,
            ..FakeServices::default()
        };
        let result = run_cli_with_services(
            &["pingme", "codex"],
            SETTING,
            &env_with_tokens(),
            &mut services,
        );

        assert_eq!(0, result.exit_code);
        assert!(result.stdout.contains("C_TEST_CONTROL 1757221923.123456"));
    }

    #[test]
    fn preserves_status_update_warning_with_launched_session() {
        let mut services = FakeServices {
            update_error: Some("cant_update".to_owned()),
            ..FakeServices::default()
        };
        let result = run_cli_with_services(
            &["pingme", "codex"],
            SETTING,
            &env_with_tokens(),
            &mut services,
        );

        assert_eq!(0, result.exit_code);
        assert!(result.stderr.contains("cant_update"));
        assert!(result.attachment.is_some());
    }

    #[test]
    fn local_codex_falls_back_to_unbridged_when_slack_thread_creation_fails() {
        let mut services = FakeServices {
            slack_error: Some("not_in_channel".to_owned()),
            ..FakeServices::default()
        };
        let result = run_cli_with_services(
            &["pingme", "codex"],
            SETTING,
            &env_with_tokens(),
            &mut services,
        );

        assert_eq!(0, result.exit_code);
        assert_eq!(vec!["create_session_thread", "spawn_agent"], services.calls);
        assert_eq!(None, services.spawned_session.unwrap().thread_ts);
        assert!(
            result
                .stderr
                .contains("failed to create Slack Session Thread: not_in_channel")
        );
        assert!(result.stderr.contains("launching unbridged Codex CLI"));
    }

    #[test]
    fn local_codex_falls_back_to_unbridged_when_slack_tokens_are_missing() {
        let mut services = FakeServices::default();
        let result = run_cli_with_services(
            &["pingme", "codex"],
            SETTING,
            &BTreeMap::new(),
            &mut services,
        );

        assert_eq!(0, result.exit_code);
        assert_eq!(vec!["spawn_agent"], services.calls);
        assert_eq!(None, services.spawned_session.unwrap().thread_ts);
        assert!(
            result
                .stderr
                .contains("missing environment variable SLACK_APP_TOKEN")
        );
        assert!(result.stderr.contains("launching unbridged Codex CLI"));
    }

    #[test]
    fn local_codex_falls_back_to_unbridged_when_setting_toml_is_missing() {
        let mut services = FakeServices::default();
        let result =
            run_cli_with_services(&["pingme", "codex"], "", &env_with_tokens(), &mut services);

        assert_eq!(0, result.exit_code);
        assert_eq!(vec!["spawn_agent"], services.calls);
        assert_eq!(None, services.spawned_session.unwrap().thread_ts);
        assert!(result.stderr.contains("setting.toml is missing or empty"));
        assert!(result.stderr.contains("launching unbridged Codex CLI"));
    }

    #[test]
    fn local_codex_forwards_resume_args_to_the_spawned_agent() {
        let mut services = FakeServices::default();
        let result = run_cli_with_services(
            &["pingme", "codex", "resume", "session-id"],
            SETTING,
            &env_with_tokens(),
            &mut services,
        );

        assert_eq!(0, result.exit_code);
        assert_eq!(
            vec!["resume".to_owned(), "session-id".to_owned()],
            services.spawned_session.unwrap().agent_args
        );
    }

    #[test]
    fn local_grok_forwards_resume_args_to_the_spawned_agent() {
        let mut services = FakeServices::default();
        let result = run_cli_with_services(
            &[
                "pingme",
                "grok",
                "--resume",
                "01a090da-d3ba-7bb1-8cc0-aab11beccab6",
            ],
            SETTING,
            &env_with_tokens(),
            &mut services,
        );

        assert_eq!(0, result.exit_code);
        assert_eq!(
            vec![
                "--resume".to_owned(),
                "01a090da-d3ba-7bb1-8cc0-aab11beccab6".to_owned()
            ],
            services.spawned_session.unwrap().agent_args
        );
        assert_eq!(
            vec![
                "--resume".to_owned(),
                "01a090da-d3ba-7bb1-8cc0-aab11beccab6".to_owned()
            ],
            result.attachment.unwrap().agent_args
        );
    }

    #[test]
    fn unbridged_local_grok_forwards_resume_args() {
        let mut services = FakeServices::default();
        let result = run_cli_with_services(
            &[
                "pingme",
                "grok",
                "--resume",
                "01a090da-d3ba-7bb1-8cc0-aab11beccab6",
            ],
            "",
            &env_with_tokens(),
            &mut services,
        );

        assert_eq!(0, result.exit_code);
        assert_eq!(
            vec![
                "--resume".to_owned(),
                "01a090da-d3ba-7bb1-8cc0-aab11beccab6".to_owned()
            ],
            services.spawned_session.unwrap().agent_args
        );
    }

    #[test]
    fn rejects_agent_args_with_control_characters() {
        let result = run_cli(
            &["pingme", "grok", "--resume", "id\nnext"],
            SETTING,
            &env_with_tokens(),
        );

        assert_eq!(2, result.exit_code);
        assert!(
            result
                .stderr
                .contains("Agent CLI arguments contain unsupported control characters")
        );
    }
}

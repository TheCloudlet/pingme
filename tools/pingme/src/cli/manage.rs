use std::collections::BTreeMap;

use pingme_host::services::LocalServices;
use pingme_session::{AgentSession, SessionStatus};

use crate::cli::{
    CliResult, fail, ok, parse_setting, require_slack_tokens, setting_summary, usage,
};

pub(crate) fn manage_or_defer(
    command: &str,
    args: &[&str],
    setting_toml: &str,
    env: &BTreeMap<&str, &str>,
    services: &mut dyn LocalServices,
) -> CliResult {
    if let Err(message) = require_slack_tokens(env) {
        return fail(message);
    }

    let setting = match parse_setting(setting_toml) {
        Ok(setting) => setting,
        Err(message) => return fail(message),
    };

    match command {
        "list" => match services.list_sessions() {
            Ok(sessions) if sessions.is_empty() => {
                ok(format!("{}no Agent Sessions\n", setting_summary(&setting),))
            }
            Ok(sessions) => ok(format!(
                "{}{}",
                setting_summary(&setting),
                session_list(&sessions)
            )),
            Err(error) => fail(format!("failed to list Agent Sessions: {error}\n")),
        },
        "attach" => match args.get(2) {
            Some(session_name) => match services.attach_session(session_name) {
                Ok(()) => ok(format!("attached: {session_name}\n")),
                Err(error) => fail(format!("failed to attach {session_name}: {error}\n")),
            },
            None => fail(format!("missing session-name\n{}", usage())),
        },
        "cleanup" => match services.cleanup_sessions() {
            Ok(count) => ok(format!("cleaned up {count} unavailable Agent Sessions\n")),
            Err(error) => fail(format!("failed to clean up Agent Sessions: {error}\n")),
        },
        _ => unreachable!("command was filtered by caller"),
    }
}

fn session_list(sessions: &[AgentSession]) -> String {
    let mut output = String::new();
    for session in sessions {
        let status = if session.available {
            session.status
        } else {
            SessionStatus::Unavailable
        };
        output.push_str(&format!(
            "{} {} project={} status={} pane={} thread={}\n",
            session.session_name,
            session.agent_cli,
            session.project,
            status.as_str(),
            session.pane_id.as_deref().unwrap_or("pending"),
            session.thread_ts.as_deref().unwrap_or("none")
        ));
    }
    output
}

#[cfg(test)]
mod tests {
    use crate::cli::testing::*;
    use crate::{BTreeMap, run_cli_with_services};

    #[test]
    fn accepts_local_management_commands() {
        let mut services = FakeServices::default();
        let list = run_cli_with_services(
            &["pingme", "list"],
            SETTING,
            &env_with_tokens(),
            &mut services,
        );
        let attach = run_cli_with_services(
            &["pingme", "attach", "codex-pingme-1420"],
            SETTING,
            &env_with_tokens(),
            &mut services,
        );
        let cleanup = run_cli_with_services(
            &["pingme", "cleanup"],
            SETTING,
            &env_with_tokens(),
            &mut services,
        );

        assert_eq!(0, list.exit_code);
        assert!(list.stdout.contains("no Agent Sessions"));
        assert_eq!(0, attach.exit_code);
        assert!(attach.stdout.contains("attached: codex-pingme-1420"));
        assert_eq!(0, cleanup.exit_code);
        assert!(
            cleanup
                .stdout
                .contains("cleaned up 2 unavailable Agent Sessions")
        );
        assert!(services.calls.contains(&"cleanup_sessions"));
    }

    #[test]
    fn parses_setting_toml() {
        let result = run_cli(&["pingme", "list"], SETTING, &env_with_tokens());

        assert_eq!(0, result.exit_code);
        assert!(result.stdout.contains("Host: linux"));
        assert!(result.stdout.contains("Control Channel: C_TEST_CONTROL"));
        assert!(result.stdout.contains("Project: pingme -> /work/pingme"));
    }

    #[test]
    fn reports_missing_slack_tokens_without_leaking_values() {
        let result = run_cli(&["pingme", "list"], SETTING, &BTreeMap::new());

        assert_eq!(2, result.exit_code);
        assert!(
            result
                .stderr
                .contains("missing environment variable SLACK_APP_TOKEN")
        );
        assert!(
            result
                .stderr
                .contains("missing environment variable SLACK_BOT_TOKEN")
        );
        assert!(!result.stderr.contains("xapp-"));
        assert!(!result.stderr.contains("xoxb-"));
    }

    #[test]
    fn reports_missing_setting_toml_for_management_commands() {
        let result = run_cli(&["pingme", "list"], "", &env_with_tokens());

        assert_eq!(2, result.exit_code);
        assert!(result.stderr.contains("setting.toml is missing or empty"));
    }
}

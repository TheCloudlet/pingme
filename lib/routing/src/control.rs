#[derive(Clone, Copy)]
pub struct ControlCommand<'a> {
    pub command: &'a str,
    pub channel_id: &'a str,
    pub user_id: &'a str,
    pub text: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ControlAction {
    NewSession {
        agent_cli: String,
        project_name: String,
        agent_args: Vec<String>,
    },
    Ignore,
    Reject(String),
}

pub fn control_action(
    command: &ControlCommand<'_>,
    control_channel_id: &str,
    operator_id: &str,
) -> ControlAction {
    if command.command != "/pingme" || command.channel_id != control_channel_id {
        return ControlAction::Ignore;
    }
    if command.user_id != operator_id {
        return ControlAction::Reject("Not executed: unauthorized Slack user.".to_owned());
    }
    let mut words = command.text.split_whitespace();
    let agent_cli = words.next().unwrap_or("").to_owned();
    let project_name = words.next().unwrap_or("").to_owned();
    if agent_cli.is_empty() || project_name.is_empty() {
        return ControlAction::Reject(
            "usage: /pingme <codex|grok> <project> [agent-args...]".to_owned(),
        );
    }
    ControlAction::NewSession {
        agent_cli,
        project_name,
        agent_args: words.map(str::to_owned).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_pingme_control_command() {
        let command = ControlCommand {
            command: "/pingme",
            channel_id: "C_TEST_CONTROL",
            user_id: "U_TEST_OPERATOR",
            text: "codex pingme",
        };

        assert_eq!(
            ControlAction::NewSession {
                agent_cli: "codex".to_owned(),
                project_name: "pingme".to_owned(),
                agent_args: Vec::new(),
            },
            control_action(&command, "C_TEST_CONTROL", "U_TEST_OPERATOR")
        );
        assert_eq!(
            ControlAction::Reject("Not executed: unauthorized Slack user.".to_owned()),
            control_action(
                &ControlCommand {
                    user_id: "UOTHER",
                    ..command
                },
                "C_TEST_CONTROL",
                "U_TEST_OPERATOR",
            )
        );
        assert_eq!(
            ControlAction::Ignore,
            control_action(
                &ControlCommand {
                    channel_id: "COTHER",
                    ..command
                },
                "C_TEST_CONTROL",
                "U_TEST_OPERATOR",
            )
        );
        assert_eq!(
            ControlAction::Ignore,
            control_action(
                &ControlCommand {
                    command: "/cli-new",
                    ..command
                },
                "C_TEST_CONTROL",
                "U_TEST_OPERATOR",
            )
        );
    }

    #[test]
    fn pingme_slash_forwards_codex_resume_after_the_project() {
        let command = ControlCommand {
            command: "/pingme",
            channel_id: "C_TEST_CONTROL",
            user_id: "U_TEST_OPERATOR",
            text: "codex pingme resume session-id",
        };

        assert_eq!(
            ControlAction::NewSession {
                agent_cli: "codex".to_owned(),
                project_name: "pingme".to_owned(),
                agent_args: vec!["resume".to_owned(), "session-id".to_owned()],
            },
            control_action(&command, "C_TEST_CONTROL", "U_TEST_OPERATOR")
        );
    }

    #[test]
    fn pingme_slash_forwards_agent_args_after_the_project() {
        let command = ControlCommand {
            command: "/pingme",
            channel_id: "C_TEST_CONTROL",
            user_id: "U_TEST_OPERATOR",
            text: "grok pingme --resume 01a090da-d3ba-7bb1-8cc0-aab11beccab6",
        };

        assert_eq!(
            ControlAction::NewSession {
                agent_cli: "grok".to_owned(),
                project_name: "pingme".to_owned(),
                agent_args: vec![
                    "--resume".to_owned(),
                    "01a090da-d3ba-7bb1-8cc0-aab11beccab6".to_owned()
                ],
            },
            control_action(&command, "C_TEST_CONTROL", "U_TEST_OPERATOR")
        );
    }
}

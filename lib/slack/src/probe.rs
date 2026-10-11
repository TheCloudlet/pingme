use serde_json::Value;

use crate::api::slack_ok;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ControlChannelProbe {
    pub is_private: bool,
    pub is_member: bool,
}

pub fn control_channel_probe(body: &Value) -> Result<ControlChannelProbe, String> {
    // The bot only holds groups:read, so Slack refuses to describe a public channel.
    // Membership is unknown here; doctor reports a public channel before membership.
    let needed = body["needed"].as_str().unwrap_or("");
    if body["error"] == "missing_scope" && needed.split(',').any(|scope| scope == "channels:read") {
        return Ok(ControlChannelProbe {
            is_private: false,
            is_member: false,
        });
    }
    slack_ok(body)?;
    let flag = |name| body["channel"][name].as_bool().unwrap_or(false);
    Ok(ControlChannelProbe {
        is_private: flag("is_private"),
        is_member: flag("is_member"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_public_control_channel_reads_as_public_not_as_a_missing_scope() {
        let body = serde_json::json!({
            "ok": false,
            "error": "missing_scope",
            "needed": "channels:read",
        });

        assert_eq!(
            Ok(ControlChannelProbe {
                is_private: false,
                is_member: false,
            }),
            control_channel_probe(&body)
        );
    }

    #[test]
    fn a_private_control_channel_reports_membership() {
        let body = serde_json::json!({
            "ok": true,
            "channel": { "is_private": true, "is_member": true },
        });

        assert_eq!(
            Ok(ControlChannelProbe {
                is_private: true,
                is_member: true,
            }),
            control_channel_probe(&body)
        );
    }
}

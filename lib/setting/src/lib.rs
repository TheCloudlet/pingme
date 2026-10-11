//! `setting.toml`: the Host, its Control Channel and Operator, and its Projects.

use std::ffi::OsStr;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub struct HostSettings {
    pub host: HostSetting,
    pub slack: SlackSetting,
    #[serde(default)]
    pub projects: Vec<ProjectSetting>,
}

#[derive(Clone, Deserialize)]
pub struct HostSetting {
    pub name: String,
}

#[derive(Clone, Deserialize)]
pub struct SlackSetting {
    pub control_channel_id: String,
    pub operator_id: String,
}

#[derive(Clone, Deserialize)]
pub struct ProjectSetting {
    pub name: String,
    pub cwd: String,
}

impl HostSettings {
    pub fn parse(toml: &str) -> Result<Self, String> {
        toml::from_str(toml).map_err(|error| format!("failed to parse setting.toml: {error}"))
    }
}

pub fn setting_path() -> PathBuf {
    setting_path_from_home(std::env::var_os("PINGME_HOME").as_deref())
}

fn setting_path_from_home(home: Option<&OsStr>) -> PathBuf {
    home.map(PathBuf::from)
        .unwrap_or_default()
        .join("setting.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pingme_home_selects_setting_toml_without_changing_the_project_directory() {
        assert_eq!(
            std::path::Path::new("/opt/pingme/setting.toml"),
            setting_path_from_home(Some(std::ffi::OsStr::new("/opt/pingme")))
        );
        assert_eq!(
            std::path::Path::new("setting.toml"),
            setting_path_from_home(None)
        );
    }
}

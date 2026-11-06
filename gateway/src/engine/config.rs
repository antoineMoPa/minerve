//! Settings in `~/.minerve/message-apps.toml`; tokens in `~/.minerve/keys.json`, next to the
//! model API keys and like them owner-only.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct MessageAppsConfig {
    /// `<transport>:<sender id>` entries, written when someone links with a code.
    #[serde(default)]
    pub allowed: Vec<String>,
    pub slack: Option<SlackSettings>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SlackSettings {
    pub bot_name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SlackTokens {
    pub app_token: String,
    pub bot_token: String,
}

fn minerve_dir() -> PathBuf {
    dirs::home_dir().expect("no home directory").join(".minerve")
}

pub fn config_path() -> PathBuf {
    minerve_dir().join("message-apps.toml")
}

fn keys_path() -> PathBuf {
    minerve_dir().join("keys.json")
}

/// A missing file means nothing is set up; a broken one is an error, so it is never overwritten blindly.
pub fn load() -> Result<MessageAppsConfig, String> {
    load_from(&config_path())
}

pub fn load_from(path: &Path) -> Result<MessageAppsConfig, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|err| format!("{}: {}", path.display(), err)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(MessageAppsConfig::default()),
        Err(err) => Err(format!("{}: {}", path.display(), err)),
    }
}

pub fn save(config: &MessageAppsConfig) -> Result<(), String> {
    save_to(&config_path(), config)
}

pub fn save_to(path: &Path, config: &MessageAppsConfig) -> Result<(), String> {
    let text = toml::to_string(config).map_err(|err| err.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    }
    std::fs::write(path, text).map_err(|err| format!("{}: {}", path.display(), err))
}

/// From `SLACK_APP_TOKEN` / `SLACK_BOT_TOKEN`, else from keys.json.
pub fn load_slack_tokens() -> Option<SlackTokens> {
    let keys = read_keys();
    let pick = |env: &str, key: &str| {
        std::env::var(env)
            .ok()
            .or_else(|| keys.get(key).and_then(|value| value.as_str()).map(String::from))
            .filter(|token| !token.trim().is_empty())
    };
    Some(SlackTokens { app_token: pick("SLACK_APP_TOKEN", "slack_app_token")?, bot_token: pick("SLACK_BOT_TOKEN", "slack_bot_token")? })
}

/// Stores the given tokens in keys.json, keeping every other key there. `None` keeps the stored one.
pub fn save_slack_tokens(app_token: Option<&str>, bot_token: Option<&str>) -> Result<(), String> {
    let mut keys = read_keys();
    if let Some(token) = app_token {
        keys.insert("slack_app_token".into(), token.into());
    }
    if let Some(token) = bot_token {
        keys.insert("slack_bot_token".into(), token.into());
    }
    let path = keys_path();
    std::fs::create_dir_all(minerve_dir()).map_err(|err| err.to_string())?;
    let json = serde_json::to_string_pretty(&keys).map_err(|err| err.to_string())?;
    std::fs::write(&path, json + "\n").map_err(|err| format!("{}: {}", path.display(), err))?;
    set_owner_only(&path).map_err(|err| err.to_string())
}

fn read_keys() -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(keys_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_from_before_workdir_was_dropped_still_loads() {
        let config: MessageAppsConfig = toml::from_str("allowed = []\n[slack]\nbot_name = \"b\"\nworkdir = \"/tmp\"\n").unwrap();
        assert_eq!(config.slack, Some(SlackSettings { bot_name: "b".into() }));
    }

    #[test]
    fn round_trips_through_toml() {
        let path = std::env::temp_dir().join(format!("message-apps-{}.toml", uuid::Uuid::new_v4()));
        assert_eq!(load_from(&path).unwrap(), MessageAppsConfig::default());
        let config = MessageAppsConfig {
            allowed: vec!["slack:U1".into()],
            slack: Some(SlackSettings { bot_name: "antoine-bot".into() }),
        };
        save_to(&path, &config).unwrap();
        assert_eq!(load_from(&path).unwrap(), config);
        let _ = std::fs::remove_file(path);
    }
}

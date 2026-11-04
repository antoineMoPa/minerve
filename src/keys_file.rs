//! API keys entered in the setup modal, saved as `~/.minerve/keys.json`.
//! `~/.env` is still read first and is never written by setup. Keys here fill only what `.env` and
//! the process environment left unset.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const KEYS_FILE_PATH: &str = ".minerve/keys.json";

/// Keys as saved on disk. A missing or empty field means no key for that provider.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct SavedKeys {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openrouter_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_api_key: Option<String>,
}

fn keys_file() -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(KEYS_FILE_PATH)
}

/// Reads the saved keys. A missing or unreadable file means none are saved.
pub fn load() -> SavedKeys {
    fs::read_to_string(keys_file())
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

/// Writes the keys to `~/.minerve/keys.json`, owner-only on unix. Replaces the file.
pub fn save(keys: &SavedKeys) -> std::io::Result<PathBuf> {
    let path = keys_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(keys).map_err(std::io::Error::other)?;
    fs::write(&path, json + "\n")?;
    set_owner_only(&path)?;
    Ok(path)
}

#[cfg(unix)]
fn set_owner_only(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_owner_only(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_only_set_keys() {
        let keys = SavedKeys {
            anthropic_api_key: Some(String::from("sk-ant-1")),
            ..SavedKeys::default()
        };
        let json = serde_json::to_string(&keys).unwrap();
        assert_eq!(json, r#"{"anthropic_api_key":"sk-ant-1"}"#);
        assert_eq!(serde_json::from_str::<SavedKeys>(&json).unwrap(), keys);
    }
}

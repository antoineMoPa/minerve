//! Maps a conversation to a minerve session id, persisted in `~/.minerve/message-apps-sessions.json`.
//!
//! A private conversation never carries on in a session that started more than a week ago: its
//! next message starts a new one, so the agent never sees anything older than that.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::types::ConversationKey;

/// How long a private conversation's session may run before it is started again.
pub const PRIVATE_SESSION_MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    /// When the session started, in unix seconds. `0` when unknown (written before this was
    /// recorded): such a private session is started again, rather than trusted to be recent.
    #[serde(default)]
    pub started_unix: u64,
}

pub struct SessionStore {
    path: PathBuf,
    map: Mutex<HashMap<String, Entry>>,
}

impl SessionStore {
    pub fn open(path: PathBuf) -> Self {
        Self { map: Mutex::new(read_entries(&path)), path }
    }

    pub fn default_path() -> PathBuf {
        dirs::home_dir().expect("no home directory").join(".minerve/message-apps-sessions.json")
    }

    /// The session of a conversation, picked the first time the conversation is seen. A private
    /// conversation whose session is over a week old gets a new one.
    pub fn session_for(&self, key: &ConversationKey, is_private: bool) -> String {
        self.session_for_at(key, is_private, now_unix())
    }

    fn session_for_at(&self, key: &ConversationKey, is_private: bool, now: u64) -> String {
        let mut map = self.map.lock().unwrap();
        let name = key.to_string();
        if let Some(entry) = map.get(&name) {
            let expired = now.saturating_sub(entry.started_unix) > PRIVATE_SESSION_MAX_AGE_SECS;
            if !(is_private && (entry.started_unix == 0 || expired)) {
                return entry.id.clone();
            }
        }
        let entry = Entry { id: uuid::Uuid::new_v4().to_string(), started_unix: now };
        let id = entry.id.clone();
        map.insert(name, entry);
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.path, serde_json::to_string_pretty(&*map).unwrap());
        id
    }
}

/// Reads the file, which was once a plain `conversation -> session id` map: those entries load
/// with an unknown start.
fn read_entries(path: &PathBuf) -> HashMap<String, Entry> {
    let Some(value) = std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    else {
        return HashMap::new();
    };
    let Some(object) = value.as_object() else { return HashMap::new() };
    object
        .iter()
        .filter_map(|(name, entry)| {
            let entry = match entry {
                serde_json::Value::String(id) => Entry { id: id.clone(), started_unix: 0 },
                other => serde_json::from_value(other.clone()).ok()?,
            };
            Some((name.clone(), entry))
        })
        .collect()
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(channel: &str) -> ConversationKey {
        ConversationKey { transport: "t".into(), workspace: "w".into(), channel: channel.into(), thread: None }
    }

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!("gw-sessions-{}.json", uuid::Uuid::new_v4()))
    }

    #[test]
    fn same_conversation_keeps_its_session() {
        let path = temp_path();
        let store = SessionStore::open(path.clone());
        let id = store.session_for(&key("a"), false);
        assert_eq!(store.session_for(&key("a"), false), id);
        assert_ne!(store.session_for(&key("b"), false), id);
        // Survives a restart.
        assert_eq!(SessionStore::open(path.clone()).session_for(&key("a"), false), id);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_private_session_older_than_a_week_is_started_again() {
        let path = temp_path();
        let store = SessionStore::open(path.clone());
        let now = 1_000_000_000;
        let id = store.session_for_at(&key("dm"), true, now);
        let week = PRIVATE_SESSION_MAX_AGE_SECS;
        assert_eq!(store.session_for_at(&key("dm"), true, now + week), id, "exactly a week is still kept");
        let fresh = store.session_for_at(&key("dm"), true, now + week + 1);
        assert_ne!(fresh, id, "older than a week starts again");
        assert_eq!(store.session_for_at(&key("dm"), true, now + week + 2), fresh);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn channels_keep_their_session_however_old() {
        let path = temp_path();
        let store = SessionStore::open(path.clone());
        let id = store.session_for_at(&key("chan"), false, 1_000);
        assert_eq!(store.session_for_at(&key("chan"), false, 1_000 + 10 * PRIVATE_SESSION_MAX_AGE_SECS), id);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_old_plain_entry_is_started_again_when_private() {
        let path = temp_path();
        let legacy = format!(r#"{{"{}": "legacy-id"}}"#, key("dm"));
        std::fs::write(&path, legacy).unwrap();
        let store = SessionStore::open(path.clone());
        assert_ne!(store.session_for_at(&key("dm"), true, 5), "legacy-id", "a private one restarts");
        assert_eq!(SessionStore::open(path.clone()).session_for_at(&key("dm"), false, 5).len(), 36);
        let _ = std::fs::remove_file(path);
    }
}

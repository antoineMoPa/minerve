//! Maps a conversation to a minerve session id, persisted in `~/.minerve/message-apps-sessions.json`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use super::types::ConversationKey;

pub struct SessionStore {
    path: PathBuf,
    map: Mutex<HashMap<String, String>>,
}

impl SessionStore {
    pub fn open(path: PathBuf) -> Self {
        let map = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self { path, map: Mutex::new(map) }
    }

    pub fn default_path() -> PathBuf {
        dirs::home_dir().expect("no home directory").join(".minerve/message-apps-sessions.json")
    }

    /// The session of a conversation, picked the first time the conversation is seen.
    pub fn session_for(&self, key: &ConversationKey) -> String {
        let mut map = self.map.lock().unwrap();
        if let Some(id) = map.get(&key.to_string()) {
            return id.clone();
        }
        let id = uuid::Uuid::new_v4().to_string();
        map.insert(key.to_string(), id.clone());
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.path, serde_json::to_string_pretty(&*map).unwrap());
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(channel: &str) -> ConversationKey {
        ConversationKey { transport: "t".into(), workspace: "w".into(), channel: channel.into(), thread: None }
    }

    #[test]
    fn same_conversation_keeps_its_session() {
        let path = std::env::temp_dir().join(format!("gw-sessions-{}.json", uuid::Uuid::new_v4()));
        let store = SessionStore::open(path.clone());
        let id = store.session_for(&key("a"));
        assert_eq!(store.session_for(&key("a")), id);
        assert_ne!(store.session_for(&key("b")), id);
        // Survives a restart.
        assert_eq!(SessionStore::open(path.clone()).session_for(&key("a")), id);
        let _ = std::fs::remove_file(path);
    }
}

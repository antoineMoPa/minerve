//! Resumable sessions. A session is the conversation kept in `Minerve.messages`, saved as
//! `~/.minerve/sessions/<uuid>.json` after every completed turn.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::chat::ChatCompletionMessage;

pub const SESSIONS_PATH: &str = ".minerve/sessions";

/// Value of `-r` when no session id is given: resume the most recently updated session.
pub const LATEST: &str = "latest";

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionFile {
    pub id: String,
    pub updated_at: String,
    pub messages: Vec<ChatCompletionMessage>,
}

pub fn sessions_dir() -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(SESSIONS_PATH)
}

fn session_path(id: &str) -> PathBuf {
    sessions_dir().join(format!("{}.json", id))
}

pub fn save_session(id: &str, messages: &[ChatCompletionMessage]) -> std::io::Result<()> {
    fs::create_dir_all(sessions_dir())?;
    let file = SessionFile {
        id: id.to_string(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        messages: messages.to_vec(),
    };
    let json = serde_json::to_string_pretty(&file).map_err(std::io::Error::other)?;
    fs::write(session_path(id), json)
}

/// Id of the session file modified most recently, if any.
fn latest_session_id() -> Option<String> {
    fs::read_dir(sessions_dir())
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            let id = entry.path().file_stem()?.to_str()?.to_string();
            Some((modified, id))
        })
        .max()
        .map(|(_, id)| id)
}

/// Loads the session named by `-r`: either a session id or `LATEST`.
pub fn load_for_resume(arg: &str) -> Result<SessionFile, String> {
    let id = if arg == LATEST {
        latest_session_id().ok_or_else(|| {
            format!(
                "no session to resume in ~/{}",
                SESSIONS_PATH
            )
        })?
    } else {
        arg.to_string()
    };
    let content = fs::read_to_string(session_path(&id))
        .map_err(|err| format!("cannot read session {}: {}", id, err))?;
    serde_json::from_str(&content).map_err(|err| format!("invalid session {}: {}", id, err))
}

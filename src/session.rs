//! Resumable sessions. A session is the conversation kept in `Minerve.messages`, saved as
//! `~/.minerve/sessions/<uuid>.json` after every completed turn, with its usage totals.

use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::chat::{ChatCompletionMessage, ChatCompletionMessageRole};
use crate::token_counter::UsageSnapshot;

pub const SESSIONS_PATH: &str = ".minerve/sessions";

/// Value of `-r` when no session id is given: resume the most recently updated session.
pub const LATEST: &str = "latest";

/// Longest first prompt shown in the `/resume` list, in chars.
const FIRST_PROMPT_CHARS: usize = 60;

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionFile {
    pub id: String,
    pub updated_at: String,
    pub messages: Vec<ChatCompletionMessage>,
    /// Usage totals of the session. Files saved before usage was stored have none.
    #[serde(default)]
    pub usage: UsageSnapshot,
}

/// One row of the `/resume` list.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    pub id: String,
    pub modified: SystemTime,
    /// Start of the first prompt the user typed, one line. Empty when there is none yet.
    pub first_prompt: String,
}

pub fn sessions_dir() -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(SESSIONS_PATH)
}

fn session_path(id: &str) -> PathBuf {
    sessions_dir().join(format!("{}.json", id))
}

pub fn save_session(
    id: &str,
    messages: &[ChatCompletionMessage],
    usage: UsageSnapshot,
) -> std::io::Result<()> {
    fs::create_dir_all(sessions_dir())?;
    let file = SessionFile {
        id: id.to_string(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        messages: messages.to_vec(),
        usage,
    };
    let json = serde_json::to_string_pretty(&file).map_err(std::io::Error::other)?;
    fs::write(session_path(id), json)
}

/// True when `id` is a UUID written the way `save_session` names files: lowercase, hyphenated.
/// Anything else could point outside the sessions folder, so it is refused before touching the disk.
fn is_session_id(id: &str) -> bool {
    Uuid::parse_str(id).is_ok_and(|uuid| uuid.hyphenated().to_string() == id)
}

/// Every saved session, most recently updated first. Files that are not sessions are skipped.
pub fn list_sessions() -> Vec<SessionSummary> {
    let Ok(entries) = fs::read_dir(sessions_dir()) else {
        return Vec::new();
    };
    let mut summaries: Vec<SessionSummary> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            let id = entry.path().file_stem()?.to_str()?.to_string();
            if !is_session_id(&id) {
                return None;
            }
            let file = read_session_file(&id).ok()?;
            Some(SessionSummary {
                id,
                modified,
                first_prompt: first_prompt_of(&file.messages),
            })
        })
        .collect();
    summaries.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.id.cmp(&b.id)));
    summaries
}

/// First user message, on one line and cut to `FIRST_PROMPT_CHARS`.
fn first_prompt_of(messages: &[ChatCompletionMessage]) -> String {
    let Some(content) = messages
        .iter()
        .find(|msg| matches!(msg.role, ChatCompletionMessageRole::User))
        .and_then(|msg| msg.content.as_deref())
    else {
        return String::new();
    };
    let one_line = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > FIRST_PROMPT_CHARS {
        let head: String = one_line.chars().take(FIRST_PROMPT_CHARS).collect();
        format!("{}...", head)
    } else {
        one_line
    }
}

/// Id of the session file modified most recently, if any.
fn latest_session_id() -> Option<String> {
    list_sessions().into_iter().next().map(|summary| summary.id)
}

fn read_session_file(id: &str) -> Result<SessionFile, String> {
    let content = fs::read_to_string(session_path(id))
        .map_err(|err| format!("cannot read session {}: {}", id, err))?;
    serde_json::from_str(&content).map_err(|err| format!("invalid session {}: {}", id, err))
}

/// Loads the session named by `-r`: either a session id or `LATEST`.
/// Fails, rather than starting a new session, when the id is not a UUID or has no file.
pub fn load_for_resume(arg: &str) -> Result<SessionFile, String> {
    if arg == LATEST {
        let id = latest_session_id()
            .ok_or_else(|| format!("no session to resume in ~/{}", SESSIONS_PATH))?;
        return read_session_file(&id);
    }
    if !is_session_id(arg) {
        return Err(format!(
            "invalid session id {:?}: expected a UUID like the one minerve prints on exit",
            arg
        ));
    }
    let file = read_session_file(arg)?;
    if file.id != arg {
        return Err(format!(
            "session file {}.json holds session {}",
            arg, file.id
        ));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_lowercase_hyphenated_uuids() {
        assert!(is_session_id("0e9af09f-378a-4f56-9f66-49d8a7bec847"));
        assert!(!is_session_id("0E9AF09F-378A-4F56-9F66-49D8A7BEC847"));
        assert!(!is_session_id("0e9af09f378a4f569f669d8a7bec847"));
        assert!(!is_session_id("../../etc/passwd"));
        assert!(!is_session_id("latest-but-not-a-uuid"));
        assert!(!is_session_id(""));
    }

    #[test]
    fn refuses_a_bad_id_before_reading_any_file() {
        let err = load_for_resume("../secret").unwrap_err();
        assert!(err.contains("invalid session id"), "{}", err);
    }

    #[test]
    fn first_prompt_is_one_line_and_cut() {
        let long = format!("first line\n{}", "word ".repeat(40));
        let messages = vec![
            ChatCompletionMessage::system(String::from("system")),
            ChatCompletionMessage::user(long),
        ];
        let prompt = first_prompt_of(&messages);
        assert!(prompt.starts_with("first line word word"), "{}", prompt);
        assert!(!prompt.contains('\n'));
        assert!(prompt.ends_with("..."));
        assert_eq!(prompt.chars().count(), FIRST_PROMPT_CHARS + 3);
    }

    #[test]
    fn first_prompt_is_empty_without_a_user_message() {
        let messages = vec![ChatCompletionMessage::system(String::from("system"))];
        assert_eq!(first_prompt_of(&messages), "");
    }
}

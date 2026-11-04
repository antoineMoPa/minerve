//! Least-recently-used list of OpenRouter model ids picked with `/model`, shown at the top of the picker.
//! Saved as `~/.minerve/recent_models.json`, newest first.

use std::fs;
use std::path::PathBuf;

pub const RECENT_MODELS_PATH: &str = ".minerve/recent_models.json";
/// How many models the picker keeps at the top.
pub const RECENT_MODELS_LIMIT: usize = 4;

pub struct RecentModels {
    /// Most recently used first. Never longer than `RECENT_MODELS_LIMIT`.
    ids: Vec<String>,
}

fn recent_models_file() -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(RECENT_MODELS_PATH)
}

impl RecentModels {
    /// Reads the saved list. A missing or unreadable file means nothing was used yet.
    pub fn load() -> Self {
        let ids = fs::read_to_string(recent_models_file())
            .ok()
            .and_then(|content| serde_json::from_str::<Vec<String>>(&content).ok())
            .unwrap_or_default();
        let mut recent = Self { ids };
        recent.ids.truncate(RECENT_MODELS_LIMIT);
        recent
    }

    /// A list holding `ids` as given, most recently used first. Does not touch the saved file.
    #[cfg(test)]
    pub fn from_ids(ids: Vec<String>) -> Self {
        Self { ids }
    }

    /// Most recently used first.
    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    /// The model picked last with `/model`, if any was saved.
    pub fn last_picked(&self) -> Option<&str> {
        self.ids.first().map(String::as_str)
    }

    /// Moves `model_id` to the front, dropping the oldest entry when the list is full, and saves.
    pub fn mark_used(&mut self, model_id: &str) {
        self.move_to_front(model_id);
        self.save();
    }

    fn move_to_front(&mut self, model_id: &str) {
        self.ids.retain(|id| id != model_id);
        self.ids.insert(0, model_id.to_string());
        self.ids.truncate(RECENT_MODELS_LIMIT);
    }

    /// Failures are ignored: losing the list must not interrupt the chat.
    fn save(&self) {
        let path = recent_models_file();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string(&self.ids) {
            let _ = fs::write(path, json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recent_with(ids: &[&str]) -> RecentModels {
        RecentModels {
            ids: ids.iter().map(|id| id.to_string()).collect(),
        }
    }

    #[test]
    fn newest_model_goes_first() {
        let mut recent = recent_with(&["a", "b"]);
        recent.move_to_front("c");
        assert_eq!(recent.ids(), ["c", "a", "b"]);
    }

    #[test]
    fn reused_model_moves_to_front_without_duplicate() {
        let mut recent = recent_with(&["a", "b", "c"]);
        recent.move_to_front("b");
        assert_eq!(recent.ids(), ["b", "a", "c"]);
    }

    #[test]
    fn list_keeps_only_limit_entries() {
        let mut recent = recent_with(&["a", "b", "c", "d"]);
        recent.move_to_front("e");
        assert_eq!(recent.ids(), ["e", "a", "b", "c"]);
    }
}

use std::sync::{Arc, Mutex};

use crate::HISTORY_PATH;

pub struct HistoryTracker {
    previous_prompts: Arc<Mutex<Vec<String>>>,
    index: Option<usize>,
    /// The unsent text that was in the input when history browsing started.
    /// Down past the newest prompt gives it back.
    draft: String,
}

impl HistoryTracker {
    pub fn new() -> Self {
        let mut tracker = Self {
            previous_prompts: Arc::new(Mutex::new(vec![])),
            index: None,
            draft: String::new(),
        };
        tracker.load_history();
        tracker
    }

    pub fn load_history(&mut self) {
        let history_path = dirs::home_dir().unwrap().join(HISTORY_PATH);
        if history_path.exists() {
            let content = std::fs::read_to_string(&history_path).unwrap_or_default();
            let prompts: Vec<String> = serde_json::from_str(&content).unwrap_or_else(|_| vec![]);
            *self.previous_prompts.lock().unwrap() = prompts;
        }
    }

    pub fn save_history(&self) {
        let history_path = dirs::home_dir().unwrap().join(HISTORY_PATH);
        if let Ok(json) = serde_json::to_string(&*self.previous_prompts.lock().unwrap()) {
            let _ = std::fs::write(history_path, json);
        }
    }

    pub fn add_prompt(&mut self, prompt: String) {
        {
            let mut prompts = self.previous_prompts.lock().unwrap();
            if let Some(last) = prompts.last() {
                if last == &prompt {
                    // skip duplicate subsequent prompt
                    return;
                }
            }
            prompts.push(prompt);
        }
        self.index = None;
        self.draft.clear();
        self.save_history();
    }

    /// Up: steps back through older prompts. The first step keeps `current`
    /// as the draft so Down can bring it back.
    pub fn get_previous_prompt(&mut self, current: &str) -> Option<String> {
        let prompts = self.previous_prompts.lock().unwrap();

        if prompts.is_empty() {
            return None;
        }

        let next = match self.index {
            None => {
                self.draft = current.to_string();
                prompts.len() - 1
            }
            Some(0) => 0, // stay at the oldest
            Some(i) => i - 1,
        };
        self.index = Some(next);
        prompts.get(next).cloned()
    }

    /// Down: steps forward through newer prompts. Past the newest one it gives
    /// back the draft. Does nothing when history is not being browsed.
    pub fn get_next_prompt(&mut self) -> Option<String> {
        let prompts = self.previous_prompts.lock().unwrap();
        let i = self.index?;

        if i + 1 >= prompts.len() {
            self.index = None;
            return Some(std::mem::take(&mut self.draft));
        }
        self.index = Some(i + 1);
        prompts.get(i + 1).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker(prompts: &[&str]) -> HistoryTracker {
        HistoryTracker {
            previous_prompts: Arc::new(Mutex::new(prompts.iter().map(|p| p.to_string()).collect())),
            index: None,
            draft: String::new(),
        }
    }

    #[test]
    fn down_after_up_restores_the_unsent_draft() {
        let mut history = tracker(&["first", "second"]);
        assert_eq!(history.get_previous_prompt("half typed").as_deref(), Some("second"));
        assert_eq!(history.get_previous_prompt("second").as_deref(), Some("first"));
        assert_eq!(history.get_next_prompt().as_deref(), Some("second"));
        assert_eq!(history.get_next_prompt().as_deref(), Some("half typed"));
        assert_eq!(history.get_next_prompt(), None);
    }

    #[test]
    fn down_without_browsing_does_nothing() {
        let mut history = tracker(&["first"]);
        assert_eq!(history.get_next_prompt(), None);
    }

    #[test]
    fn up_stays_at_the_oldest_prompt_and_keeps_the_draft() {
        let mut history = tracker(&["only"]);
        assert_eq!(history.get_previous_prompt("draft").as_deref(), Some("only"));
        assert_eq!(history.get_previous_prompt("only").as_deref(), Some("only"));
        assert_eq!(history.get_next_prompt().as_deref(), Some("draft"));
    }
}

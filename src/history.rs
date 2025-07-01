use std::sync::{Arc, Mutex};

use crate::HISTORY_PATH;

pub struct HistoryTracker {
    previous_prompts: Arc<Mutex<Vec<String>>>,
    index: Option<usize>,
}

impl HistoryTracker {
    pub fn new() -> Self {
        let mut tracker = Self {
            previous_prompts: Arc::new(Mutex::new(vec![])),
            index: None,
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
        self.save_history();
    }

    pub fn get_previous_prompt(&mut self) -> Option<String> {
        let prompts = self.previous_prompts.lock().unwrap();

        if prompts.is_empty() {
            return None;
        }

        self.index = match self.index {
            None => Some(prompts.len().saturating_sub(1)),
            Some(0) => Some(0), // stay at the oldest
            Some(i) => Some(i - 1),
        };

        self.index.and_then(|i| prompts.get(i).cloned())
    }

    pub fn get_next_prompt(&mut self) -> Option<String> {
        let prompts = self.previous_prompts.lock().unwrap();

        if prompts.is_empty() {
            return None;
        }

        match self.index {
            None => Some(String::new()), // already at fresh input
            Some(i) if i + 1 >= prompts.len() => {
                self.index = None;
                Some(String::new()) // move out of history
            }
            Some(i) => {
                self.index = Some(i + 1);
                prompts.get(i + 1).cloned()
            }
        }
    }
}

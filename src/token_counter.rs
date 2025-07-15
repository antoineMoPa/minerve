use std::sync::atomic::{AtomicUsize, Ordering};

pub struct TokenCounter {
    prompt_tokens: AtomicUsize,
    completion_tokens: AtomicUsize,
}

impl TokenCounter {
    pub fn new() -> Self {
        TokenCounter {
            prompt_tokens: AtomicUsize::new(0),
            completion_tokens: AtomicUsize::new(0),
        }
    }

    pub fn increment_prompt(&self, count: usize) {
        self.prompt_tokens.fetch_add(count, Ordering::SeqCst);
    }

    pub fn increment_completion(&self, count: usize) {
        self.completion_tokens.fetch_add(count, Ordering::SeqCst);
    }

    pub fn current_prompt(&self) -> usize {
        self.prompt_tokens.load(Ordering::SeqCst)
    }

    pub fn current_completion(&self) -> usize {
        self.completion_tokens.load(Ordering::SeqCst)
    }
}
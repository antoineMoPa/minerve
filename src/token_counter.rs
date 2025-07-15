use std::sync::{atomic::{AtomicUsize, Ordering}, Arc, OnceLock};

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

static GLOBAL_TOKEN_COUNTER: OnceLock<Arc<TokenCounter>> = OnceLock::new();

pub fn get_global_token_counter() -> Arc<TokenCounter> {
    GLOBAL_TOKEN_COUNTER.get_or_init(|| Arc::new(TokenCounter::new())).clone()
}

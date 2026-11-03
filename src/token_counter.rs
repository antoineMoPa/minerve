use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex, OnceLock,
};

pub struct TokenCounter {
    prompt_tokens: AtomicUsize,
    completion_tokens: AtomicUsize,
    cost_usd: Mutex<f64>,
}

impl TokenCounter {
    pub fn new() -> Self {
        TokenCounter {
            prompt_tokens: AtomicUsize::new(0),
            completion_tokens: AtomicUsize::new(0),
            cost_usd: Mutex::new(0.0),
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

    pub fn increment_cost_usd(&self, cost_usd: f64) {
        *self.cost_usd.lock().unwrap() += cost_usd;
    }

    pub fn current_cost_usd(&self) -> f64 {
        *self.cost_usd.lock().unwrap()
    }

    /// One line for the TUI status bar and the end of headless runs.
    pub fn format_usage_summary(&self) -> String {
        format!(
            "Sent: {} | Received: {} | Cost: ${:.4}",
            self.current_prompt(),
            self.current_completion(),
            self.current_cost_usd()
        )
    }
}

static GLOBAL_TOKEN_COUNTER: OnceLock<Arc<TokenCounter>> = OnceLock::new();

pub fn get_global_token_counter() -> Arc<TokenCounter> {
    GLOBAL_TOKEN_COUNTER
        .get_or_init(|| Arc::new(TokenCounter::new()))
        .clone()
}

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

/// Formats a token count with k (thousands) or m (millions), e.g. 512 -> "512", 15300 -> "15.3k", 1000000 -> "1.0m".
pub fn format_tokens(tokens: usize) -> String {
    // 999_950 and up would round to "1000.0k", so those show as millions.
    if tokens >= 999_950 {
        format!("{:.1}m", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

pub struct TokenCounter {
    prompt_tokens: AtomicUsize,
    cached_prompt_tokens: AtomicUsize,
    completion_tokens: AtomicUsize,
    cost_usd: Mutex<f64>,
    /// Prompt size of the latest request, i.e. how much of the context window the conversation uses.
    last_prompt_tokens: AtomicUsize,
    /// Size of the model's context window in tokens. 0 while it is unknown.
    context_window: AtomicUsize,
}

impl TokenCounter {
    pub fn new() -> Self {
        TokenCounter {
            prompt_tokens: AtomicUsize::new(0),
            cached_prompt_tokens: AtomicUsize::new(0),
            completion_tokens: AtomicUsize::new(0),
            cost_usd: Mutex::new(0.0),
            last_prompt_tokens: AtomicUsize::new(0),
            context_window: AtomicUsize::new(0),
        }
    }

    pub fn increment_prompt(&self, count: usize) {
        self.prompt_tokens.fetch_add(count, Ordering::SeqCst);
    }

    pub fn increment_cached_prompt(&self, count: usize) {
        self.cached_prompt_tokens.fetch_add(count, Ordering::SeqCst);
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

    /// Records the prompt size reported for the latest request (replaces the previous value).
    pub fn set_last_prompt_tokens(&self, count: usize) {
        self.last_prompt_tokens.store(count, Ordering::SeqCst);
    }

    pub fn set_context_window(&self, tokens: usize) {
        self.context_window.store(tokens, Ordering::SeqCst);
    }

    /// Context window size in tokens, or None while it is unknown.
    pub fn context_window(&self) -> Option<usize> {
        match self.context_window.load(Ordering::SeqCst) {
            0 => None,
            tokens => Some(tokens),
        }
    }

    /// One line for the TUI status bar and the end of headless runs.
    pub fn format_usage_summary(&self) -> String {
        let prompt_tokens = self.current_prompt();
        let cached_percent = if prompt_tokens == 0 {
            0
        } else {
            self.cached_prompt_tokens.load(Ordering::SeqCst) * 100 / prompt_tokens
        };
        let mut summary = format!(
            "Sent: {} ({}% cached) | Received: {} | Cost: ${:.4}",
            format_tokens(prompt_tokens),
            cached_percent,
            format_tokens(self.current_completion()),
            self.current_cost_usd()
        );

        let last_prompt = self.last_prompt_tokens.load(Ordering::SeqCst);
        match self.context_window() {
            Some(window) => summary.push_str(&format!(
                " | Context: {} / {} tokens ({:.1}%)",
                format_tokens(last_prompt),
                format_tokens(window),
                last_prompt as f64 * 100.0 / window as f64
            )),
            None if last_prompt > 0 => {
                summary.push_str(&format!(" | Context: {} tokens", format_tokens(last_prompt)))
            }
            None => {}
        }
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::TokenCounter;

    #[test]
    fn shows_context_percent_when_window_is_known() {
        let counter = TokenCounter::new();
        counter.increment_prompt(1000);
        counter.set_last_prompt_tokens(250_000);
        counter.set_context_window(1_000_000);
        let summary = counter.format_usage_summary();
        assert!(summary.ends_with("| Context: 250.0k / 1.0m tokens (25.0%)"), "{}", summary);
    }

    #[test]
    fn formats_token_counts_with_k_and_m() {
        use super::format_tokens;
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(512), "512");
        assert_eq!(format_tokens(1_000), "1.0k");
        assert_eq!(format_tokens(15_300), "15.3k");
        assert_eq!(format_tokens(999_949), "999.9k");
        assert_eq!(format_tokens(999_950), "1.0m");
        assert_eq!(format_tokens(1_000_000), "1.0m");
        assert_eq!(format_tokens(2_500_000), "2.5m");
    }

    #[test]
    fn shows_only_tokens_when_window_is_unknown() {
        let counter = TokenCounter::new();
        counter.set_last_prompt_tokens(512);
        let summary = counter.format_usage_summary();
        assert!(summary.ends_with("| Context: 512 tokens"), "{}", summary);
    }

    #[test]
    fn omits_context_before_first_request() {
        let counter = TokenCounter::new();
        assert!(!counter.format_usage_summary().contains("Context"));
    }
}

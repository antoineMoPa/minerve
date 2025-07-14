use std::sync::atomic::{AtomicUsize, Ordering};

pub struct TokenCounter {
    sent_tokens: AtomicUsize,
    received_tokens: AtomicUsize,
}

impl TokenCounter {
    pub fn new() -> Self {
        TokenCounter {
            sent_tokens: AtomicUsize::new(0),
            received_tokens: AtomicUsize::new(0),
        }
    }

    pub fn increment_sent(&self, count: usize) {
        self.sent_tokens.fetch_add(count, Ordering::SeqCst);
    }

    pub fn increment_received(&self, count: usize) {
        self.received_tokens.fetch_add(count, Ordering::SeqCst);
    }

    pub fn current_sent(&self) -> usize {
        self.sent_tokens.load(Ordering::SeqCst)
    }

    pub fn current_received(&self) -> usize {
        self.received_tokens.load(Ordering::SeqCst)
    }
}

//! One-time codes that link a chat account. The code is shown in minerve; whoever sends it to
//! the bot in a direct message becomes allowed. The bot never answers anything else from a
//! stranger, so there is nothing to approve and nothing to phish out of the bot.

use std::time::{Duration, Instant};

pub const LINK_TTL: Duration = Duration::from_secs(5 * 60);

/// No 0/O or 1/I, so a code read off the screen is typed right. 32 symbols: a byte maps evenly.
const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

#[derive(Clone, Debug)]
pub struct PendingLink {
    pub code: String,
    pub expires_at: Instant,
}

impl PendingLink {
    pub fn new(now: Instant) -> Self {
        Self { code: generate_code(), expires_at: now + LINK_TTL }
    }

    /// The code alone, ignoring case, spaces and the dash; never after it expired.
    pub fn matches(&self, text: &str, now: Instant) -> bool {
        now < self.expires_at && normalize(text) == normalize(&self.code)
    }
}

/// 8 symbols (40 bits) as `XXXX-XXXX`, from the OS random generator through a v4 UUID.
fn generate_code() -> String {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let symbols: String = bytes[..8].iter().map(|byte| ALPHABET[(*byte as usize) % ALPHABET.len()] as char).collect();
    format!("{}-{}", &symbols[..4], &symbols[4..])
}

fn normalize(text: &str) -> String {
    text.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_look_right_and_differ() {
        let now = Instant::now();
        let a = PendingLink::new(now).code;
        assert_eq!(a.len(), 9);
        assert_eq!(&a[4..5], "-");
        assert_ne!(a, PendingLink::new(now).code);
    }

    #[test]
    fn matches_only_the_code_before_it_expires() {
        let now = Instant::now();
        let link = PendingLink { code: "ABCD-EF23".into(), expires_at: now + LINK_TTL };
        assert!(link.matches("abcd ef23", now));
        assert!(link.matches(" ABCD-EF23\n", now));
        assert!(!link.matches("please link me ABCD-EF23", now));
        assert!(!link.matches("ABCD-EF24", now));
        assert!(!link.matches("ABCD-EF23", now + LINK_TTL));
    }
}

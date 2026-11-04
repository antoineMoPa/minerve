//! Placeholder tips shown dimmed in the empty message box. Add a tip by appending to `PRO_TIPS`.
//! The tip changes each time the app starts, so a returning user sees a different one.

const PRO_TIPS: &[&str] = &["hold shift to select text here and in message history"];

/// Tip for this launch, picked from `PRO_TIPS` by the current time, with the "Pro tip:" prefix.
pub fn tip_for_launch() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    format!("Pro tip: {}", PRO_TIPS[(seconds as usize) % PRO_TIPS.len()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tip_is_in_the_list() {
        let tip = tip_for_launch();
        assert!(tip.starts_with("Pro tip: "));
        assert!(PRO_TIPS.iter().any(|text| tip == format!("Pro tip: {text}")));
    }
}

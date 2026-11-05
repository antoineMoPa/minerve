//! Shapes agent text for a transport's limits.

use super::types::{Capabilities, Overflow};

/// Splits `text` into pieces of at most `max_len` chars, preferring to cut at a blank line,
/// then a newline, then a space.
pub fn chunk(text: &str, max_len: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut rest = text.trim();
    while rest.chars().count() > max_len {
        let limit = rest.char_indices().nth(max_len).map(|(i, _)| i).unwrap_or(rest.len());
        // One char past the limit, so a break right at the limit counts.
        let window_end = rest.char_indices().nth(max_len + 1).map(|(i, _)| i).unwrap_or(rest.len());
        let window = &rest[..window_end];
        let cut = window
            .rfind("\n\n")
            .or_else(|| window.rfind('\n'))
            .or_else(|| window.rfind(' '))
            .filter(|&i| i > 0)
            .unwrap_or(limit);
        chunks.push(rest[..cut].trim_end().to_string());
        rest = rest[cut..].trim_start();
    }
    if !rest.is_empty() {
        chunks.push(rest.to_string());
    }
    chunks
}

/// Closes a code block a chunk ends inside, and reopens it at the next chunk's start, so each
/// message renders on its own.
pub fn balance_fences(chunks: Vec<String>) -> Vec<String> {
    let mut open = false;
    chunks
        .into_iter()
        .map(|piece| {
            let mut piece = if open { format!("```\n{}", piece) } else { piece };
            open = piece.matches("```").count() % 2 == 1;
            if open {
                piece.push_str("\n```");
            }
            piece
        })
        .collect()
}

const TRUNCATION_MARK: &str = "\n… [cut]";

/// The start of `text` fitting in `max_len` chars with a visible mark, or `text` itself when it fits.
pub fn truncate(text: &str, max_len: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max_len {
        return text.to_string();
    }
    let keep = max_len.saturating_sub(TRUNCATION_MARK.chars().count());
    let end = text.char_indices().nth(keep).map(|(i, _)| i).unwrap_or(text.len());
    format!("{}{}", text[..end].trim_end(), TRUNCATION_MARK)
}

/// What to send for `reply`, and the note to give the agent on its next turn when the reply was cut.
pub fn plan_reply(reply: &str, caps: Capabilities) -> (Vec<String>, Option<String>) {
    let length = reply.trim().chars().count();
    match caps.overflow {
        Overflow::Truncate if length > caps.max_len => {
            let note = format!(
                "your previous reply ({} chars) was cut to {} chars on this chat app; the user saw only the start. Keep replies short.",
                length, caps.max_len
            );
            (vec![truncate(reply, caps.max_len)], Some(note))
        }
        // Room for the fence `balance_fences` may add at each end.
        _ => (balance_fences(chunk(reply, caps.max_len.saturating_sub(8).max(1))), None),
    }
}

/// Standing instruction for a new session on a transport that cuts long replies.
pub fn length_instruction(caps: Capabilities) -> Option<String> {
    (caps.overflow == Overflow::Truncate).then(|| {
        format!(
            "You are talking through a chat app that cuts replies longer than {} characters. Keep every reply under that, and put the answer first.",
            caps.max_len
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_one_chunk() {
        assert_eq!(chunk("hello", 10), vec!["hello"]);
    }

    #[test]
    fn cuts_at_paragraphs_then_spaces() {
        assert_eq!(chunk("aaa bbb\n\nccc ddd", 9), vec!["aaa bbb", "ccc ddd"]);
        assert_eq!(chunk("aaaa bbbb cccc", 9), vec!["aaaa bbbb", "cccc"]);
    }

    #[test]
    fn hard_cuts_when_no_break_and_never_splits_a_char() {
        assert_eq!(chunk("ééééé", 2), vec!["éé", "éé", "é"]);
    }

    fn caps(overflow: Overflow) -> Capabilities {
        Capabilities { max_len: 20, can_edit: false, overflow }
    }

    #[test]
    fn truncate_fits_the_limit_with_a_mark() {
        let cut = truncate(&"a".repeat(100), 20);
        assert_eq!(cut.chars().count(), 20);
        assert!(cut.ends_with("[cut]"));
        assert_eq!(truncate("short", 20), "short");
    }

    #[test]
    fn truncating_transports_cut_and_leave_a_note_for_the_agent() {
        let (pieces, note) = plan_reply(&"b".repeat(100), caps(Overflow::Truncate));
        assert_eq!(pieces.len(), 1);
        assert!(note.unwrap().contains("100 chars"));
        assert_eq!(plan_reply("fits", caps(Overflow::Truncate)), (vec!["fits".to_string()], None));
    }

    #[test]
    fn a_code_block_split_across_chunks_stays_a_code_block() {
        let chunks = balance_fences(vec!["text\n```\nline 1".into(), "line 2\n```\nafter".into()]);
        assert_eq!(chunks, vec!["text\n```\nline 1\n```", "```\nline 2\n```\nafter"]);
    }

    #[test]
    fn chunking_transports_never_leave_a_note() {
        let (pieces, note) = plan_reply(&"c ".repeat(40), caps(Overflow::Chunk));
        assert!(pieces.len() > 1 && note.is_none());
    }
}

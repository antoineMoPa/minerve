//! A conversation that outgrew the model's context window: every request failed the same way,
//! and each new prompt only made the history longer. On that error the history is compacted
//! and the request sent again.
//!
//! History is otherwise append-only, to keep the prompt cache (see `Minerve::add_prompt`); a
//! compaction costs one uncached request, which beats a session that can never answer again.

use crate::chat::{ChatCompletionMessage, ChatCompletionMessageRole};

/// Tool results shorter than this are kept: dropping them saves little.
const MIN_DROPPED_OUTPUT_CHARS: usize = 200;

const DROPPED_OUTPUT_NOTE: &str = "[Tool output dropped to fit the context window]";
const DROPPED_MESSAGES_NOTE_END: &str = "were dropped to fit the context window]";

fn is_dropped_messages_note(message: &ChatCompletionMessage) -> bool {
    is_prompt(message)
        && message
            .content
            .as_ref()
            .is_some_and(|text| text.starts_with('[') && text.ends_with(DROPPED_MESSAGES_NOTE_END))
}

/// Whether an API error says the request is longer than the model's context window.
/// Anthropic: "prompt is too long: 210000 tokens > 200000 maximum".
/// OpenAI: "context_length_exceeded". OpenRouter: "maximum context length is 128000 tokens".
pub fn is_context_overflow_error(error: &str) -> bool {
    let error = error.to_lowercase();
    error.contains("prompt is too long")
        || error.contains("context_length_exceeded")
        || error.contains("maximum context length")
        || error.contains("context window")
        || error.contains("input is too long")
}

/// Shrinks the history, mildest step first, and says what was done; `None` when nothing is
/// left to drop. The latest prompt and everything after it (the turn running) stay whole.
/// 1. Tool outputs and images before the latest prompt are replaced by a note.
/// 2. Then the older half of the conversation is dropped, cut where a prompt starts so every
///    tool call keeps its result, and a note says how many messages went.
pub fn compact_history(messages: &mut Vec<ChatCompletionMessage>) -> Option<String> {
    let current_turn_start = find_latest_prompt(messages)?;
    let dropped_outputs = drop_old_tool_outputs(&mut messages[..current_turn_start]);
    if dropped_outputs > 0 {
        return Some(format!(
            "The conversation outgrew the model's context window: {} old tool outputs were dropped.",
            dropped_outputs
        ));
    }
    let dropped_messages = drop_older_half(messages, current_turn_start)?;
    Some(format!(
        "The conversation outgrew the model's context window: its {} oldest messages were dropped.",
        dropped_messages
    ))
}

fn is_prompt(message: &ChatCompletionMessage) -> bool {
    matches!(message.role, ChatCompletionMessageRole::User)
}

fn find_latest_prompt(messages: &[ChatCompletionMessage]) -> Option<usize> {
    messages.iter().rposition(is_prompt)
}

fn drop_old_tool_outputs(messages: &mut [ChatCompletionMessage]) -> usize {
    let mut dropped = 0;
    for message in messages {
        let is_long_tool_output = matches!(message.role, ChatCompletionMessageRole::Tool)
            && message
                .content
                .as_ref()
                .is_some_and(|text| text.len() >= MIN_DROPPED_OUTPUT_CHARS);
        if is_long_tool_output || !message.images.is_empty() {
            if is_long_tool_output {
                message.content = Some(String::from(DROPPED_OUTPUT_NOTE));
            }
            message.images.clear();
            dropped += 1;
        }
    }
    dropped
}

/// Drops the older half of the messages between the system prompts and the latest prompt,
/// cut at a prompt. Returns how many went, or `None` when no earlier prompt is left to cut at.
fn drop_older_half(
    messages: &mut Vec<ChatCompletionMessage>,
    current_turn_start: usize,
) -> Option<usize> {
    let first = messages
        .iter()
        .position(|m| !matches!(m.role, ChatCompletionMessageRole::System))?;
    if first >= current_turn_start {
        return None;
    }
    let middle = first + (current_turn_start - first) / 2;
    // The first prompt at or after the middle: the conversation goes on from there.
    let cut = (middle.max(first + 1)..=current_turn_start).find(|&i| is_prompt(&messages[i]))?;
    let dropped = cut - first;
    // Only an earlier note would go: nothing is left to drop.
    if messages[first..cut].iter().all(is_dropped_messages_note) {
        return None;
    }
    messages.splice(
        first..cut,
        [ChatCompletionMessage::user(format!(
            "[{} earlier messages {}",
            dropped, DROPPED_MESSAGES_NOTE_END
        ))],
    );
    Some(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{ChatCompletionFunctionCall, ChatCompletionToolCall};

    fn build_exchange(id: &str, output: &str) -> Vec<ChatCompletionMessage> {
        let mut call = ChatCompletionMessage::assistant(String::new());
        call.tool_calls = Some(vec![ChatCompletionToolCall {
            id: id.to_string(),
            call_type: String::from("function"),
            function: ChatCompletionFunctionCall {
                name: String::from("read"),
                arguments: String::from("{}"),
            },
        }]);
        vec![
            ChatCompletionMessage::user(format!("prompt {id}")),
            call,
            ChatCompletionMessage::tool_result(id.to_string(), output.to_string()),
            ChatCompletionMessage::assistant(format!("answer {id}")),
        ]
    }

    fn build_history(outputs: &[&str]) -> Vec<ChatCompletionMessage> {
        let mut history = vec![ChatCompletionMessage::system(String::from("system"))];
        for (i, output) in outputs.iter().enumerate() {
            history.extend(build_exchange(&i.to_string(), output));
        }
        history.push(ChatCompletionMessage::user(String::from("latest")));
        history
    }

    #[test]
    fn recognizes_the_providers_overflow_errors() {
        assert!(is_context_overflow_error(
            r#"400: {"type":"error","error":{"message":"prompt is too long: 210000 tokens > 200000 maximum"}}"#
        ));
        assert!(is_context_overflow_error(
            "This endpoint's maximum context length is 128000 tokens"
        ));
        assert!(!is_context_overflow_error("429 Too Many Requests"));
    }

    #[test]
    fn old_tool_outputs_go_first_and_the_running_turn_stays() {
        let long = "x".repeat(500);
        let mut history = build_history(&[&long, "short"]);
        let before = history.len();
        let notice = compact_history(&mut history).unwrap();
        assert!(notice.contains("1 old tool outputs"), "{notice}");
        assert_eq!(history.len(), before);
        assert_eq!(history[3].content.as_deref(), Some(DROPPED_OUTPUT_NOTE));
        assert_eq!(history[7].content.as_deref(), Some("short"));
        assert_eq!(history.last().unwrap().content.as_deref(), Some("latest"));
    }

    #[test]
    fn then_the_older_half_goes_cut_at_a_prompt_so_calls_keep_their_results() {
        let mut history = build_history(&["a", "b", "c", "d"]);
        let notice = compact_history(&mut history).unwrap();
        assert!(notice.contains("8 oldest messages"), "{notice}");
        assert!(matches!(history[0].role, ChatCompletionMessageRole::System));
        assert!(history[1]
            .content
            .as_deref()
            .unwrap()
            .contains("8 earlier messages"));
        assert_eq!(history[2].content.as_deref(), Some("prompt 2"));
        assert_eq!(history.last().unwrap().content.as_deref(), Some("latest"));

        // Nothing is left to drop once only the running turn remains.
        while compact_history(&mut history).is_some() {}
        assert_eq!(history.last().unwrap().content.as_deref(), Some("latest"));
    }
}

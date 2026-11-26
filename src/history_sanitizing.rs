//! Messages a provider could reject on every request, made valid for the request only.
//!
//! History is append-only and saved, so one message a provider refuses (written by another
//! provider before a `/model` switch, or by a model answering oddly) would fail every later
//! request the same way. The saved history is left as it is; each request carries a repaired copy.

use crate::chat::{ChatCompletionMessage, ChatCompletionMessageRole};

/// Text of an assistant message that had neither text nor tool calls. OpenAI rejects such a
/// message and Anthropic rejects an empty text block.
const EMPTY_ANSWER_NOTE: &str = "[empty answer]";

/// The history with every message a provider could reject repaired:
/// - an empty `tool_calls` list is dropped (OpenAI rejects `[]`);
/// - tool call arguments that are not a JSON object become `{}` (Anthropic needs an object);
/// - tool call ids keep only the characters Anthropic accepts, on both the call and its result;
/// - an assistant message with neither text nor tool calls gets a note as its text;
/// - a tool result without a tool call id is dropped (no provider can pair it).
pub fn sanitize_history_for_request(
    history: Vec<ChatCompletionMessage>,
) -> Vec<ChatCompletionMessage> {
    history
        .into_iter()
        .filter_map(|mut message| match message.role {
            ChatCompletionMessageRole::Assistant => {
                sanitize_assistant_message(&mut message);
                Some(message)
            }
            ChatCompletionMessageRole::Tool => {
                let id = message.tool_call_id.take()?;
                message.tool_call_id = Some(sanitize_tool_call_id(&id));
                Some(message)
            }
            _ => Some(message),
        })
        .collect()
}

fn sanitize_assistant_message(message: &mut ChatCompletionMessage) {
    if message
        .tool_calls
        .as_ref()
        .is_some_and(|calls| calls.is_empty())
    {
        message.tool_calls = None;
    }
    for call in message.tool_calls.iter_mut().flatten() {
        call.id = sanitize_tool_call_id(&call.id);
        if !is_json_object_or_empty(&call.function.arguments) {
            call.function.arguments = String::from("{}");
        }
    }
    let has_text = message
        .content
        .as_ref()
        .is_some_and(|text| !text.is_empty());
    if !has_text && message.tool_calls.is_none() {
        message.content = Some(String::from(EMPTY_ANSWER_NOTE));
    }
}

fn is_json_object_or_empty(arguments: &str) -> bool {
    arguments.trim().is_empty()
        || matches!(
            serde_json::from_str::<serde_json::Value>(arguments),
            Ok(serde_json::Value::Object(_))
        )
}

/// Anthropic accepts tool call ids matching `^[a-zA-Z0-9_-]+$`; other providers write ids such
/// as `functions.bash:0`. The same id always maps to the same result, so calls and results pair.
fn sanitize_tool_call_id(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        String::from("call")
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{ChatCompletionFunctionCall, ChatCompletionToolCall};

    fn build_assistant_with_calls(calls: Vec<(&str, &str)>) -> ChatCompletionMessage {
        let mut message = ChatCompletionMessage::assistant(String::new());
        message.content = None;
        message.tool_calls = Some(
            calls
                .into_iter()
                .map(|(id, arguments)| ChatCompletionToolCall {
                    id: id.to_string(),
                    call_type: String::from("function"),
                    function: ChatCompletionFunctionCall {
                        name: String::from("run_shell_command"),
                        arguments: arguments.to_string(),
                    },
                })
                .collect(),
        );
        message
    }

    #[test]
    fn a_call_from_another_provider_is_made_valid_and_stays_paired_with_its_result() {
        let history = vec![
            build_assistant_with_calls(vec![("functions.bash:0", r#"{"command":"ls"#)]),
            ChatCompletionMessage::tool_result(String::from("functions.bash:0"), String::from("a")),
        ];
        let out = sanitize_history_for_request(history);
        let call = &out[0].tool_calls.as_ref().unwrap()[0];
        assert_eq!(call.id, "functions_bash_0");
        assert_eq!(call.function.arguments, "{}");
        assert_eq!(out[1].tool_call_id.as_deref(), Some("functions_bash_0"));
    }

    #[test]
    fn valid_calls_are_left_alone() {
        let history = vec![build_assistant_with_calls(vec![(
            "toolu_01-A",
            r#"{"command":"ls"}"#,
        )])];
        let out = sanitize_history_for_request(history);
        let call = &out[0].tool_calls.as_ref().unwrap()[0];
        assert_eq!(call.id, "toolu_01-A");
        assert_eq!(call.function.arguments, r#"{"command":"ls"}"#);
        assert_eq!(out[0].content, None);
    }

    #[test]
    fn an_empty_answer_gets_a_note_and_an_empty_call_list_is_dropped() {
        let out = sanitize_history_for_request(vec![build_assistant_with_calls(vec![])]);
        assert!(out[0].tool_calls.is_none());
        assert_eq!(out[0].content.as_deref(), Some(EMPTY_ANSWER_NOTE));
    }

    #[test]
    fn a_tool_result_without_an_id_is_dropped() {
        let mut orphan = ChatCompletionMessage::tool_result(String::new(), String::from("x"));
        orphan.tool_call_id = None;
        let out = sanitize_history_for_request(vec![
            ChatCompletionMessage::user(String::from("hi")),
            orphan,
        ]);
        assert_eq!(out.len(), 1);
    }
}

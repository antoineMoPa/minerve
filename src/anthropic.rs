//! Anthropic (Claude) Messages API. Minerve keeps its history in the OpenAI chat completions
//! shape; this module translates a request to the Messages shape and the answer back.
//!
//! Differences handled here:
//! - system messages go to the top-level `system` field,
//! - tool calls are `tool_use` blocks, tool results are `tool_result` blocks inside a user message,
//! - consecutive tool results are merged into one user message, as the API requires,
//! - tool definitions use `input_schema` instead of `parameters`,
//! - `max_tokens` is required.

use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::chat::{
    ChatCompletionChoice, ChatCompletionMessage, ChatCompletionMessageRole,
    ChatCompletionRequest, ChatCompletionResponse, ChatCompletionToolCall,
    ChatCompletionToolDefinition, ChatCompletionFunctionCall, PromptTokensDetails,
    RequestContent, Usage,
};

pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Output limit sent with every request. 8192 is accepted by every Claude model.
const MAX_OUTPUT_TOKENS: u64 = 8192;

fn cache_control_json() -> Value {
    json!({ "type": "ephemeral" })
}

/// Text of a message and whether it carries a cache breakpoint.
fn text_and_breakpoint(content: &Option<RequestContent>) -> Option<(&str, bool)> {
    match content {
        None => None,
        Some(RequestContent::Text(text)) => Some((text.as_str(), false)),
        Some(RequestContent::TextWithCacheBreakpoint([part])) => Some((part.text.as_str(), true)),
    }
}

fn text_block(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

/// Adds a message with `role`, or appends `blocks` to the previous message when it has the same role.
fn push_blocks(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut() {
        if last["role"] == role {
            last["content"]
                .as_array_mut()
                .expect("message content is an array")
                .extend(blocks);
            return;
        }
    }
    messages.push(json!({ "role": role, "content": blocks }));
}

fn parse_tool_arguments(tool_call: &ChatCompletionToolCall) -> Result<Value, String> {
    let arguments = tool_call.function.arguments.trim();
    if arguments.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(arguments).map_err(|err| {
        format!(
            "arguments of tool call {} are not valid JSON: {}",
            tool_call.function.name, err
        )
    })
}

/// Translates a request in the chat completions shape to a Messages API body.
/// Fails when a tool call of the history has arguments that are not JSON.
pub fn to_messages_body(request: &ChatCompletionRequest) -> Result<Value, String> {
    let mut system_blocks: Vec<Value> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();

    for message in &request.messages {
        let text = text_and_breakpoint(&message.content);
        let mut blocks: Vec<Value> = Vec::new();
        let mut role = "user";
        match message.role {
            ChatCompletionMessageRole::System => {
                if let Some((text, cache)) = text {
                    let mut block = text_block(text);
                    if cache {
                        block["cache_control"] = cache_control_json();
                    }
                    system_blocks.push(block);
                }
                continue;
            }
            ChatCompletionMessageRole::User => {
                if let Some((text, _)) = text {
                    blocks.push(text_block(text));
                }
            }
            ChatCompletionMessageRole::Assistant => {
                role = "assistant";
                if let Some((text, _)) = text {
                    if !text.is_empty() {
                        blocks.push(text_block(text));
                    }
                }
                for tool_call in message.tool_calls.iter().flatten() {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": tool_call.id,
                        "name": tool_call.function.name,
                        "input": parse_tool_arguments(tool_call)?,
                    }));
                }
            }
            ChatCompletionMessageRole::Tool => {
                let tool_call_id = message
                    .tool_call_id
                    .as_ref()
                    .expect("tool message without tool_call_id");
                let content = text.map(|(text, _)| text).unwrap_or("");
                blocks.push(json!({
                    "type": "tool_result",
                    "tool_use_id": tool_call_id,
                    "content": content,
                }));
            }
        }

        // A cache breakpoint goes on the last block of the message.
        if let Some((_, true)) = text {
            if let Some(last) = blocks.last_mut() {
                last["cache_control"] = cache_control_json();
            }
        }
        push_blocks(&mut messages, role, blocks);
    }

    let mut body = json!({
        "model": request.model,
        "max_tokens": MAX_OUTPUT_TOKENS,
        "messages": messages,
    });
    if !system_blocks.is_empty() {
        body["system"] = Value::Array(system_blocks);
    }
    if let Some(tools) = &request.tools {
        body["tools"] = Value::Array(tools.iter().map(tool_json).collect());
    }
    Ok(body)
}

fn tool_json(tool: &ChatCompletionToolDefinition) -> Value {
    let mut json_tool = json!({
        "name": tool.function.name,
        "input_schema": tool
            .function
            .parameters
            .clone()
            .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
    });
    if let Some(description) = &tool.function.description {
        json_tool["description"] = json!(description);
    }
    if tool.cache_control.is_some() {
        json_tool["cache_control"] = cache_control_json();
    }
    json_tool
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<Value>,
    usage: MessagesUsage,
}

#[derive(Deserialize)]
struct MessagesUsage {
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

/// Translates a Messages API answer to the chat completions shape.
/// Anthropic does not report a cost, so `Usage::cost` is None.
pub fn parse_messages_response(body: &str) -> Result<ChatCompletionResponse, String> {
    let response: MessagesResponse = serde_json::from_str(body)
        .map_err(|err| format!("Unexpected response ({}): {}", err, body))?;

    let mut text_parts: Vec<&str> = Vec::new();
    let mut tool_calls: Vec<ChatCompletionToolCall> = Vec::new();
    for block in &response.content {
        match block["type"].as_str() {
            Some("text") => text_parts.push(block["text"].as_str().unwrap_or_default()),
            Some("tool_use") => tool_calls.push(ChatCompletionToolCall {
                id: block["id"].as_str().unwrap_or_default().to_string(),
                call_type: String::from("function"),
                function: ChatCompletionFunctionCall {
                    name: block["name"].as_str().unwrap_or_default().to_string(),
                    arguments: block["input"].to_string(),
                },
            }),
            // Other block types (such as thinking) are not shown or sent back.
            _ => {}
        }
    }

    let usage = response.usage;
    let message = ChatCompletionMessage {
        role: ChatCompletionMessageRole::Assistant,
        content: (!text_parts.is_empty()).then(|| text_parts.concat()),
        tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
        tool_call_id: None,
    };
    Ok(ChatCompletionResponse {
        choices: vec![ChatCompletionChoice { message }],
        usage: Some(Usage {
            // Prompt tokens of the chat completions shape include the cached ones.
            prompt_tokens: usage.input_tokens
                + usage.cache_creation_input_tokens
                + usage.cache_read_input_tokens,
            completion_tokens: usage.output_tokens,
            cost: None,
            prompt_tokens_details: Some(PromptTokensDetails {
                cached_tokens: usage.cache_read_input_tokens,
            }),
        }),
    })
}

/// Posts a request to the Messages API at `url` and returns the answer in the chat completions shape.
pub async fn post_messages(
    client: &Client,
    url: &str,
    api_key: &str,
    request: &ChatCompletionRequest,
) -> Result<ChatCompletionResponse, String> {
    let body = to_messages_body(request)?;
    let response = client
        .post(url)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|err| err.to_string())?;

    let status = response.status();
    let text = response.text().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("{}: {}", status, text));
    }
    parse_messages_response(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::RequestMessage;

    fn request_with(messages: Vec<RequestMessage>) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: String::from("claude-3-5-haiku-latest"),
            messages,
            tools: None,
            usage: None,
        }
    }

    fn from(message: ChatCompletionMessage) -> RequestMessage {
        RequestMessage::from_message(&message)
    }

    #[test]
    fn system_goes_to_top_level_and_tool_results_are_merged() {
        let call = ChatCompletionToolCall {
            id: String::from("toolu_1"),
            call_type: String::from("function"),
            function: ChatCompletionFunctionCall {
                name: String::from("read"),
                arguments: String::from("{\"path\":\"a.txt\"}"),
            },
        };
        let assistant = ChatCompletionMessage {
            role: ChatCompletionMessageRole::Assistant,
            content: None,
            tool_calls: Some(vec![call.clone(), call]),
            tool_call_id: None,
        };
        let request = request_with(vec![
            from(ChatCompletionMessage::system(String::from("be brief"))),
            from(ChatCompletionMessage::user(String::from("hi"))),
            from(assistant),
            from(ChatCompletionMessage::tool_result(String::from("toolu_1"), String::from("one"))),
            from(ChatCompletionMessage::tool_result(String::from("toolu_1"), String::from("two"))),
        ]);

        let body = to_messages_body(&request).unwrap();
        assert_eq!(body["system"][0]["text"], "be brief");
        assert_eq!(body["max_tokens"], MAX_OUTPUT_TOKENS);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1]["content"][0]["type"], "tool_use");
        assert_eq!(messages[1]["content"][0]["input"]["path"], "a.txt");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"].as_array().unwrap().len(), 2);
        assert_eq!(messages[2]["content"][1]["content"], "two");
    }

    #[test]
    fn breakpoint_is_kept_on_last_block() {
        let mut first = RequestMessage::from_message(&ChatCompletionMessage::user(String::from("hi")));
        first.add_cache_breakpoint();
        let body = to_messages_body(&request_with(vec![first])).unwrap();
        assert_eq!(body["messages"][0]["content"][0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn text_and_tool_use_answer_is_parsed() {
        let body = r#"{
            "content": [
                {"type": "text", "text": "Let me look."},
                {"type": "tool_use", "id": "toolu_9", "name": "read", "input": {"path": "b.txt"}}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5,
                      "cache_creation_input_tokens": 2, "cache_read_input_tokens": 100}
        }"#;
        let response = parse_messages_response(body).unwrap();
        let message = &response.choices[0].message;
        assert_eq!(message.content.as_deref(), Some("Let me look."));
        let tool_calls = message.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls[0].id, "toolu_9");
        assert_eq!(tool_calls[0].function.arguments, "{\"path\":\"b.txt\"}");
        let usage = response.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 112);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.prompt_tokens_details.unwrap().cached_tokens, 100);
        assert!(usage.cost.is_none());
    }
}

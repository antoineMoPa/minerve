use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatCompletionMessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionMessage {
    pub role: ChatCompletionMessageRole,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ChatCompletionToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatCompletionMessage {
    pub fn system(content: String) -> Self {
        Self {
            role: ChatCompletionMessageRole::System,
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: String) -> Self {
        Self {
            role: ChatCompletionMessageRole::User,
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant(content: String) -> Self {
        Self {
            role: ChatCompletionMessageRole::Assistant,
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: String, content: String) -> Self {
        Self {
            role: ChatCompletionMessageRole::Tool,
            content: Some(content),
            tool_calls: None,
            tool_call_id: Some(tool_call_id),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ChatCompletionFunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionFunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Marks a cache breakpoint: providers that need breakpoints (Anthropic) cache the prompt
/// up to and including the tool or message part carrying it.
#[derive(Debug, Clone, Serialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub cache_type: &'static str,
}

pub const EPHEMERAL_CACHE_CONTROL: CacheControl = CacheControl {
    cache_type: "ephemeral",
};

#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: &'static str,
    pub function: ChatCompletionFunctionDefinition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionFunctionDefinition {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<serde_json::Value>,
}

/// OpenRouter usage accounting: with include set, the response usage carries the cost in USD.
#[derive(Debug, Serialize)]
pub struct UsageAccountingRequest {
    pub include: bool,
}

/// A message as sent in a request. Its content becomes a one-part array when it carries a
/// cache breakpoint, the only shape that can hold cache_control.
#[derive(Debug, Serialize)]
pub struct RequestMessage {
    pub role: ChatCompletionMessageRole,
    pub content: Option<RequestContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ChatCompletionToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum RequestContent {
    Text(String),
    TextWithCacheBreakpoint([RequestTextPart; 1]),
}

#[derive(Debug, Serialize)]
pub struct RequestTextPart {
    #[serde(rename = "type")]
    pub part_type: &'static str,
    pub text: String,
    pub cache_control: CacheControl,
}

impl RequestMessage {
    pub fn from_message(message: &ChatCompletionMessage) -> Self {
        Self {
            role: message.role.clone(),
            content: message.content.clone().map(RequestContent::Text),
            tool_calls: message.tool_calls.clone(),
            tool_call_id: message.tool_call_id.clone(),
        }
    }

    pub fn add_cache_breakpoint(&mut self) {
        let text = match self.content.take() {
            Some(RequestContent::Text(text)) => text,
            other => panic!(
                "cache breakpoint on a message without text content: {:?}",
                other
            ),
        };
        self.content = Some(RequestContent::TextWithCacheBreakpoint([RequestTextPart {
            part_type: "text",
            text,
            cache_control: EPHEMERAL_CACHE_CONTROL,
        }]));
    }
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<RequestMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ChatCompletionToolDefinition>>,
    /// OpenRouter only; OpenAI and Anthropic reject the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageAccountingRequest>,
}

#[derive(Debug, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// USD charged by OpenRouter for this request. None when the provider reports no cost.
    #[serde(default)]
    pub cost: Option<f64>,
    /// Not every provider reports it; a request without it counts as having no cached tokens.
    pub prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Debug, Deserialize)]
pub struct PromptTokensDetails {
    pub cached_tokens: u64,
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionResponse {
    pub choices: Vec<ChatCompletionChoice>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionChoice {
    pub message: ChatCompletionMessage,
}

pub enum ToolCallResult {
    Success(ChatCompletionMessage),
    Error(String),
}

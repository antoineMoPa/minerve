use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize, Serializer};

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
    /// Set on user-role messages that minerve wrote rather than the user, so the chat labels
    /// them by who wrote them. Kept in session files; never sent to the API (`RequestMessage` has no such field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_author: Option<PromptAuthor>,
    /// Image files shown to the model: pasted into a user prompt, or opened by a tool (`view_image`).
    /// Only paths are kept; each request reads and encodes the files again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptAuthor {
    /// `/goal`: the evaluator's reason the goal is not reached yet.
    GoalEvaluator,
}

impl ChatCompletionMessage {
    pub fn system(content: String) -> Self {
        Self {
            role: ChatCompletionMessageRole::System,
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
            prompt_author: None,
            images: Vec::new(),
        }
    }

    pub fn user(content: String) -> Self {
        Self {
            role: ChatCompletionMessageRole::User,
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
            prompt_author: None,
            images: Vec::new(),
        }
    }

    pub fn assistant(content: String) -> Self {
        Self {
            role: ChatCompletionMessageRole::Assistant,
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
            prompt_author: None,
            images: Vec::new(),
        }
    }

    /// A prompt typed by the user, with the images pasted into it.
    pub fn user_with_images(content: String, images: Vec<PathBuf>) -> Self {
        Self {
            images,
            ..Self::user(content)
        }
    }

    pub fn tool_result(tool_call_id: String, content: String) -> Self {
        Self {
            role: ChatCompletionMessageRole::Tool,
            content: Some(content),
            tool_calls: None,
            tool_call_id: Some(tool_call_id),
            prompt_author: None,
            images: Vec::new(),
        }
    }

    /// A user-role message written by minerve: sent to the model as the user, labeled `author` in the chat.
    pub fn authored_prompt(author: PromptAuthor, content: String) -> Self {
        Self {
            prompt_author: Some(author),
            ..Self::user(content)
        }
    }
}

/// Providers reject a tool call that has no tool result right after it. A session can hold such
/// a call when a turn was cut short, so each missing result gets a placeholder here, sent with
/// the request only. The saved history is left as it is.
pub fn with_placeholder_results_for_unanswered_tool_calls(
    history: &[ChatCompletionMessage],
) -> Vec<ChatCompletionMessage> {
    let mut out = Vec::with_capacity(history.len());
    let mut index = 0;
    while index < history.len() {
        let message = &history[index];
        index += 1;
        out.push(message.clone());
        let Some(tool_calls) = &message.tool_calls else {
            continue;
        };
        let mut answered = Vec::new();
        while index < history.len() && matches!(history[index].role, ChatCompletionMessageRole::Tool) {
            if let Some(id) = &history[index].tool_call_id {
                answered.push(id.clone());
            }
            out.push(history[index].clone());
            index += 1;
        }
        for call in tool_calls {
            if !answered.contains(&call.id) {
                out.push(ChatCompletionMessage::tool_result(
                    call.id.clone(),
                    "No result was recorded for this tool call; the turn was interrupted before it finished.".to_string(),
                ));
            }
        }
    }
    out
}

/// OpenAI takes only text in tool messages. Images of tool results are moved, for the request
/// only, to a user message right after the run of tool messages they belong to. Anthropic
/// merges that message with the tool results, so it works there too.
pub fn with_tool_images_in_user_messages(history: Vec<ChatCompletionMessage>) -> Vec<ChatCompletionMessage> {
    let mut out = Vec::with_capacity(history.len());
    let mut pending_images: Vec<PathBuf> = Vec::new();
    for mut message in history {
        let is_tool = matches!(message.role, ChatCompletionMessageRole::Tool);
        if !is_tool && !pending_images.is_empty() {
            out.push(tool_images_message(std::mem::take(&mut pending_images)));
        }
        if is_tool {
            pending_images.append(&mut message.images);
        }
        out.push(message);
    }
    if !pending_images.is_empty() {
        out.push(tool_images_message(pending_images));
    }
    out
}

fn tool_images_message(images: Vec<PathBuf>) -> ChatCompletionMessage {
    let paths: Vec<String> = images.iter().map(|path| path.display().to_string()).collect();
    ChatCompletionMessage::user_with_images(format!("Images opened by the tool calls above: {}", paths.join(", ")), images)
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

/// Media type of an image file, by file extension. Extensions are matched lowercase.
pub const IMAGE_MEDIA_TYPE_BY_EXTENSION: [(&str, &str); 5] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
];

/// Media type of an image file, or None when its extension is not an image one.
pub fn image_media_type(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_lowercase();
    IMAGE_MEDIA_TYPE_BY_EXTENSION
        .iter()
        .find(|(known, _)| *known == extension)
        .map(|(_, media_type)| *media_type)
}

/// A message as sent in a request. Its content becomes an array of parts when it carries
/// images or a cache breakpoint, the only shape that can hold them.
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
    Parts(Vec<RequestPart>),
}

/// A content part in the chat completions shape, which OpenAI and OpenRouter take as is.
/// `anthropic.rs` translates it to Messages API blocks.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RequestPart {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ImageUrl {
        image_url: RequestImage,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

/// An image file's bytes, base64 encoded. Serialized as `{"url": "data:<media type>;base64,<data>"}`.
#[derive(Debug)]
pub struct RequestImage {
    pub media_type: &'static str,
    pub base64_data: String,
}

impl Serialize for RequestImage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry("url", &format!("data:{};base64,{}", self.media_type, self.base64_data))?;
        map.end()
    }
}

impl RequestImage {
    /// Reads and encodes an image file. Fails when the file cannot be read or is not an image.
    pub fn read(path: &Path) -> Result<Self, String> {
        let media_type = image_media_type(path)
            .ok_or_else(|| format!("{} is not an image file (png, jpg, gif or webp)", path.display()))?;
        let bytes = std::fs::read(path).map_err(|err| format!("cannot read image {}: {}", path.display(), err))?;
        Ok(Self {
            media_type,
            base64_data: base64::engine::general_purpose::STANDARD.encode(bytes),
        })
    }
}

impl RequestMessage {
    /// Fails when an image of the message cannot be read.
    /// Only user messages may carry images; see `with_tool_images_in_user_messages` for tool results.
    pub fn from_message(message: &ChatCompletionMessage) -> Result<Self, String> {
        assert!(
            message.images.is_empty() || matches!(message.role, ChatCompletionMessageRole::User),
            "images on a {:?} message",
            message.role
        );
        let content = if message.images.is_empty() {
            message.content.clone().map(RequestContent::Text)
        } else {
            // Images go before the text, as Anthropic recommends.
            let mut parts = Vec::with_capacity(message.images.len() + 1);
            for path in &message.images {
                parts.push(RequestPart::ImageUrl {
                    image_url: RequestImage::read(path)?,
                    cache_control: None,
                });
            }
            if let Some(text) = &message.content {
                parts.push(RequestPart::Text { text: text.clone(), cache_control: None });
            }
            Some(RequestContent::Parts(parts))
        };
        Ok(Self {
            role: message.role.clone(),
            content,
            tool_calls: message.tool_calls.clone(),
            tool_call_id: message.tool_call_id.clone(),
        })
    }

    /// Puts the breakpoint on the last content part.
    pub fn add_cache_breakpoint(&mut self) {
        let mut parts = match self.content.take() {
            Some(RequestContent::Text(text)) => vec![RequestPart::Text { text, cache_control: None }],
            Some(RequestContent::Parts(parts)) => parts,
            None => panic!("cache breakpoint on a message without content"),
        };
        match parts.last_mut().expect("cache breakpoint on a message without content parts") {
            RequestPart::Text { cache_control, .. } | RequestPart::ImageUrl { cache_control, .. } => {
                *cache_control = Some(EPHEMERAL_CACHE_CONTROL);
            }
        }
        self.content = Some(RequestContent::Parts(parts));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_part_is_a_data_url_in_the_chat_completions_shape() {
        let image = std::env::temp_dir().join(format!("minerve-chat-test-{}.jpg", uuid::Uuid::new_v4()));
        std::fs::write(&image, [1u8, 2, 3]).unwrap();
        let mut message = RequestMessage::from_message(&ChatCompletionMessage::user_with_images(
            String::from("what is this?"),
            vec![image.clone()],
        ))
        .unwrap();
        message.add_cache_breakpoint();
        std::fs::remove_file(&image).unwrap();

        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(
            json["content"],
            serde_json::json!([
                {"type": "image_url", "image_url": {"url": "data:image/jpeg;base64,AQID"}},
                {"type": "text", "text": "what is this?", "cache_control": {"type": "ephemeral"}},
            ])
        );
    }

    #[test]
    fn missing_image_fails_the_request_with_its_path() {
        let message = ChatCompletionMessage::user_with_images(String::from("hi"), vec![PathBuf::from("/nope/x.png")]);
        let err = RequestMessage::from_message(&message).unwrap_err();
        assert!(err.contains("/nope/x.png"), "{}", err);
    }

    #[test]
    fn tool_images_follow_the_run_of_tool_results() {
        let mut call = ChatCompletionMessage::assistant(String::new());
        call.tool_calls = Some(vec![]);
        let mut with_image = ChatCompletionMessage::tool_result(String::from("1"), String::from("attached"));
        with_image.images = vec![PathBuf::from("/a.png")];
        let history = vec![
            call,
            with_image,
            ChatCompletionMessage::tool_result(String::from("2"), String::from("text")),
            ChatCompletionMessage::assistant(String::from("done")),
        ];

        let out = with_tool_images_in_user_messages(history);
        let roles: Vec<String> = out.iter().map(|m| format!("{:?}", m.role)).collect();
        assert_eq!(roles, ["Assistant", "Tool", "Tool", "User", "Assistant"]);
        assert!(out[1].images.is_empty());
        assert_eq!(out[3].images, vec![PathBuf::from("/a.png")]);
    }

    #[test]
    fn messages_saved_before_images_still_load() {
        let message: ChatCompletionMessage = serde_json::from_str(r#"{"role":"user","content":"hi"}"#).unwrap();
        assert!(message.images.is_empty());
        assert!(!serde_json::to_string(&message).unwrap().contains("images"));
    }

    #[test]
    fn media_type_comes_from_the_extension() {
        assert_eq!(image_media_type(Path::new("a/B.JPG")), Some("image/jpeg"));
        assert_eq!(image_media_type(Path::new("a.webp")), Some("image/webp"));
        assert_eq!(image_media_type(Path::new("a.txt")), None);
        assert_eq!(image_media_type(Path::new("png")), None);
    }
}

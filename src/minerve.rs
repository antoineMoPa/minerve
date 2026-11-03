use crate::token_counter::TokenCounter;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;

pub async fn post_request_with_token_count(
    client: &Client,
    url: &str,
    api_key: &str,
    request: ChatCompletionRequest,
    token_counter: Arc<TokenCounter>,
) -> Result<ChatCompletionResponse, String> {
    let response = client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .map_err(|err| err.to_string())?;

    let status = response.status();
    let body = response.text().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("{}: {}", status, body));
    }

    let chat_response: ChatCompletionResponse = serde_json::from_str(&body)
        .map_err(|err| format!("Unexpected response ({}): {}", err, body))?;

    if let Some(ref usage) = chat_response.usage {
        // Correctly use the increment with the provided token_counter
        token_counter.increment_prompt(usage.prompt_tokens as usize);
        token_counter.increment_completion(usage.completion_tokens as usize);
        token_counter.increment_cost_usd(usage.cost);
        if let Some(details) = &usage.prompt_tokens_details {
            token_counter.increment_cached_prompt(details.cached_tokens as usize);
        }
    }

    Ok(chat_response)
}

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use crate::openrouter::OpenRouterConnection;
use crate::session::{self, SessionFile};
use crate::tools::registry::get_tool_registry;
use crate::tools::utils::cap_tool_output;
use crate::ui::{send_status, update_chat_ui, UiSender};
use crate::{
    ChatCompletionFunctionDefinition, ChatCompletionMessage, ChatCompletionMessageRole,
    ChatCompletionRequest, ChatCompletionResponse, ChatCompletionToolCall,
    ChatCompletionToolDefinition, RequestMessage, ToolCallResult, UsageAccountingRequest,
    EPHEMERAL_CACHE_CONTROL,
};

pub struct Minerve {
    /// Id of the session file this conversation is saved to.
    pub session_id: String,
    pub messages: Arc<Mutex<Vec<ChatCompletionMessage>>>,
    pub client: Client,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub uses_cache_breakpoints: bool,
    pub request_in_flight: Arc<AtomicBool>,
    pub token_counter: Arc<TokenCounter>,
}

pub fn get_system_prompt() -> String {
    return String::from(include_str!("../prompts/MAIN_SYSTEM_PROMPT.txt"));
}

fn get_tool_definitions(uses_cache_breakpoints: bool) -> Option<Vec<ChatCompletionToolDefinition>> {
    let mut tools: Vec<ChatCompletionToolDefinition> = get_tool_registry()
        .values()
        .map(|tool| ChatCompletionToolDefinition {
            tool_type: "function",
            function: ChatCompletionFunctionDefinition {
                name: tool.name().to_string(),
                description: Some(tool.description().to_string()),
                parameters: Some(tool.function_definition()),
            },
            cache_control: None,
        })
        .collect();
    if uses_cache_breakpoints {
        if let Some(last_tool) = tools.last_mut() {
            last_tool.cache_control = Some(EPHEMERAL_CACHE_CONTROL);
        }
    }
    if tools.is_empty() {
        None
    } else {
        Some(tools)
    }
}

/// Messages shown in the chat view. Tool results are labeled with the function name
/// found in the assistant tool call they answer; assistant messages that only carry
/// tool calls have no text and are left out.
fn get_ui_messages(messages: &[ChatCompletionMessage]) -> Vec<(String, String)> {
    let mut function_name_by_tool_call_id: HashMap<String, String> = HashMap::new();
    let mut ui_messages = Vec::new();
    for msg in messages {
        if let Some(tool_calls) = &msg.tool_calls {
            for tool_call in tool_calls {
                function_name_by_tool_call_id
                    .insert(tool_call.id.clone(), tool_call.function.name.clone());
            }
        }
        let Some(content) = &msg.content else {
            continue;
        };
        let role_label = match msg.role {
            ChatCompletionMessageRole::System => String::from("system"),
            ChatCompletionMessageRole::User => String::from("user"),
            ChatCompletionMessageRole::Assistant => String::from("minerve"),
            ChatCompletionMessageRole::Tool => {
                let tool_call_id = msg
                    .tool_call_id
                    .as_ref()
                    .expect("tool message without tool_call_id");
                function_name_by_tool_call_id
                    .get(tool_call_id)
                    .expect("tool message answering an unknown tool call")
                    .clone()
            }
        };
        ui_messages.push((role_label, content.clone()));
    }
    ui_messages
}

/// Every tool call of an assistant message needs a tool message answering it,
/// otherwise OpenRouter rejects the next request.
/// Returns whether the conversation should continue with another request.
async fn run_tool_calls(
    tool_calls: &[ChatCompletionToolCall],
    ui: Option<UiSender>,
    is_headless: bool,
    on_tool_message: &mut impl FnMut(ChatCompletionMessage),
    on_error: &mut impl FnMut(String),
) -> bool {
    let mut should_continue = true;
    for tool_call in tool_calls {
        if !should_continue {
            on_tool_message(ChatCompletionMessage::tool_result(
                tool_call.id.clone(),
                String::from("Skipped: a previous tool call failed."),
            ));
            continue;
        }
        match handle_tool_call(tool_call, ui.clone(), is_headless).await {
            ToolCallResult::Success(mut msg) => {
                msg.content = msg.content.map(cap_tool_output);
                on_tool_message(msg)
            }
            ToolCallResult::Error(err) => {
                on_tool_message(ChatCompletionMessage::tool_result(
                    tool_call.id.clone(),
                    format!("Error: {}", err),
                ));
                on_error(format!("Error occurred in tool call: {}", err));
                should_continue = false;
            }
        }
    }
    should_continue
}

pub async fn handle_tool_call(
    tool_call: &ChatCompletionToolCall,
    ui: Option<UiSender>,
    is_headless: bool,
) -> ToolCallResult {
    let settings = crate::tools::ExecuteCommandSettings { is_headless };
    let registry = get_tool_registry();
    let tool_name = &tool_call.function.name;
    let args_str = &tool_call.function.arguments;

    if let Some(tool) = registry.get(tool_name.as_str()) {
        // Parse as generic JSON value first, then convert all values to strings
        let args: HashMap<String, String> =
            match serde_json::from_str::<serde_json::Value>(args_str) {
                Ok(serde_json::Value::Object(map)) => map
                    .into_iter()
                    .map(|(k, v)| {
                        let string_value = match v {
                            serde_json::Value::String(s) => s,
                            serde_json::Value::Number(n) => n.to_string(),
                            serde_json::Value::Bool(b) => b.to_string(),
                            _ => v.to_string(),
                        };
                        (k, string_value)
                    })
                    .collect(),
                _ => HashMap::new(),
            };

        if let Some(ui) = &ui {
            send_status(ui, format!("Running tool: {}", tool_name));
        }

        let result = tool.run(args, settings).await;

        if let Some(ui) = &ui {
            send_status(ui, format!("Reading tool result: {}", tool_name));
        }

        ToolCallResult::Success(ChatCompletionMessage::tool_result(
            tool_call.id.clone(),
            result,
        ))
    } else {
        ToolCallResult::Error(format!("Function '{}' not found", tool_name))
    }
}

/// Called when the user sends a new prompt. Tool results of earlier prompts are dropped then,
/// never during a run of tool calls, so every request of a run extends the previous one and
/// hits the prompt cache.
fn clean_tool_outputs_of_previous_prompts(messages: &mut [ChatCompletionMessage]) {
    for msg in messages.iter_mut() {
        if let ChatCompletionMessageRole::Tool = msg.role {
            msg.content = Some(String::from("[cleaned from history]"));
        }
    }
}

impl Minerve {
    fn build_request(&self, history: &[ChatCompletionMessage]) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: self.model.clone(),
            messages: self.build_request_messages(history),
            tools: get_tool_definitions(self.uses_cache_breakpoints),
            usage: UsageAccountingRequest { include: true },
        }
    }

    /// Breakpoints, when the model needs them, go after the system prompt and on the latest
    /// message, like opencode does: the next request reads everything up to there from cache.
    fn build_request_messages(&self, history: &[ChatCompletionMessage]) -> Vec<RequestMessage> {
        let mut messages: Vec<RequestMessage> =
            history.iter().map(RequestMessage::from_message).collect();
        if self.uses_cache_breakpoints {
            messages
                .first_mut()
                .expect("request without messages")
                .add_cache_breakpoint();
            if messages.len() > 1 {
                messages.last_mut().unwrap().add_cache_breakpoint();
            }
        }
        messages
    }

    fn get_completions_url(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    /// Prints assistant replies to stdout and errors to stderr as the conversation goes.
    pub async fn chat_headless(&self) {
        let is_headless = true;

        let mut history: Vec<ChatCompletionMessage> = {
            let msgs = self.messages.lock().unwrap();
            msgs.clone()
        };

        let mut should_continue = true;

        while should_continue {
            should_continue = false;

            let chat_result = post_request_with_token_count(
                &self.client,
                &self.get_completions_url(),
                &self.api_key,
                self.build_request(&history),
                self.token_counter.clone(),
            )
            .await;

            match chat_result {
                Ok(chat_response) => {
                    let assistant_message = chat_response
                        .choices
                        .into_iter()
                        .next()
                        .expect("chat completion response without choices")
                        .message;

                    if let Some(content) = &assistant_message.content {
                        println!("{}", content);
                    }

                    let tool_calls = assistant_message.tool_calls.clone();
                    history.push(assistant_message);

                    if let Some(tool_calls) = tool_calls {
                        for tool_call in &tool_calls {
                            println!("Handling tool call: {}", tool_call.function.name);
                        }
                        should_continue = run_tool_calls(
                            &tool_calls,
                            None,
                            is_headless,
                            &mut |msg| history.push(msg),
                            &mut |err| eprintln!("{}", err),
                        )
                        .await;
                    }
                }
                Err(req_err) => {
                    eprintln!("Request Error: {}", req_err);
                }
            }
        }

        // Keep the replies in the shared conversation, so the session file gets them.
        *self.messages.lock().unwrap() = history;
    }

    /// The chat as shown in the UI, used to show a resumed session.
    pub fn ui_messages(&self) -> Vec<(String, String)> {
        get_ui_messages(&self.messages.lock().unwrap())
    }

    fn add_assistant_message_with_update_ui(
        &self,
        messages: &Arc<Mutex<Vec<ChatCompletionMessage>>>,
        message_content: String,
        ui: &UiSender,
    ) {
        let mut msgs = messages.lock().unwrap();
        msgs.push(ChatCompletionMessage::assistant(message_content));
        update_chat_ui(ui, get_ui_messages(&msgs));
    }

    /// Starts a new session, or continues the saved one when `resume` is given.
    /// `extra_system_prompts` are added after the default system prompt of a new session.
    pub fn new(
        openrouter_connection: OpenRouterConnection,
        resume: Option<SessionFile>,
        extra_system_prompts: &[String],
    ) -> Self {
        let (session_id, messages) = match resume {
            Some(file) => (file.id, file.messages),
            None => {
                let mut messages = vec![ChatCompletionMessage::system(get_system_prompt())];
                messages.extend(
                    extra_system_prompts
                        .iter()
                        .map(|prompt| ChatCompletionMessage::system(prompt.clone())),
                );
                (uuid::Uuid::new_v4().to_string(), messages)
            }
        };
        Self {
            session_id,
            messages: Arc::new(Mutex::new(messages)),
            client: Client::new(),
            api_key: openrouter_connection.api_key,
            base_url: openrouter_connection.base_url,
            model: openrouter_connection.model,
            uses_cache_breakpoints: openrouter_connection.uses_cache_breakpoints,
            request_in_flight: Arc::new(AtomicBool::new(false)),
            token_counter: Arc::new(TokenCounter::new()),
        }
    }

    /// Writes the conversation to its session file. Failures are ignored: a missing save
    /// must not interrupt the chat.
    pub fn save_session(&self) {
        let msgs = self.messages.lock().unwrap();
        let _ = session::save_session(&self.session_id, &msgs);
    }

    pub fn chat_with_arc(self: Arc<Self>, user_input: String, ui: UiSender, is_headless: bool) {
        use std::sync::atomic::Ordering;

        self.request_in_flight.store(true, Ordering::SeqCst);

        {
            let mut msgs = self.messages.lock().unwrap();
            clean_tool_outputs_of_previous_prompts(&mut msgs);
            msgs.push(ChatCompletionMessage::user(user_input));
            update_chat_ui(&ui, get_ui_messages(&msgs));
        }

        send_status(&ui, "working...");

        crate::get_global_runtime().spawn({
            let self_clone = self.clone();
            async move {
                let messages = self_clone.messages.clone();
                let mut should_continue = true;

                while should_continue {
                    should_continue = false;

                    // Show working indicator at start of each loop iteration
                    send_status(&ui, "working...");

                    let request = {
                        let msgs = messages.lock().unwrap();
                        self_clone.build_request(&msgs)
                    };

                    let chat_result = post_request_with_token_count(
                        &self_clone.client,
                        &self_clone.get_completions_url(),
                        &self_clone.api_key,
                        request,
                        self_clone.token_counter.clone(),
                    )
                    .await;

                    match chat_result {
                        Ok(response) => {
                            let assistant_message = response
                                .choices
                                .into_iter()
                                .next()
                                .expect("chat completion response without choices")
                                .message;

                            let tool_calls = assistant_message.tool_calls.clone();
                            messages.lock().unwrap().push(assistant_message);

                            if let Some(tool_calls) = tool_calls {
                                let mut errors = Vec::new();
                                should_continue = run_tool_calls(
                                    &tool_calls,
                                    Some(ui.clone()),
                                    is_headless,
                                    &mut |msg| messages.lock().unwrap().push(msg),
                                    &mut |err| errors.push(err),
                                )
                                .await;
                                for err in errors {
                                    self_clone.add_assistant_message_with_update_ui(
                                        &messages, err, &ui,
                                    );
                                }
                            }

                            update_chat_ui(&ui, get_ui_messages(&messages.lock().unwrap()));
                        }
                        Err(req_err) => {
                            let error_msg = format!("Request Error: {}", req_err);
                            self_clone.add_assistant_message_with_update_ui(
                                &messages, error_msg, &ui,
                            );
                        }
                    }
                }

                self_clone.save_session();
                self_clone.request_in_flight.store(false, Ordering::SeqCst);
                send_status(&ui, "");
            }
        });
    }
}

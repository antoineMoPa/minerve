use crate::token_counter::{TokenCounter, UsageSnapshot};
use reqwest::Client;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Sends `request` to the API of `provider` and records its usage in `token_counter`.
/// `url` is the endpoint of that provider; `api_key` is the key of that provider.
pub async fn post_request_with_token_count(
    client: &Client,
    provider: Provider,
    url: &str,
    api_key: &str,
    request: ChatCompletionRequest,
    token_counter: Arc<TokenCounter>,
) -> Result<ChatCompletionResponse, String> {
    let chat_response = match provider {
        Provider::Anthropic => anthropic::post_messages(client, url, api_key, &request).await?,
        Provider::OpenRouter | Provider::OpenAi => {
            post_chat_completions(client, url, api_key, &request).await?
        }
    };

    if let Some(ref usage) = chat_response.usage {
        // Correctly use the increment with the provided token_counter
        token_counter.increment_prompt(usage.prompt_tokens as usize);
        token_counter.increment_completion(usage.completion_tokens as usize);
        token_counter.add_request_cost(usage.cost);
        token_counter.set_last_prompt_tokens(usage.prompt_tokens as usize);
        if let Some(details) = &usage.prompt_tokens_details {
            token_counter.increment_cached_prompt(details.cached_tokens as usize);
        }
    }

    Ok(chat_response)
}

/// OpenRouter and OpenAI share the chat completions request and answer shape.
async fn post_chat_completions(
    client: &Client,
    url: &str,
    api_key: &str,
    request: &ChatCompletionRequest,
) -> Result<ChatCompletionResponse, String> {
    let response = client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(request)
        .send()
        .await
        .map_err(|err| err.to_string())?;

    let status = response.status();
    let body = response.text().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("{}: {}", status, body));
    }

    serde_json::from_str(&body)
        .map_err(|err| format!("Unexpected response ({}): {}", err, body))
}

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tokio::sync::Notify;

use crate::anthropic;
use crate::openrouter::OpenRouterConnection;
use crate::provider::{ActiveModel, ApiKeys, Provider};
use crate::session::{self, SessionFile, SessionStart};
use crate::mcp::McpServers;
use crate::tools::registry::get_tool_registry;
use crate::tools::utils::cap_tool_output;
use crate::control_socket::{AgentState, ControlSocket, MinerveEvent};
use crate::chat::PromptAuthor;
use crate::goal;
use crate::ui::{send_notice, send_status, update_chat_ui, UiSender};
use crate::{
    ChatCompletionFunctionDefinition, ChatCompletionMessage, ChatCompletionMessageRole,
    ChatCompletionRequest, ChatCompletionResponse, ChatCompletionToolCall,
    ChatCompletionToolDefinition, RequestMessage, ToolCallResult, UsageAccountingRequest,
    EPHEMERAL_CACHE_CONTROL,
};

pub struct Minerve {
    /// Id of the session file this conversation is saved to. Replaced by `replace_session`.
    session_id: Mutex<String>,
    pub messages: Arc<Mutex<Vec<ChatCompletionMessage>>>,
    pub client: Client,
    /// Keys of every provider. A request fails if the key of its model's provider is not set.
    /// Replaced by `set_openrouter_key` from the first-start modal.
    pub api_keys: Mutex<ApiKeys>,
    /// OpenRouter base url, e.g. https://openrouter.ai/api/v1.
    pub base_url: String,
    /// Model the next request goes to. Replaced by `set_model` from the `/model` picker.
    pub model: Mutex<ActiveModel>,
    pub request_in_flight: Arc<AtomicBool>,
    /// Set by `cancel_turn`; checked before each request and tool call of the running turn.
    pub cancel_requested: Arc<AtomicBool>,
    /// Wakes a request that is waiting for the API when the turn is cancelled.
    pub cancel_notify: Arc<Notify>,
    pub token_counter: Arc<TokenCounter>,
    /// `--append-system-prompt` texts, added to every new session (see `start_new_session`).
    extra_system_prompts: Vec<String>,
    /// Socket of the program that started minerve, told about the conversation as it goes.
    /// See `control_socket`.
    control: Option<Arc<ControlSocket>>,
    /// `/budget`: the session's cost that pauses it. See `budget`.
    pub budget: crate::budget::Budget,
}

/// Role label in the chat transcript of the prompts the `/goal` evaluator sends.
pub const GOAL_EVALUATOR_UI_ROLE: &str = "goal evaluator";

pub fn get_system_prompt() -> String {
    return String::from(include_str!("../prompts/MAIN_SYSTEM_PROMPT.txt"));
}

/// Built-in tools first, then MCP server tools.
fn get_tool_definitions(uses_cache_breakpoints: bool, mcp: &McpServers) -> Option<Vec<ChatCompletionToolDefinition>> {
    let registry = get_tool_registry();
    let builtin_functions = registry.values().map(|tool| ChatCompletionFunctionDefinition {
        name: tool.name().to_string(),
        description: Some(tool.description().to_string()),
        parameters: Some(tool.function_definition()),
    });
    let mut tools: Vec<ChatCompletionToolDefinition> = builtin_functions
        .chain(mcp.function_definitions())
        .map(|function| ChatCompletionToolDefinition {
            tool_type: "function",
            function,
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
            ChatCompletionMessageRole::User => match msg.prompt_author {
                None => String::from("user"),
                Some(PromptAuthor::GoalEvaluator) => String::from(GOAL_EVALUATOR_UI_ROLE),
            },
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
        let mut text = content.clone();
        for image in &msg.images {
            text.push_str(&format!("\n[image: {}]", image.display()));
        }
        ui_messages.push((role_label, text));
    }
    ui_messages
}

/// Consecutive calls to the same tool, as (name, count), in call order.
fn group_tool_calls(tool_calls: &[ChatCompletionToolCall]) -> Vec<(&str, usize)> {
    let mut groups: Vec<(&str, usize)> = Vec::new();
    for tool_call in tool_calls {
        let name = tool_call.function.name.as_str();
        match groups.last_mut() {
            Some((last, count)) if *last == name => *count += 1,
            _ => groups.push((name, 1)),
        }
    }
    groups
}

/// One line per run of identical tool calls, e.g. `Handling tool call: read x 3`.
fn format_tool_call_group(name: &str, count: usize) -> String {
    if count == 1 {
        format!("Handling tool call: {}", name)
    } else {
        format!("Handling tool call: {} x {}", name, count)
    }
}

/// Every tool call of an assistant message needs a tool message answering it,
/// otherwise OpenRouter rejects the next request.
/// Returns whether the conversation should continue with another request.
async fn run_tool_calls(
    tool_calls: &[ChatCompletionToolCall],
    mcp: &McpServers,
    ui: Option<UiSender>,
    is_headless: bool,
    cancel_requested: &AtomicBool,
    on_tool_message: &mut impl FnMut(ChatCompletionMessage),
    on_error: &mut impl FnMut(String),
) -> bool {
    let mut should_continue = true;
    for tool_call in tool_calls {
        // Cancelled calls are still answered, so the history stays valid for the next prompt.
        if cancel_requested.load(Ordering::SeqCst) {
            on_tool_message(ChatCompletionMessage::tool_result(
                tool_call.id.clone(),
                String::from("Skipped: the turn was cancelled."),
            ));
            should_continue = false;
            continue;
        }
        if !should_continue {
            on_tool_message(ChatCompletionMessage::tool_result(
                tool_call.id.clone(),
                String::from("Skipped: a previous tool call failed."),
            ));
            continue;
        }
        match handle_tool_call(tool_call, mcp, ui.clone(), is_headless).await {
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
    mcp: &McpServers,
    ui: Option<UiSender>,
    is_headless: bool,
) -> ToolCallResult {
    let settings = crate::tools::ExecuteCommandSettings { is_headless };
    let registry = get_tool_registry();
    let tool_name = &tool_call.function.name;
    let args_str = &tool_call.function.arguments;

    if let Some(ui) = &ui {
        send_status(ui, format!("Running tool: {}", tool_name));
    }
    if let Some(mcp_result) = mcp.call(tool_name, args_str).await {
        return match mcp_result {
            Ok(output) => ToolCallResult::Success(ChatCompletionMessage::tool_result(tool_call.id.clone(), output)),
            Err(err) => ToolCallResult::Error(format!("MCP tool '{}': {}", tool_name, err)),
        };
    }

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

        let (result, images) = tool.run_with_images(args, settings).await;

        if let Some(ui) = &ui {
            send_status(ui, format!("Reading tool result: {}", tool_name));
        }

        ToolCallResult::Success(ChatCompletionMessage {
            images,
            ..ChatCompletionMessage::tool_result(tool_call.id.clone(), result)
        })
    } else {
        ToolCallResult::Error(format!("Function '{}' not found", tool_name))
    }
}

/// Called when a new prompt is added. Tool results and images of earlier prompts are dropped
/// then, never during a run of tool calls, so every request of a run extends the previous one
/// and hits the prompt cache. The agent can open a dropped image again with `view_image`.
fn drop_tool_outputs_and_images_of_previous_prompts(messages: &mut [ChatCompletionMessage]) {
    for msg in messages.iter_mut() {
        if let ChatCompletionMessageRole::Tool = msg.role {
            msg.content = Some(String::from("[cleaned from history]"));
        }
        for image in std::mem::take(&mut msg.images) {
            let note = format!("[image {} no longer attached; open it with view_image if needed]", image.display());
            msg.content = Some(match msg.content.take() {
                Some(text) => format!("{}\n{}", text, note),
                None => note,
            });
        }
    }
}

impl Minerve {
    /// Model the next turn will use. A turn reads it once when it starts.
    pub fn active_model(&self) -> ActiveModel {
        self.model.lock().unwrap().clone()
    }

    /// Switches the model for the next turn. A turn already running keeps the model it started with.
    pub fn set_model(&self, model_id: String) {
        *self.model.lock().unwrap() = ActiveModel::new(model_id);
    }

    /// Copy of the keys this session currently uses, e.g. to show which ones are already set.
    pub fn api_keys_snapshot(&self) -> ApiKeys {
        self.api_keys.lock().unwrap().clone()
    }

    /// Sets the provider keys entered in the setup modal. A None key keeps the key already held.
    pub fn set_api_keys(&self, openrouter: Option<String>, openai: Option<String>, anthropic: Option<String>) {
        let mut keys = self.api_keys.lock().unwrap();
        if openrouter.is_some() {
            keys.openrouter = openrouter;
        }
        if openai.is_some() {
            keys.openai = openai;
        }
        if anthropic.is_some() {
            keys.anthropic = anthropic;
        }
    }

    /// Fails when an image of the history cannot be read.
    fn build_request(&self, model: &ActiveModel, history: &[ChatCompletionMessage], mcp: &McpServers) -> Result<ChatCompletionRequest, String> {
        Ok(ChatCompletionRequest {
            model: model.api_model.clone(),
            messages: build_request_messages(history, model.uses_cache_breakpoints)?,
            tools: get_tool_definitions(model.uses_cache_breakpoints, mcp),
            usage: (model.provider == Provider::OpenRouter)
                .then_some(UsageAccountingRequest { include: true }),
        })
    }

    /// URL and key of the API the model is served from. Fails when that provider has no key set.
    pub(crate) fn endpoint_for(&self, model: &ActiveModel) -> Result<(String, String), String> {
        let url = model.provider.request_url(&self.base_url);
        let api_key = self.api_keys.lock().unwrap().for_provider(model.provider)?.to_string();
        Ok((url, api_key))
    }
}

/// Breakpoints, when the model needs them, go after the system prompt and on the latest
/// message, like opencode does: the next request reads everything up to there from cache.
fn build_request_messages(history: &[ChatCompletionMessage], uses_cache_breakpoints: bool) -> Result<Vec<RequestMessage>, String> {
    let history = crate::chat::with_tool_images_in_user_messages(
        crate::chat::with_placeholder_results_for_unanswered_tool_calls(history),
    );
    let mut messages: Vec<RequestMessage> = history
        .iter()
        .map(RequestMessage::from_message)
        .collect::<Result<_, _>>()?;
    if uses_cache_breakpoints {
        messages
            .first_mut()
            .expect("request without messages")
            .add_cache_breakpoint();
        if messages.len() > 1 {
            messages.last_mut().unwrap().add_cache_breakpoint();
        }
    }
    Ok(messages)
}

/// System messages a new session opens with. AGENTS.md files come after the default prompt and
/// before the --append-system-prompt ones.
fn new_session_messages(extra_system_prompts: &[String]) -> Vec<ChatCompletionMessage> {
    let mut messages = vec![ChatCompletionMessage::system(get_system_prompt())];
    messages.extend(
        crate::agents_md::load_agents_md()
            .into_iter()
            .chain(extra_system_prompts.iter().cloned())
            .map(ChatCompletionMessage::system),
    );
    messages
}

impl Minerve {
    /// Prints assistant replies to stdout and errors to stderr as the conversation goes.
    pub async fn chat_headless(&self) {
        let is_headless = true;

        let mut history: Vec<ChatCompletionMessage> = {
            let msgs = self.messages.lock().unwrap();
            msgs.clone()
        };

        let model = self.active_model();
        let mut should_continue = true;

        while should_continue {
            should_continue = false;
            let mcp = crate::mcp::connected_servers().await;

            let chat_result = match self
                .endpoint_for(&model)
                .and_then(|endpoint| Ok((endpoint, self.build_request(&model, &history, &mcp)?)))
            {
                Ok(((url, api_key), request)) => {
                    post_request_with_token_count(
                        &self.client,
                        model.provider,
                        &url,
                        &api_key,
                        request,
                        self.token_counter.clone(),
                    )
                    .await
                }
                Err(err) => Err(err),
            };

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
                        for (name, count) in group_tool_calls(&tool_calls) {
                            println!("{}", format_tool_call_group(name, count));
                        }
                        should_continue = run_tool_calls(
                            &tool_calls,
                            &mcp,
                            None,
                            is_headless,
                            &AtomicBool::new(false),
                            &mut |msg| history.push(msg),
                            &mut |err| eprintln!("{}", err),
                        )
                        .await;
                        println!("{}", self.token_counter.format_usage_summary());
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

    fn add_assistant_message_with_update_ui(&self, message_content: String, ui: &UiSender) {
        self.push_message(ChatCompletionMessage::assistant(message_content));
        update_chat_ui(ui, get_ui_messages(&self.messages.lock().unwrap()));
    }

    /// Adds a message to the conversation and tells the control socket about it.
    /// Sent under the messages lock, like `send_history_to_control`, so a `history` answer
    /// either holds the message or comes before its `message` event.
    fn push_message(&self, message: ChatCompletionMessage) {
        let mut messages = self.messages.lock().unwrap();
        self.send_control_event(&MinerveEvent::Message { message: &message });
        messages.push(message);
    }

    pub fn send_control_event(&self, event: &MinerveEvent) {
        if let Some(control) = &self.control {
            control.send(event);
        }
    }

    /// Answers the control socket's `get_history` with the whole conversation.
    pub fn send_history_to_control(&self) {
        let session_id = self.current_session_id();
        let messages = self.messages.lock().unwrap();
        self.send_control_event(&MinerveEvent::History { session_id: &session_id, messages: &messages });
    }

    /// Starts a new session, or continues a saved one.
    /// `extra_system_prompts` are added after the default system prompt of a new session.
    pub fn new(
        openrouter_connection: OpenRouterConnection,
        start: SessionStart,
        extra_system_prompts: &[String],
        control: Option<Arc<ControlSocket>>,
    ) -> Self {
        let token_counter = Arc::new(TokenCounter::new());
        let save_now = matches!(start, SessionStart::NewWithId(_));
        let (session_id, messages) = match start {
            SessionStart::Resume(file) => {
                token_counter.restore(&file.usage);
                (file.id, file.messages)
            }
            SessionStart::New => (
                uuid::Uuid::new_v4().to_string(),
                new_session_messages(extra_system_prompts),
            ),
            SessionStart::NewWithId(id) => (id, new_session_messages(extra_system_prompts)),
        };
        let minerve = Self {
            session_id: Mutex::new(session_id),
            messages: Arc::new(Mutex::new(messages)),
            client: Client::new(),
            api_keys: Mutex::new(openrouter_connection.api_keys),
            base_url: openrouter_connection.base_url,
            model: Mutex::new(ActiveModel::new(openrouter_connection.model)),
            request_in_flight: Arc::new(AtomicBool::new(false)),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            cancel_notify: Arc::new(Notify::new()),
            token_counter,
            extra_system_prompts: extra_system_prompts.to_vec(),
            control,
            budget: Default::default(),
        };
        if save_now {
            minerve.save_session();
        }
        minerve.send_control_event(&MinerveEvent::Session { session_id: &minerve.current_session_id() });
        minerve.send_control_event(&MinerveEvent::State { state: AgentState::WaitingForInput });
        minerve
    }

    /// Looks up the context window of the model in the background, so the status bar can show a percent.
    /// Only OpenRouter has the model list; other providers show no percent.
    pub fn fetch_context_window_in_background(self: &Arc<Self>) {
        let minerve = Arc::clone(self);
        let model = minerve.active_model();
        if model.provider != Provider::OpenRouter {
            return;
        }
        let model_id = model.api_model;
        crate::get_global_runtime().spawn(async move {
            if let Some(tokens) = crate::openrouter::fetch_context_window(
                &minerve.client,
                &minerve.base_url,
                &model_id,
            )
            .await
            {
                minerve.token_counter.set_context_window(tokens);
            }
        });
    }

    /// `/budget <arg>`: sets, drops or shows the budget, and says which.
    pub fn budget_command(&self, arg: &str) -> String {
        let reports_cost = self.active_model().provider == Provider::OpenRouter;
        self.budget.command(arg, self.token_counter.current_cost_usd(), reports_cost)
    }

    /// The notice to pause with, once the session cost the budget.
    pub fn over_budget(&self) -> Option<String> {
        self.budget.exceeded(self.token_counter.current_cost_usd())
    }

    /// Asks the running turn to stop. The request in flight is dropped, and tool calls not
    /// started yet are skipped, so the conversation stays valid for the next prompt.
    pub fn cancel_turn(&self) {
        self.cancel_requested.store(true, Ordering::SeqCst);
        self.cancel_notify.notify_waiters();
    }

    /// Writes the conversation to its session file. Failures are ignored: a missing save
    /// must not interrupt the chat.
    pub fn save_session(&self) {
        let msgs = self.messages.lock().unwrap();
        let session_id = self.session_id.lock().unwrap();
        let _ = session::save_session(&session_id, &msgs, self.token_counter.snapshot());
    }

    /// Id of the session this conversation is saved to.
    pub fn current_session_id(&self) -> String {
        self.session_id.lock().unwrap().clone()
    }

    /// `/clear`: starts a fresh conversation under a new session id, with the same system prompts
    /// as a new session. Nothing is saved until the first message, like `SessionStart::New`.
    /// The caller makes sure no turn is running.
    pub fn start_new_session(&self) {
        self.token_counter.restore(&UsageSnapshot::default());
        *self.messages.lock().unwrap() = new_session_messages(&self.extra_system_prompts);
        *self.session_id.lock().unwrap() = uuid::Uuid::new_v4().to_string();
        self.send_control_event(&MinerveEvent::Session { session_id: &self.current_session_id() });
    }

    /// Continues another saved session: its messages, id and usage replace the current ones.
    /// The caller makes sure no turn is running.
    pub fn replace_session(&self, file: SessionFile) {
        self.token_counter.restore(&file.usage);
        *self.messages.lock().unwrap() = file.messages;
        *self.session_id.lock().unwrap() = file.id;
        self.send_control_event(&MinerveEvent::Session { session_id: &self.current_session_id() });
    }

    /// Marks a turn as running and adds the user's prompt to the conversation.
    fn start_turn(&self, prompt: ChatCompletionMessage, ui: &UiSender) {
        self.request_in_flight.store(true, Ordering::SeqCst);
        self.cancel_requested.store(false, Ordering::SeqCst);
        self.send_control_event(&MinerveEvent::State { state: AgentState::Working });
        self.add_prompt(prompt, ui);
        send_status(ui, "working...");
    }

    /// Adds a user-role message, written by the user or by minerve (see `PromptAuthor`).
    fn add_prompt(&self, prompt: ChatCompletionMessage, ui: &UiSender) {
        drop_tool_outputs_and_images_of_previous_prompts(&mut self.messages.lock().unwrap());
        self.push_message(prompt);
        update_chat_ui(ui, get_ui_messages(&self.messages.lock().unwrap()));
    }

    fn end_turn(&self, ui: &UiSender) {
        self.save_session();
        self.request_in_flight.store(false, Ordering::SeqCst);
        self.send_control_event(&MinerveEvent::State { state: AgentState::WaitingForInput });
        send_status(ui, "");
    }

    /// Requests and tool calls until the model answers without tool calls, a tool fails, or the turn is cancelled.
    /// Fails with the error of a request that failed; it is also added to the chat.
    async fn run_turn(&self, ui: &UiSender, is_headless: bool) -> Result<(), String> {
        let messages = self.messages.clone();
        let cancel_requested = self.cancel_requested.clone();
        // One model for the whole turn, even if `/model` switches it meanwhile.
        let model = self.active_model();
        let mut should_continue = true;

        while should_continue {
            should_continue = false;

            // Servers still connecting (at launch, or from `/mcp`) are waited for.
            if crate::mcp::is_any_connecting() {
                send_status(ui, "connecting to MCP servers...");
            }
            let mcp = crate::mcp::connected_servers().await;

            if let Some(notice) = self.over_budget() {
                send_notice(ui, notice);
                break;
            }

            // Show working indicator at start of each loop iteration
            send_status(ui, "working...");

            let request = {
                let msgs = messages.lock().unwrap();
                self.build_request(&model, &msgs, &mcp)
            };

            // Register for the cancel signal before checking the flag, so a cancel
            // arriving in between is not missed.
            let cancelled = self.cancel_notify.notified();
            tokio::pin!(cancelled);
            cancelled.as_mut().enable();
            let chat_result = match self.endpoint_for(&model).and_then(|endpoint| Ok((endpoint, request?))) {
                Err(err) => Some(Err(err)),
                Ok(_) if cancel_requested.load(Ordering::SeqCst) => None,
                Ok(((url, api_key), request)) => {
                    tokio::select! {
                        result = post_request_with_token_count(
                            &self.client,
                            model.provider,
                            &url,
                            &api_key,
                            request,
                            self.token_counter.clone(),
                        ) => Some(result),
                        _ = &mut cancelled => None,
                    }
                }
            };

            match chat_result {
                // Cancelled: nothing was added, so the conversation ends on the last user
                // or tool message and the next prompt can follow.
                None => {}
                Some(Ok(response)) => {
                    let assistant_message = response
                        .choices
                        .into_iter()
                        .next()
                        .expect("chat completion response without choices")
                        .message;

                    let tool_calls = assistant_message.tool_calls.clone();
                    self.push_message(assistant_message);

                    if let Some(tool_calls) = tool_calls {
                        let mut errors = Vec::new();
                        should_continue = run_tool_calls(
                            &tool_calls,
                            &mcp,
                            Some(ui.clone()),
                            is_headless,
                            &cancel_requested,
                            &mut |msg| self.push_message(msg),
                            &mut |err| errors.push(err),
                        )
                        .await;
                        for err in errors {
                            self.add_assistant_message_with_update_ui(err, ui);
                        }
                    }

                    update_chat_ui(ui, get_ui_messages(&messages.lock().unwrap()));
                }
                Some(Err(req_err)) => {
                    let error_msg = format!("Request Error: {}", req_err);
                    self.add_assistant_message_with_update_ui(error_msg, ui);
                    return Err(req_err);
                }
            }
        }
        Ok(())
    }

    /// Sends a prompt typed by the user, with the image files pasted into it.
    pub fn chat_with_arc(self: Arc<Self>, user_input: String, images: Vec<PathBuf>, ui: UiSender, is_headless: bool) {
        self.start_turn(ChatCompletionMessage::user_with_images(user_input, images), &ui);
        crate::get_global_runtime().spawn(async move {
            // A failed request is already shown in the chat.
            let _ = self.run_turn(&ui, is_headless).await;
            self.end_turn(&ui);
        });
    }

    /// `/goal`: takes turns until the goal evaluator judges the goal reached, the user cancels
    /// with Esc, the evaluator fails, or `MAX_GOAL_TURNS` turns ran. How it ended is sent as a notice.
    pub fn pursue_goal(self: Arc<Self>, goal: String, ui: UiSender) {
        let goal_start = self.messages.lock().unwrap().len();
        self.start_turn(ChatCompletionMessage::user(goal::start_prompt(&goal)), &ui);
        crate::get_global_runtime().spawn(async move {
            let provider = self.active_model().provider;
            let mut outcome = goal::GoalOutcome::NotReached;
            // Copy of the goal's messages for the evaluator. A new prompt cleans tool results out of
            // `messages`; this copy keeps them, so every turn's results can be checked.
            let mut goal_messages: Vec<ChatCompletionMessage> = Vec::new();
            let mut copied_until = goal_start;
            for turn in 1..=goal::MAX_GOAL_TURNS {
                if let Err(error) = self.run_turn(&ui, false).await {
                    outcome = goal::GoalOutcome::RequestFailed { error };
                    break;
                }
                if self.over_budget().is_some() {
                    outcome = goal::GoalOutcome::OverBudget;
                    break;
                }
                // Registered before checking the flag, so a cancel arriving in between is not missed.
                let cancelled = self.cancel_notify.notified();
                tokio::pin!(cancelled);
                cancelled.as_mut().enable();
                if self.cancel_requested.load(Ordering::SeqCst) {
                    outcome = goal::GoalOutcome::Cancelled;
                    break;
                }
                send_status(&ui, format!("goal: evaluating turn {}/{}...", turn, goal::MAX_GOAL_TURNS));
                {
                    let messages = self.messages.lock().unwrap();
                    goal_messages.extend_from_slice(&messages[copied_until..]);
                    copied_until = messages.len();
                }
                let verdict = tokio::select! {
                    verdict = goal::evaluate(&self, provider, &goal, &goal_messages) => verdict,
                    _ = &mut cancelled => {
                        outcome = goal::GoalOutcome::Cancelled;
                        break;
                    }
                };
                let (verdict, evaluator_model_id) = match verdict {
                    Ok(verdict) => verdict,
                    Err(errors) => {
                        outcome = goal::GoalOutcome::EvaluatorFailed { errors };
                        break;
                    }
                };
                match verdict.status {
                    goal::GoalStatus::Reached => {
                        outcome = goal::GoalOutcome::Reached { turns: turn, evaluator_model_id, reason: verdict.reason };
                        break;
                    }
                    goal::GoalStatus::Stalled => {
                        outcome = goal::GoalOutcome::Stalled { turns: turn, evaluator_model_id, reason: verdict.reason };
                        break;
                    }
                    goal::GoalStatus::NotReached => {
                        self.add_prompt(
                            ChatCompletionMessage::authored_prompt(
                                PromptAuthor::GoalEvaluator,
                                goal::continue_prompt(&goal, &verdict.reason),
                            ),
                            &ui,
                        )
                    }
                }
            }
            send_notice(&ui, outcome.notice());
            self.end_turn(&ui);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_prompt_drops_earlier_images_and_names_them() {
        let mut messages = vec![
            ChatCompletionMessage::user_with_images(String::from("look [Image #1]"), vec![PathBuf::from("/a.png")]),
            ChatCompletionMessage {
                images: vec![PathBuf::from("/b.png")],
                ..ChatCompletionMessage::tool_result(String::from("1"), String::from("attached"))
            },
        ];
        drop_tool_outputs_and_images_of_previous_prompts(&mut messages);
        assert!(messages.iter().all(|m| m.images.is_empty()));
        assert_eq!(
            messages[0].content.as_deref(),
            Some("look [Image #1]\n[image /a.png no longer attached; open it with view_image if needed]")
        );
        assert_eq!(
            messages[1].content.as_deref(),
            Some("[cleaned from history]\n[image /b.png no longer attached; open it with view_image if needed]")
        );
    }
}

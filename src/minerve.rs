use crate::token_counter::TokenCounter;
use cursive::views::{ResizedView, TextView};
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;

pub async fn post_request_with_token_count(
    client: &Client,
    url: &str,
    api_key: &str,
    request: ChatCompletionRequest,
    _cb_sink: Option<&cursive::CbSink>,
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
    }

    Ok(chat_response)
}

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

const HIST_CUTOFF: usize = 30;

use crate::openrouter::OpenRouterConnection;
use crate::tools::registry::get_tool_registry;
use crate::tools::run_shell_command_tool::RunShellCommandTool;
use crate::tools::utils::cap_tool_output;
use crate::{
    update_chat_ui, ChatCompletionFunctionDefinition, ChatCompletionMessage,
    ChatCompletionMessageRole, ChatCompletionRequest, ChatCompletionResponse,
    ChatCompletionToolCall, ChatCompletionToolDefinition, ToolCallResult, UsageAccountingRequest,
};

pub struct Minerve {
    pub messages: Arc<Mutex<Vec<ChatCompletionMessage>>>,
    pub client: Client,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub request_in_flight: Arc<AtomicBool>,
    pub token_counter: Arc<TokenCounter>,
}

pub fn get_system_prompt() -> String {
    return String::from(include_str!("../prompts/MAIN_SYSTEM_PROMPT.txt"));
}

fn get_tool_definitions() -> Option<Vec<ChatCompletionToolDefinition>> {
    let tools: Vec<ChatCompletionToolDefinition> = get_tool_registry()
        .values()
        .map(|tool| ChatCompletionToolDefinition {
            tool_type: "function",
            function: ChatCompletionFunctionDefinition {
                name: tool.name().to_string(),
                description: Some(tool.description().to_string()),
                parameters: Some(tool.function_definition()),
            },
        })
        .collect();
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
    cb_sink: Option<cursive::CbSink>,
    is_headless: bool,
    on_tool_message: &mut impl FnMut(ChatCompletionMessage),
    on_error: &mut impl FnMut(String),
) -> bool {
    let mut should_continue = true;
    for tool_call in tool_calls {
        if !should_continue {
            on_tool_message(ChatCompletionMessage::tool_result(
                tool_call.id.clone(),
                String::from("Skipped: a previous tool call was cancelled or failed."),
            ));
            continue;
        }
        match handle_tool_call(tool_call, cb_sink.clone(), is_headless).await {
            ToolCallResult::Success(mut msg) => {
                msg.content = msg.content.map(cap_tool_output);
                on_tool_message(msg)
            }
            ToolCallResult::Cancelled => {
                on_tool_message(ChatCompletionMessage::tool_result(
                    tool_call.id.clone(),
                    String::from("Cancelled by the user."),
                ));
                should_continue = false;
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
    cb_sink: Option<cursive::CbSink>,
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

        if tool_name.as_str() == "run_shell_command" {
            if let Some(cb_sink) = &cb_sink {
                use cursive::views::Dialog;
                use std::sync::mpsc::sync_channel;

                let (tx, rx) = sync_channel::<bool>(0);
                let command = args.get("command").unwrap_or(&String::new()).clone();

                let tx_yes = tx.clone();
                let tx_no = tx.clone();
                let command_for_dialog = command.clone();

                // Send confirmation dialog to main UI
                let cb_sink_clone = cb_sink.clone();
                cb_sink_clone
                    .send(Box::new(move |s| {
                        s.add_layer(
                            Dialog::text(format!(
                                "Accept running the following shell command?\n{}",
                                command_for_dialog
                            ))
                            .button("Yes", move |s| {
                                s.pop_layer();
                                let _ = tx_yes.send(true);
                            })
                            .button("No", move |s| {
                                s.pop_layer();
                                let _ = tx_no.send(false);
                            }),
                        );
                    }))
                    .unwrap();

                // Wait for user confirmation
                let confirmed = rx.recv().unwrap_or(false);
                if !confirmed {
                    return ToolCallResult::Cancelled;
                }

                let output = RunShellCommandTool::execute_command(&command, Some(settings));

                return ToolCallResult::Success(ChatCompletionMessage::tool_result(
                    tool_call.id.clone(),
                    output,
                ));
            }
        }

        let function_name_for_indicator = tool_name.clone();

        // Show working indicator
        if let Some(cb_sink) = &cb_sink {
            let _ = cb_sink.send(Box::new(move |s| {
                if let Some(mut view) = s.find_name::<ResizedView<TextView>>("working_textview") {
                    let message = format!("Running tool: {}", function_name_for_indicator);
                    view.get_inner_mut().set_content(message);
                } else {
                    panic!("working_textview view not found");
                }
            }));
        }

        let result = tool.run(args, settings).await;

        let function_name_for_indicator = tool_name.clone();

        // Hide working indicator
        if let Some(cb_sink) = &cb_sink {
            let _ = cb_sink.send(Box::new(move |s| {
                if let Some(mut view) = s.find_name::<ResizedView<TextView>>("working_textview") {
                    let message = format!("Reading tool result: {}", function_name_for_indicator);
                    view.get_inner_mut().set_content(message);
                } else {
                    panic!("working_textview view not found");
                }
            }));
        }

        ToolCallResult::Success(ChatCompletionMessage::tool_result(
            tool_call.id.clone(),
            result,
        ))
    } else {
        ToolCallResult::Error(format!("Function '{}' not found", tool_name))
    }
}

fn clean_old_tool_outputs(history: &[ChatCompletionMessage]) -> Vec<ChatCompletionMessage> {
    let mut cleaned_history = history.to_vec();
    let old_message_count = cleaned_history.len().saturating_sub(HIST_CUTOFF);
    for msg in cleaned_history.iter_mut().take(old_message_count) {
        if let ChatCompletionMessageRole::Tool = msg.role {
            msg.content = Some(String::from("[cleaned from history]"));
        }
    }
    cleaned_history
}

impl Minerve {
    fn build_request(&self, history: &[ChatCompletionMessage]) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: self.model.clone(),
            messages: clean_old_tool_outputs(history),
            tools: get_tool_definitions(),
            usage: UsageAccountingRequest { include: true },
        }
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
                None,
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
    }

    fn add_assistant_message_with_update_ui(
        &self,
        messages: &Arc<Mutex<Vec<ChatCompletionMessage>>>,
        message_content: String,
        cb_sink: &cursive::CbSink,
    ) {
        let mut msgs = messages.lock().unwrap();
        msgs.push(ChatCompletionMessage::assistant(message_content));

        let request_status = false;
        update_chat_ui(
            cb_sink.clone(),
            get_ui_messages(&msgs),
            request_status,
            self.token_counter.clone(),
        );
    }

    pub fn new(openrouter_connection: OpenRouterConnection) -> Self {
        Self {
            messages: Arc::new(Mutex::new(vec![ChatCompletionMessage::system(
                get_system_prompt(),
            )])),
            client: Client::new(),
            api_key: openrouter_connection.api_key,
            base_url: openrouter_connection.base_url,
            model: openrouter_connection.model,
            request_in_flight: Arc::new(AtomicBool::new(false)),
            token_counter: Arc::new(TokenCounter::new()),
        }
    }

    pub fn chat_with_arc(
        self: Arc<Self>,
        user_input: String,
        cb_sink: cursive::CbSink,
        is_headless: bool,
    ) {
        use std::sync::atomic::Ordering;

        self.request_in_flight.store(true, Ordering::SeqCst);

        {
            let mut msgs = self.messages.lock().unwrap();
            msgs.push(ChatCompletionMessage::user(user_input));

            let request_status = false;
            update_chat_ui(
                cb_sink.clone(),
                get_ui_messages(&msgs),
                request_status,
                self.token_counter.clone(),
            );
        }

        // Show working indicator
        cb_sink
            .send(Box::new(|s| {
                if let Some(mut view) = s.find_name::<ResizedView<TextView>>("working_textview") {
                    view.get_inner_mut().set_content("working...");
                } else {
                    panic!("working_textview view not found");
                }
            }))
            .unwrap();

        crate::get_global_runtime().spawn({
            let self_clone = self.clone();
            async move {
                let messages = self_clone.messages.clone();
                let mut should_continue = true;

                while should_continue {
                    should_continue = false;

                    // Show working indicator at start of each loop iteration
                    cb_sink
                        .send(Box::new(|s| {
                            if let Some(mut view) =
                                s.find_name::<ResizedView<TextView>>("working_textview")
                            {
                                view.get_inner_mut().set_content("working...");
                            } else {
                                panic!("working_textview view not found");
                            }
                        }))
                        .unwrap();

                    let request = {
                        let msgs = messages.lock().unwrap();
                        self_clone.build_request(&msgs)
                    };

                    let chat_result = post_request_with_token_count(
                        &self_clone.client,
                        &self_clone.get_completions_url(),
                        &self_clone.api_key,
                        request,
                        Some(&cb_sink),
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
                                    Some(cb_sink.clone()),
                                    is_headless,
                                    &mut |msg| messages.lock().unwrap().push(msg),
                                    &mut |err| errors.push(err),
                                )
                                .await;
                                for err in errors {
                                    self_clone.add_assistant_message_with_update_ui(
                                        &messages, err, &cb_sink,
                                    );
                                }
                            }

                            let request_status = false;
                            update_chat_ui(
                                cb_sink.clone(),
                                get_ui_messages(&messages.lock().unwrap()),
                                request_status,
                                self_clone.token_counter.clone(),
                            );
                        }
                        Err(req_err) => {
                            let error_msg = format!("Request Error: {}", req_err);
                            self_clone.add_assistant_message_with_update_ui(
                                &messages, error_msg, &cb_sink,
                            );
                        }
                    }
                }

                self_clone.request_in_flight.store(false, Ordering::SeqCst);
                cb_sink
                    .send(Box::new(|s| {
                        if let Some(mut view) =
                            s.find_name::<ResizedView<TextView>>("working_textview")
                        {
                            view.get_inner_mut().set_content("");
                        } else {
                            panic!("working_textview view not found");
                        }
                    }))
                    .unwrap();
            }
        });
    }
}

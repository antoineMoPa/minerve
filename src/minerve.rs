use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;
use cursive::views::{ResizedView, TextView};
use dotenvy::from_path;
use tokio::runtime::Runtime;
use reqwest::Client;

use crate::tools::registry::get_tool_registry;
use crate::{update_chat_ui, ChatCompletionFunctionCall, ChatCompletionFunctionDefinition, ChatCompletionMessage, ChatCompletionMessageRole, ChatCompletionRequest, ChatCompletionResponse, ToolCallResult, MODEL_NAME};

pub struct Minerve {
    pub messages: Arc<Mutex<Vec<ChatCompletionMessage>>> ,
    pub rt: Runtime,
    pub client: Client,
    pub api_key: String,
    pub base_url: String,
    pub request_in_flight: Arc<AtomicBool>,
}

pub fn get_system_prompt() -> String {
    return String::from(
        r#"
const SYSTEM_PROMPT = `
You are **Minerve**, a shell assistant that behaves like a professional software developer.

Guidance:
- Be proactive at using tools instead of asking.
- Assume you are somewhere in a repository with files.
- Confirm your changes worked
 - Example: read the file after editing it.
 - Run cargo check or other compile check tool.
- Think and act swiftly, like a developper. You have limited tools, but use them effectively.
- Be curious and explore the environment before asking questions.
- First thing you should do is likely to use a tool to get context.
- Remain critical of your tools and evaluate if they work as they are still in development.
- You may be working on yourself, but the current session still uses old code.
- Privilege small changes (10 lines) with compile check in-between.
- Read and write notes abundantly like a new employee learning a code base and its tools.

Dont's:

Don't answer stuff like "I'm sorry for the confusion, but as an AI, I don't have the ability to directly modify files or write code to your project. I can provide guidance and code snippets, but you'll need to implement the changes in your project."

  - Instead, directly use the tools available to you to help the user with their coding tasks.

Don't answer stuff like "Sure, I can help with that. However, I need to know the file you want to get the code listing from. Could you please provide the file path?".

 - Instead use the tools available to you to explore the environment and find the file.

Don't answer stuff like "Now you can implement the bingBong function to get a file code listing with line numbers.   - Instead, go and implement that new function.

Don't ask questions that can be figured out from prompt, context or by using the tools available to you, like "Now, could you please specify which file you want to add the tool to?"
 - Instead, figure out yourself.

Don't say "I read file XYZ". just read it directly with the tools.
`;"#,
    );
}

pub async fn handle_tool_call(
    tool_call: &ChatCompletionFunctionCall,
    cb_sink: Option<cursive::CbSink>,
    is_headless: bool,
) -> ToolCallResult {
    let settings = crate::tools::ExecuteCommandSettings { is_headless };
    let registry = get_tool_registry();
    let tool_name = &tool_call.name;
    let args_str = &tool_call.arguments;

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

                let output = crate::tools::registry::RunShellCommandTool::execute_command(
                    &command,
                    Some(settings),
                );

                return ToolCallResult::Success(ChatCompletionMessage {
                    role: ChatCompletionMessageRole::Function,
                    content: Some(output),
                    name: Some(tool_name.clone()),
                    function_call: None,
                    tool_call_id: Some(tool_call.name.clone()),
                    tool_calls: None,
                });
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

        ToolCallResult::Success(ChatCompletionMessage {
            role: ChatCompletionMessageRole::Function,
            content: Some(result),
            name: Some(tool_name.clone()),
            function_call: None,
            tool_call_id: Some(tool_call.name.clone()),
            tool_calls: None,
        })
    } else {
        ToolCallResult::Error(format!("Function '{}' not found", tool_name))
    }
}

impl Minerve {
    fn add_assistant_message_with_update_ui(
        messages: &Arc<Mutex<Vec<ChatCompletionMessage>>>,
        message_content: String,
        cb_sink: &cursive::CbSink,
    ) {
        let mut msgs = messages.lock().unwrap();
        msgs.push(ChatCompletionMessage {
            role: ChatCompletionMessageRole::Assistant,
            content: Some(message_content),
            name: None,
            function_call: None,
            tool_call_id: None,
            tool_calls: None,
        });

        let ui_messages = msgs
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    ChatCompletionMessageRole::System => "system".to_string(),
                    ChatCompletionMessageRole::User => "user".to_string(),
                    ChatCompletionMessageRole::Assistant => "minerve".to_string(),
                    ChatCompletionMessageRole::Function => msg
                        .tool_call_id
                        .clone()
                        .unwrap_or(String::from("unknown function call")),
                };
                (role, msg.content.clone().unwrap_or_default())
            })
            .collect();

        let request_status = false;
        update_chat_ui(cb_sink.clone(), ui_messages, request_status);
    }

    pub fn new() -> Self {
        if let Some(home_dir) = dirs::home_dir() {
            let dotenv_path = home_dir.join(".env");
            if dotenv_path.exists() {
                from_path(&dotenv_path).expect("Failed to load ~/.env");
            }
        }

        let api_key = env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
        let base_url =
            env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());

        let system_message = ChatCompletionMessage {
            role: ChatCompletionMessageRole::System,
            content: Some(get_system_prompt()),
            name: None,
            function_call: None,
            tool_call_id: None,
            tool_calls: None,
        };

        Self {
            messages: Arc::new(Mutex::new(vec![system_message])),
            rt: Runtime::new().unwrap(),
            client: Client::new(),
            api_key,
            base_url,
            request_in_flight: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn chat(&self, user_input: String, cb_sink: cursive::CbSink, is_headless: bool) {
        use std::sync::atomic::Ordering;

        self.request_in_flight.store(true, Ordering::SeqCst);

        let mut msgs = self.messages.lock().unwrap();

        let user_message = ChatCompletionMessage {
            role: ChatCompletionMessageRole::User,
            content: Some(user_input.clone()),
            name: None,
            function_call: None,
            tool_call_id: None,
            tool_calls: None,
        };
        msgs.push(user_message);

        let ui_messages = msgs
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    ChatCompletionMessageRole::System => "system".to_string(),
                    ChatCompletionMessageRole::User => "user".to_string(),
                    ChatCompletionMessageRole::Assistant => "minerve".to_string(),
                    ChatCompletionMessageRole::Function => msg
                        .tool_call_id
                        .clone()
                        .unwrap_or(String::from("unknown function call")),
                };
                (role, msg.content.clone().unwrap_or_default())
            })
            .collect();

        let request_status = false;
        update_chat_ui(cb_sink.clone(), ui_messages, request_status);

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

        let messages = msgs.clone();
        drop(msgs); // unlock before async

        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let messages_clone = self.messages.clone();
        let request_in_flight = self.request_in_flight.clone();

        self.rt.spawn(async move {
            let mut history: Vec<ChatCompletionMessage> = messages;
            let registry = get_tool_registry();
            let functions: Vec<ChatCompletionFunctionDefinition> = registry
                .values()
                .map(|tool| ChatCompletionFunctionDefinition {
                    name: tool.name().to_string(),
                    description: Some(tool.description().to_string()),
                    parameters: Some(tool.function_definition()),
                })
                .collect();

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

                // Prepare history with cleaned older function outputs
                let history_len = history.len();
                let mut cleaned_history = history.clone();
                if history_len > 30 {
                    for i in 0..history_len - 30 {
                        if let ChatCompletionMessageRole::Function = cleaned_history[i].role {
                            cleaned_history[i].content = Some("[cleaned from history]".to_string());
                        }
                    }
                }

                let request = ChatCompletionRequest {
                    model: String::from(MODEL_NAME),
                    messages: cleaned_history,
                    functions: if functions.is_empty() {
                        None
                    } else {
                        Some(functions.clone())
                    },
                };

                let url = format!("{}/chat/completions", base_url);

                let chat_result = client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", api_key))
                    .header("Content-Type", "application/json")
                    .json(&request)
                    .send()
                    .await;

                match chat_result {
                    Ok(response) => {
                        match response.json::<ChatCompletionResponse>().await {
                            Ok(chat_response) => {
                                let choice = chat_response.choices.first().unwrap();
                                let assistant_message = &choice.message;

                                // Add assistant message to history
                                history.push(ChatCompletionMessage {
                                    role: ChatCompletionMessageRole::Assistant,
                                    content: assistant_message.content.clone(),
                                    name: None,
                                    function_call: assistant_message.function_call.clone(),
                                    tool_call_id: None,
                                    tool_calls: None,
                                });

                                // Add assistant response to UI
                                if let Some(content) = &assistant_message.content {
                                    let mut msgs = messages_clone.lock().unwrap();
                                    msgs.push(ChatCompletionMessage {
                                        role: ChatCompletionMessageRole::Assistant,
                                        content: Some(content.clone()),
                                        name: None,
                                        function_call: None,
                                        tool_call_id: None,
                                        tool_calls: None,
                                    });
                                }

                                // Handle function call if present
                                if let Some(function_call) = &assistant_message.function_call {
                                    let tool_call_result = handle_tool_call(
                                        function_call,
                                        Some(cb_sink.clone()),
                                        is_headless,
                                    )
                                    .await;

                                    match tool_call_result {
                                        ToolCallResult::Cancelled => break,
                                        ToolCallResult::Success(msg) => {
                                            if msg.content.is_some() {
                                                let mut msgs = messages_clone.lock().unwrap();
                                                msgs.push(msg.clone());
                                            }
                                            history.push(msg);
                                            should_continue = true;
                                        }
                                        ToolCallResult::Error(err) => {
                                            let msg =
                                                format!("Error occurred in tool call: {}", err);
                                            Minerve::add_assistant_message_with_update_ui(
                                                &messages_clone,
                                                msg,
                                                &cb_sink,
                                            );
                                            break;
                                        }
                                    }
                                }

                                let ui_messages = messages_clone
                                    .lock()
                                    .unwrap()
                                    .iter()
                                    .map(|msg| {
                                        let role = match msg.role {
                                            ChatCompletionMessageRole::System => {
                                                "system".to_string()
                                            }
                                            ChatCompletionMessageRole::User => "user".to_string(),
                                            ChatCompletionMessageRole::Assistant => {
                                                "minerve".to_string()
                                            }
                                            ChatCompletionMessageRole::Function => msg
                                                .tool_call_id
                                                .clone()
                                                .unwrap_or(String::from("unknown function call")),
                                        };
                                        (role, msg.content.clone().unwrap_or_default())
                                    })
                                    .collect();

                                let request_status = false;
                                update_chat_ui(cb_sink.clone(), ui_messages, request_status);
                            }
                            Err(json_err) => {
                                let error_msg = format!("JSON Error: {}", json_err);
                                Self::add_assistant_message_with_update_ui(
                                    &messages_clone,
                                    error_msg,
                                    &cb_sink,
                                );
                                break;
                            }
                        }
                    }
                    Err(req_err) => {
                        let error_msg = format!("Request Error: {}", req_err);
                        Self::add_assistant_message_with_update_ui(
                            &messages_clone,
                            error_msg,
                            &cb_sink,
                        );
                        break;
                    }
                }
            }

            // Hide working indicator on finish
            request_in_flight.store(false, Ordering::SeqCst);
            cb_sink
                .send(Box::new(|s| {
                    if let Some(mut view) = s.find_name::<ResizedView<TextView>>("working_textview")
                    {
                        view.get_inner_mut().set_content("");
                    } else {
                        panic!("working_textview view not found");
                    }
                }))
                .unwrap();
        });
    }
}

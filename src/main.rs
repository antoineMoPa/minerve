use cursive::event::EventResult;
use cursive::theme::ColorStyle;
use cursive::theme::{
    BaseColor::{self, *},
    Color, Palette, PaletteColor, Theme,
};
use cursive::traits::*;
use cursive::utils::markup::StyledString;
use cursive::views::{
    Dialog, LinearLayout, NamedView, OnEventView, ResizedView, ScrollView, TextArea, TextView
};
use dotenvy::from_path;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tools::registry::get_tool_registry;

mod tools;

use clap::command;
use clap::Parser;

/// Minerve: A terminal-based assistant with headless support
#[derive(Parser)]
#[command(name = "Minerve")]
#[command(about = "Terminal assistant", long_about = None)]
struct Cli {
    /// Run a one-off prompt without UI
    #[arg(short, long)]
    prompt: Option<String>,
}

const HISTORY_PATH: &str = ".minerve_chat_history.json";
const MODEL_NAME: &str = "gpt-4.1-mini";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatCompletionMessageRole {
    System,
    User,
    Assistant,
    Function,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionMessage {
    pub role: ChatCompletionMessageRole,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call: Option<ChatCompletionFunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionFunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionFunctionDefinition {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatCompletionMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    functions: Option<Vec<ChatCompletionFunctionDefinition>>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatCompletionChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChoice {
    message: ChatCompletionMessage,
}

fn custom_theme() -> Theme {
    let mut palette = Palette::default();

    palette[PaletteColor::Background] = Color::Dark(Black);
    palette[PaletteColor::View] = Color::Dark(Black);
    palette[PaletteColor::Primary] = Color::Dark(White);
    palette[PaletteColor::TitlePrimary] = Color::Dark(Cyan);
    palette[PaletteColor::Highlight] = Color::Dark(Black);
    palette[PaletteColor::HighlightText] = Color::Light(White);
    palette[PaletteColor::Secondary] = Color::Light(White);

    Theme {
        palette,
        ..Theme::default()
    }
}

/// Holds app state and the OpenAI runtime
use std::sync::atomic::AtomicBool;

struct Minerve {
    messages: Arc<Mutex<Vec<ChatCompletionMessage>>>,
    rt: Runtime,
    client: Client,
    api_key: String,
    base_url: String,
    request_in_flight: Arc<AtomicBool>,
}

pub async fn handle_function_call(
    function_call: &ChatCompletionFunctionCall,
) -> ChatCompletionMessage {
    let registry = get_tool_registry();
    let function_name = &function_call.name;
    let args_str = &function_call.arguments;

    if let Some(tool) = registry.get(function_name.as_str()) {
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
        let result = tool.run(args).await;

        ChatCompletionMessage {
            role: ChatCompletionMessageRole::Function,
            content: Some(result),
            name: Some(function_name.clone()),
            function_call: None,
            tool_call_id: Some(function_call.name.clone()),
            tool_calls: None,
        }
    } else {
        ChatCompletionMessage {
            role: ChatCompletionMessageRole::Function,
            content: Some(format!("Error: Function '{}' not found", function_name)),
            name: Some(function_name.clone()),
            function_call: None,
            tool_call_id: None,
            tool_calls: None,
        }
    }
}

impl Minerve {
    fn new() -> Self {
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

    fn chat(&self, user_input: String, cb_sink: cursive::CbSink) {
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
                if let Some(mut view) = s.find_name::<ResizedView<TextView>>("working_panel") {
                    view.get_inner_mut().set_content("working...");
                } else {
                    panic!("working_panel view not found");
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

                // Prepare history with cleaned older function outputs
                let history_len = history.len();
                let mut cleaned_history = history.clone();
                if history_len > 10 {
                    for i in 0..history_len - 10 {
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
                                    let function_message =
                                        handle_function_call(function_call).await;

                                    if function_message.content.is_some() {
                                        let mut msgs = messages_clone.lock().unwrap();
                                        msgs.push(function_message.clone());
                                    }

                                    history.push(function_message);
                                    should_continue = true;
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
                                let mut msgs = messages_clone.lock().unwrap();
                                msgs.push(ChatCompletionMessage {
                                    role: ChatCompletionMessageRole::Assistant,
                                    content: Some(error_msg),
                                    name: None,
                                    function_call: None,
                                    tool_call_id: None,
                                    tool_calls: None,
                                });

                                let ui_messages = msgs
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
                                break;
                            }
                        }
                    }
                    Err(req_err) => {
                        let error_msg = format!("Request Error: {}", req_err);
                        let mut msgs = messages_clone.lock().unwrap();
                        msgs.push(ChatCompletionMessage {
                            role: ChatCompletionMessageRole::Assistant,
                            content: Some(error_msg),
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
                        break;
                    }
                }
            }

            // Hide working indicator on finish
            request_in_flight.store(false, Ordering::SeqCst);
            cb_sink
                .send(Box::new(|s| {
                    if let Some(mut view) = s.find_name::<ResizedView<TextView>>("working_panel") {
                        view.get_inner_mut().set_content("");
                    } else {
                        panic!("working_panel view not found");
                    }
                }))
                .unwrap();
        });
    }
}

fn update_chat_ui(
    cb_sink: cursive::CbSink,
    messages: Vec<(String, String)>,
    request_in_flight: bool,
) {
    const MAX_OUTPUT_LEN: usize = 500;

    cb_sink
        .send(Box::new(move |s| {
            let mut view = s
                .find_name::<TextView>("chat")
                .expect("TextView 'chat' not found");

            let mut styled = StyledString::new();

            // only keep last 10 messages
            let messages: Vec<(String, String)> =
                messages.into_iter().rev().take(10).rev().collect();

            for (role, content) in messages.iter().filter(|(r, _)| r != "system") {
                let (label_style, prefix) = match role.as_str() {
                    "user" => (
                        ColorStyle::new(Color::Dark(BaseColor::Green), Color::TerminalDefault),
                        "You",
                    ),
                    "minerve" => (
                        ColorStyle::new(Color::Dark(BaseColor::Cyan), Color::TerminalDefault),
                        "Minerve",
                    ),
                    _ => (ColorStyle::primary(), role.as_str()),
                };

                styled.append_styled(format!("{}:\n", prefix), label_style);

                // Truncate content if too long
                let truncated_content = if content.len() > MAX_OUTPUT_LEN {
                    format!("{}\n...[truncated]", &content[..MAX_OUTPUT_LEN])
                } else {
                    content.to_string()
                };

                styled.append(format!("{}\n\n", truncated_content));
            }

            view.set_content(styled);

            if let Some(mut scroll_view) = s.find_name::<ScrollView<ResizedView<NamedView<TextView>>>>("chat_scroll") {
                scroll_view.scroll_to_bottom();
            } else {
                panic!("ScrollView 'chat_scroll' not found");
            }

            // Update working indicator visibility
            if let Some(mut view) = s.find_name::<ResizedView<TextView>>("working_panel") {
                if request_in_flight {
                    view.get_inner_mut().set_content("working...");
                } else {
                    view.get_inner_mut().set_content("");
                }
            } else {
                panic!("working_panel view not found");
            }
        }))
        .unwrap();
}


pub fn get_system_prompt() -> String {
    return String::from(
        r#"
const SYSTEM_PROMPT = `
You are **Minerve**, a shell assistant that behaves like a professional software developer.

Guidance:
-  Be proactive at using tools instead of asking.
-  Assume you are somewhere in a repository with files.
-  Confirm your changes worked
 - Example: read the file after editing it.
 - Run cargo check or other compile check tool.
-  Think and act swiftly, like a developper. You have limited tools, but use them effectively.
-  Be curious and explore the environment before asking questions.
-  First thing you should do is likely to use a tool to get context.
-  Remain critical of your tools and evaluate if they work as they are still in development.
-  You may be working on yourself, but the current session still uses old code.
-  Privilege small changes (10 lines) with compile check in-between.

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

struct HistoryTracker {
    previous_prompts: Arc<Mutex<Vec<String>>>,
    index: Option<usize>,
}

impl HistoryTracker {
    fn new() -> Self {
        let mut tracker = Self {
            previous_prompts: Arc::new(Mutex::new(vec![])),
            index: None,
        };
        tracker.load_history();
        tracker
    }

    fn load_history(&mut self) {
        let history_path = dirs::home_dir().unwrap().join(HISTORY_PATH);
        if history_path.exists() {
            let content = std::fs::read_to_string(&history_path).unwrap_or_default();
            let prompts: Vec<String> = serde_json::from_str(&content).unwrap_or_else(|_| vec![]);
            *self.previous_prompts.lock().unwrap() = prompts;
        }
    }

    fn save_history(&self) {
        let history_path = dirs::home_dir().unwrap().join(HISTORY_PATH);
        if let Ok(json) = serde_json::to_string(&*self.previous_prompts.lock().unwrap()) {
            let _ = std::fs::write(history_path, json);
        }
    }

    fn add_prompt(&mut self, prompt: String) {
        {
            let mut prompts = self.previous_prompts.lock().unwrap();
            prompts.push(prompt);
        }
        self.index = None;
        self.save_history();
    }

    fn get_previous_prompt(&mut self) -> Option<String> {
        let prompts = self.previous_prompts.lock().unwrap();

        if prompts.is_empty() {
            return None;
        }

        self.index = match self.index {
            None => Some(prompts.len().saturating_sub(1)),
            Some(0) => Some(0), // stay at the oldest
            Some(i) => Some(i - 1),
        };

        self.index.and_then(|i| prompts.get(i).cloned())
    }

    fn get_next_prompt(&mut self) -> Option<String> {
        let prompts = self.previous_prompts.lock().unwrap();

        if prompts.is_empty() {
            return None;
        }

        match self.index {
            None => Some(String::new()), // already at fresh input
            Some(i) if i + 1 >= prompts.len() => {
                self.index = None;
                Some(String::new()) // move out of history
            }
            Some(i) => {
                self.index = Some(i + 1);
                prompts.get(i + 1).cloned()
            }
        }
    }
}

fn run_headless(prompt: String) {
    let minerve = Minerve::new();
    let rt = Runtime::new().unwrap();

    let system_message = ChatCompletionMessage {
        role: ChatCompletionMessageRole::System,
        content: Some(get_system_prompt()),
        name: None,
        function_call: None,
        tool_call_id: None,
        tool_calls: None,
    };

    let user_message = ChatCompletionMessage {
        role: ChatCompletionMessageRole::User,
        content: Some(prompt.clone()),
        name: None,
        function_call: None,
        tool_call_id: None,
        tool_calls: None,
    };

    let messages: Arc<Mutex<Vec<ChatCompletionMessage>>> =
        Arc::new(Mutex::new(vec![system_message, user_message]));

    rt.block_on(async {
        let messages = messages.lock().unwrap();
        let mut history: Vec<ChatCompletionMessage> = messages.clone();

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
        let client = minerve.client.clone();
        let api_key = minerve.api_key.clone();
        let base_url = minerve.base_url.clone();

        while should_continue {
            should_continue = false;

            if history.len() > 10 {
                for i in 0..history.len().saturating_sub(10) {
                    if let ChatCompletionMessageRole::Function = history[i].role {
                        history[i].content = Some(String::from("[cleaned from history]"));
                    }
                }
            }

            let request = ChatCompletionRequest {
                model: String::from(MODEL_NAME),
                messages: history.clone(),
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

                            // Print assistant response
                            if let Some(content) = &assistant_message.content {
                                println!("{}", content);
                            }

                            // Handle function call if present
                            if let Some(function_call) = &assistant_message.function_call {
                                let function_message = handle_function_call(function_call).await;
                                history.push(function_message);
                                should_continue = true;
                            }
                        }
                        Err(json_err) => {
                            eprintln!("JSON Error: {json_err}");
                            break;
                        }
                    }
                }
                Err(req_err) => {
                    eprintln!("Request Error: {req_err}");
                    break;
                }
            }
        }
    });
}

fn launch_tui() {
    let mut siv = cursive::default();
    siv.set_theme(custom_theme());
    let minerve = Arc::new(Minerve::new());
    let history_tracker = Arc::new(Mutex::new(HistoryTracker::new()));

    let history_tracker_for_submit = history_tracker.clone();

    let submit_button = cursive::views::Button::new("Send (Tab-Enter)", move |s| {
        let content = s
            .call_on_name("input", |view: &mut TextArea| {
                view.get_content().to_string()
            })
            .unwrap();

        if content.trim().is_empty() {
            return;
        }

        history_tracker_for_submit
            .lock()
            .unwrap()
            .add_prompt(content.clone());
        minerve.chat(content, s.cb_sink().clone());

        // Clear input
        s.call_on_name("input", |view: &mut TextArea| view.set_content(""));

        // Select the input for better UX after querying OpenAPI
        s.call_on_name("input", |view: &mut TextArea| {
            view.set_cursor(0);
        });
        s.focus_name("input").unwrap();
    });

    let chat_view = TextView::new("").with_name("chat").full_height();
    use cursive::theme::{BaseColor, Color, ColorStyle};

    let working_panel = TextView::new("")
        .center()
        .style(ColorStyle::new(
            Color::Dark(BaseColor::Magenta),
            Color::TerminalDefault,
        ))
        .fixed_height(3)
        .with_name("working_panel");
    let status_view = TextView::new("").with_name("status");
    let input_view = TextArea::new().with_name("input");
    let history_tracker_for_up = history_tracker.clone();
    let history_tracker_for_down = history_tracker.clone();

    let input_view = OnEventView::new(input_view)
        .on_event_inner(cursive::event::Key::Up, move |s, _e| {
            let mut cursor_position = 0;
            s.call_on_name("input", |view: &mut TextArea| {
                cursor_position = view.cursor();
            });

            if cursor_position > 0 {
                // If the cursor is not at the start, do nothing.
                // Let original handler process:
                return Some(EventResult::Ignored);
            }

            let previous_prompt = history_tracker_for_up
                .lock()
                .unwrap()
                .get_previous_prompt()
                .unwrap_or_default();
            s.call_on_name("input", |view: &mut TextArea| {
                view.set_content(previous_prompt)
            });

            return Some(EventResult::consumed());
        })
        .on_event_inner(cursive::event::Key::Down, move |s, _e| {
            let next_prompt = history_tracker_for_down
                .lock()
                .unwrap()
                .get_next_prompt()
                .unwrap_or_default();
            s.call_on_name("input", |view: &mut TextArea| view.set_content(next_prompt));

            return Some(EventResult::consumed());
        })
        .on_event_inner(cursive::event::Event::CtrlChar('a'), |s, _e| {
            s.call_on_name("input", |view: &mut TextArea| view.set_cursor(0));
            return Some(EventResult::consumed());
        })
        .on_event_inner(cursive::event::Event::CtrlChar('e'), |s, _e| {
            s.call_on_name("input", |view: &mut TextArea| {
                view.set_cursor(view.get_content().len())
            });
            return Some(EventResult::consumed());
        })
        .on_event_inner(cursive::event::Event::CtrlChar('k'), |s, _e| {
            s.call_on_name("input", |view: &mut TextArea| {
                view.set_content("");
            });
            return Some(EventResult::consumed());
        });

    let scroll_chat_view = ScrollView::new(chat_view)
        .scroll_strategy(cursive::view::ScrollStrategy::StickToBottom)
        .with_name("chat_scroll");

    siv.add_fullscreen_layer(
        Dialog::around(
            LinearLayout::vertical()
                .child(scroll_chat_view)
                .child(working_panel)
                .child(status_view)
                .child(input_view.full_width())
                .child(submit_button),
        )
        .title("minerve"),
    );

    siv.run();
}

use std::fs::OpenOptions;
use std::io::Write;

fn main() {
    // Open panic.log file for appending
    let panic_log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("panic.log")
        .expect("Failed to open panic.log");

    let panic_log_file = std::sync::Mutex::new(panic_log_file);

    std::panic::set_hook(Box::new(move |panic_info| {
        let mut file = panic_log_file.lock().unwrap();
        let msg = match panic_info.payload().downcast_ref::<&str>() {
            Some(s) => *s,
            None => match panic_info.payload().downcast_ref::<String>() {
                Some(s) => &s[..],
                None => "Unknown panic payload",
            },
        };

        let location = if let Some(location) = panic_info.location() {
            format!("{}:{}", location.file(), location.line())
        } else {
            "Unknown location".to_string()
        };

        let log_message = format!("Panic occurred at {}: {}\n", location, msg);

        let _ = file.write_all(log_message.as_bytes());
    }));

    let cli = Cli::parse();

    if let Some(prompt) = cli.prompt {
        run_headless(prompt);
        return;
    }

    // Otherwise, launch full TUI
    launch_tui();
}

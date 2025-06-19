use cursive::event::EventResult;
use cursive::theme::ColorStyle;
use cursive::traits::*;
use cursive::utils::markup::StyledString;
use cursive::views::{Dialog, LinearLayout, OnEventView, ScrollView, TextArea, TextView};
use dotenvy::from_path;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tools::registry::get_tool_registry;
use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use cursive::theme::{BaseColor::{self, *}, Color, Palette, PaletteColor, Theme};

mod tools;

use clap::Parser;
use clap::command;

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
struct Minerve {
    messages: Arc<Mutex<Vec<ChatCompletionMessage>>>,
    rt: Runtime,
    client: Client,
    api_key: String,
    base_url: String,
}

pub async fn handle_function_call(function_call: &ChatCompletionFunctionCall) -> ChatCompletionMessage {
    let registry = get_tool_registry();
    let function_name = &function_call.name;
    let args_str = &function_call.arguments;

    if let Some(tool) = registry.get(function_name.as_str()) {
        let args: HashMap<String, String> = serde_json::from_str(args_str).unwrap_or_default();
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
        }
    }

    async fn create_chat_completion(&self, messages: Vec<ChatCompletionMessage>, functions: Vec<ChatCompletionFunctionDefinition>) -> Result<ChatCompletionResponse, reqwest::Error> {
        let request = ChatCompletionRequest {
            model: "gpt-4o".to_string(),
            messages,
            functions: if functions.is_empty() { None } else { Some(functions) },
        };

        let url = format!("{}/chat/completions", self.base_url);
        
        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        response.json::<ChatCompletionResponse>().await
    }

    fn chat(&self, user_input: String, cb_sink: cursive::CbSink) {
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

        let ui_messages = msgs.iter().map(|msg| {
            let role = match msg.role {
                ChatCompletionMessageRole::System => "system".to_string(),
                ChatCompletionMessageRole::User => "user".to_string(),
                ChatCompletionMessageRole::Assistant => "minerve".to_string(),
                ChatCompletionMessageRole::Function => "function".to_string(),
                _ => "unknown".to_string(),
            };
            (role, msg.content.clone().unwrap_or_default())
        }).collect();

        update_chat_ui(cb_sink.clone(), ui_messages);

        let messages = msgs.clone();
        drop(msgs); // unlock before async

        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let messages_clone = self.messages.clone();

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

                let request = ChatCompletionRequest {
                    model: "gpt-4o".to_string(),
                    messages: history.clone(),
                    functions: if functions.is_empty() { None } else { Some(functions.clone()) },
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
                                    let function_message = handle_function_call(function_call).await;

                                    if function_message.content.is_some() {
                                        let mut msgs = messages_clone.lock().unwrap();
                                        msgs.push(function_message.clone());
                                    }

                                    history.push(function_message);
                                    should_continue = true;
                                }

                                let ui_messages = messages_clone.lock().unwrap().iter().map(|msg| {
                                    let role = match msg.role {
                                        ChatCompletionMessageRole::System => "system".to_string(),
                                        ChatCompletionMessageRole::User => "user".to_string(),
                                        ChatCompletionMessageRole::Assistant => "minerve".to_string(),
                                        ChatCompletionMessageRole::Function => "function".to_string(),
                                    };
                                    (role, msg.content.clone().unwrap_or_default())
                                }).collect();

                                update_chat_ui(cb_sink.clone(), ui_messages);
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

                                let ui_messages = msgs.iter().map(|msg| {
                                    let role = match msg.role {
                                        ChatCompletionMessageRole::System => "system".to_string(),
                                        ChatCompletionMessageRole::User => "user".to_string(),
                                        ChatCompletionMessageRole::Assistant => "minerve".to_string(),
                                        ChatCompletionMessageRole::Function => "function".to_string(),
                                    };
                                    (role, msg.content.clone().unwrap_or_default())
                                }).collect();

                                update_chat_ui(cb_sink.clone(), ui_messages);
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

                        let ui_messages = msgs.iter().map(|msg| {
                            let role = match msg.role {
                                ChatCompletionMessageRole::System => "system".to_string(),
                                ChatCompletionMessageRole::User => "user".to_string(),
                                ChatCompletionMessageRole::Assistant => "minerve".to_string(),
                                ChatCompletionMessageRole::Function => "function".to_string(),
                            };
                            (role, msg.content.clone().unwrap_or_default())
                        }).collect();

                        update_chat_ui(cb_sink.clone(), ui_messages);
                        break;
                    }
                }
            }
        });
    }
}

fn update_chat_ui(cb_sink: cursive::CbSink, messages: Vec<(String, String)>) {
    cb_sink.send(Box::new(move |s| {
        let mut view = s
            .find_name::<TextView>("chat")
            .expect("TextView 'chat' not found");

        let mut styled = StyledString::new();

        // can be useful to know the sys prompt.
        //for (role, content) in messages.iter().filter(|(r, _)| r != "system") {
        for (role, content) in messages.iter() {
            let (label_style, prefix) = match role.as_str() {
                "user" => (ColorStyle::new(Color::Dark(BaseColor::Green), Color::TerminalDefault), "You"),
                "minerve" => (ColorStyle::new(Color::Dark(BaseColor::Cyan), Color::TerminalDefault), "Minerve"),
                "function" => (ColorStyle::new(Color::Dark(BaseColor::Yellow), Color::TerminalDefault), "Function"),
                _ => (ColorStyle::primary(), role.as_str()),
            };

            styled.append_styled(format!("{}:\n", prefix), label_style);
            styled.append(format!("{}\n\n", content));
        }

        view.set_content(styled);
    })).unwrap();
}

pub fn get_system_prompt() -> String {
    return String::from(r#"
const SYSTEM_PROMPT = `
You are **Minerve**, a shell assistant that behaves like a professional software developer.

Guidance:
1. Be proactive at using tools instead of asking.
2. Assume you are somewhere in a repository with files.
3. Confirm your changes worked
- Example: read the file after editing it.
4. Think and act swiftly, like a developper. You have limited tools, but use them effectively.
5. Be curious and explore the environment before asking questions.
6. First thing you should do is likely to use a tool to get context.

Dont's:

Don't answer stuff like "I'm sorry for the confusion, but as an AI, I don't have the ability to directly modify files or write code to your project. I can provide guidance and code snippets, but you'll need to implement the changes in your project."

  - Instead, directly use the tools available to you to help the user with their coding tasks.

Don't answer stuff like "Sure, I can help with that. However, I need to know the file you want to get the code listing from. Could you please provide the file path?".

 - Instead use the tools available to you to explore the environment and find the file.

Don't answer stuff like "Now you can implement the bingBong function to get a file code listing with line numbers.   - Instead, go and implement that new function.

Don't ask questions that can be figured out from prompt, context or by using the tools available to you, like "Now, could you please specify which file you want to add the tool to?"
 - Instead, figure out yourself.

Don't say "I read file XYZ". just read it directly with the tools.
`;

Immediately use the tools available after stating your intention.

Example good chats:

Q: What is this repo about?

A: <tool name="get_general_context"></tool>

Tool: ...

A: This appears to be a repo about XYZ.

"#);
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

    let messages: Arc<Mutex<Vec<ChatCompletionMessage>>> = Arc::new(Mutex::new(vec![
        system_message,
        user_message,
    ]));

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

            let request = ChatCompletionRequest {
                model: "gpt-4o".to_string(),
                messages: history.clone(),
                functions: if functions.is_empty() { None } else { Some(functions.clone()) },
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

    let submit_button = cursive::views::Button::new("Send (Tab-Enter)", move|s| {
        let content = s
            .call_on_name("input", |view: &mut TextArea| view.get_content().to_string())
            .unwrap();

        if content.trim().is_empty() {
            return;
        }

        history_tracker_for_submit.lock().unwrap().add_prompt(content.clone());
        minerve.chat(content, s.cb_sink().clone());

        // Clear input
        s.call_on_name("input", |view: &mut TextArea| view.set_content(""));

        s.focus_name("input").unwrap();
    });

    let chat_view = TextView::new("").with_name("chat").full_height();
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
            s.call_on_name("input", |view: &mut TextArea| view.set_content(previous_prompt));

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
            s.call_on_name("input", |view: &mut TextArea| view.set_cursor(view.get_content().len()));
            return Some(EventResult::consumed());
        })
        .on_event_inner(cursive::event::Event::CtrlChar('k'), |s, _e| {
            s.call_on_name("input", |view: &mut TextArea| { view.set_content(""); });
            return Some(EventResult::consumed());
        });


    let scroll_chat_view = ScrollView::new(chat_view)
        .scroll_strategy(cursive::view::ScrollStrategy::StickToBottom)
        .with_name("chat_scroll");

    siv.add_fullscreen_layer(
        Dialog::around(
            LinearLayout::vertical()
                .child(scroll_chat_view)
                .child(input_view.full_width())
                .child(submit_button)

        ).title("minerve"),
    );

    siv.run();
}

fn main() {
    let cli = Cli::parse();

    if let Some(prompt) = cli.prompt {
        run_headless(prompt);
        return;
    }

    // Otherwise, launch full TUI
    launch_tui();

}

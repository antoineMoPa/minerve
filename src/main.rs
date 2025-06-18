use cursive::theme::ColorStyle;
use cursive::traits::*;
use cursive::utils::markup::StyledString;
use cursive::views::{Dialog, LinearLayout, ScrollView, TextArea, TextView};
use dotenvy::from_path;
use openai::chat::{ChatCompletion, ChatCompletionMessage, ChatCompletionMessageRole};
use openai::Credentials;
use regex::Regex;
use tools::registry::get_tool_registry;
use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use cursive::theme::{BaseColor::{self, *}, Color, Palette, PaletteColor, Theme};
use std::fmt::Write;

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
    messages: Arc<Mutex<Vec<(String, String)>>>,
    rt: Runtime,
    credentials: Credentials,
}

pub async fn handle_tool_call(input: &str) -> Option<String> {
    let registry = get_tool_registry();

    if let Some(caps) = Regex::new(r#"(?s)<tool name="(.*?)">(.*?)</tool>"#).unwrap()
.captures(input) {
        let name = caps.get(1).unwrap().as_str();
        let args_json = caps.get(2).unwrap().as_str();

        if let Some(tool) = registry.get(name) {
            let args: HashMap<String, String> = serde_json::from_str(args_json).unwrap_or_default();
            let result = tool.run(args).await;
            return Some(result);
        }
    }
    None
}

impl Minerve {
    fn new() -> Self {
        // - Open AI -
        if let Some(home_dir) = dirs::home_dir() {
            let dotenv_path = home_dir.join(".env");
            if dotenv_path.exists() {
                from_path(&dotenv_path).expect("Failed to load ~/.env");
            }
        }

        let api_key = env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
        let base_url =
            env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1/".into());

        let credentials = Credentials::new(api_key, base_url);

        // - History -
        let history_path = dirs::home_dir().unwrap().join(HISTORY_PATH);
        let prompts = if history_path.exists() {
            let content = std::fs::read_to_string(&history_path).unwrap_or_default();
            serde_json::from_str::<Vec<String>>(&content).unwrap_or_else(|_| vec![])
        } else {
            vec![]
        };

        Self {
            messages: Arc::new(Mutex::new(vec![("system".into(), get_system_prompt())])),
            rt: Runtime::new().unwrap(),
            credentials,
        }
    }

    fn chat(&self, user_input: String, cb_sink: cursive::CbSink) {
        let mut msgs = self.messages.lock().unwrap();
        msgs.push(("user".to_string(), user_input.clone()));

        update_chat_ui(cb_sink.clone(), msgs.clone());

        let messages = msgs.clone();
        drop(msgs); // unlock before async

        let credentials = self.credentials.clone();
        let messages_clone = self.messages.clone();

        self.rt.spawn(async move {
            let history: Vec<ChatCompletionMessage> = messages
                .iter()
                .map(|(role, content)| ChatCompletionMessage {
                    role: match role.as_str() {
                        "system" => ChatCompletionMessageRole::System,
                        "user" => ChatCompletionMessageRole::User,
                        "minerve" => ChatCompletionMessageRole::Assistant,
                        _ => ChatCompletionMessageRole::User,
                    },
                    content: Some(content.clone()),
                    name: None,
                    function_call: None,
                    tool_call_id: None,
                    tool_calls: None,
                })
                .collect();

            let mut tool_result: Option<String> = None;
            let mut should_continue = true;

            while should_continue {
                should_continue = false;
                let mut history = history.clone();

                if let Some(ref result) = tool_result {
                    history.push(ChatCompletionMessage {
                        role: ChatCompletionMessageRole::User, // OR Assistant if tool result is simulated as a response
                        content: Some(format!("Tool output:\n{}", result)),
                        name: None,
                        function_call: None,
                        tool_call_id: None,
                        tool_calls: None,
                    });
                }

                tool_result = None;

                let chat = ChatCompletion::builder("gpt-4o", history)
                    .credentials(credentials.clone())
                    .create()
                    .await;

                let (reply, tool_result) = match chat {
                    Ok(response) => {

                        let r = response
                            .choices
                            .first()
                            .and_then(|c| c.message.content.clone())
                            .unwrap_or("<empty response>".to_string());

                        tool_result = handle_tool_call(&r).await;

                        let debug = format!("{:?}", response);

                        messages_clone
                            .lock()
                            .unwrap()
                            .push(("debug".to_string(), debug));

                        update_chat_ui(cb_sink.clone(), messages_clone.lock().unwrap().clone());

                        (r, tool_result.clone())
                    },
                    Err(err) => (format!("Error: {}", err), None),
                };

                messages_clone
                    .lock()
                    .unwrap()
                    .push(("minerve".to_string(), reply.clone()));

                match tool_result {
                    Some(r) => {
                        messages_clone
                            .lock()
                            .unwrap()
                            .push(("tool".to_string(), r));

                        update_chat_ui(cb_sink.clone(), messages_clone.lock().unwrap().clone());

                        should_continue = true;
                    },
                    None => {}
                }

                update_chat_ui(cb_sink.clone(), messages_clone.lock().unwrap().clone());
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
                _ => (ColorStyle::primary(), role.as_str()),
            };

            styled.append_styled(format!("{}:\n", prefix), label_style);
            styled.append(format!("{}\n\n", content));
        }

        view.set_content(styled);
    })).unwrap();
}

pub fn get_system_prompt() -> String {
    let registry = get_tool_registry(); // Returns HashMap<&str, Arc<dyn Tool>>

    let mut tools_description = String::from("You can use the following tools:\n");

    let mut tool_descriptions: Vec<String> = Vec::new();

    for tool in registry.values() {
        let mut param_string = String::new();

        for (key, ty) in tool.parameters() {
            let _ = write!(&mut param_string, "{}: {}, ", key, ty);
        }

        // Trim trailing comma and space, if any
        if param_string.ends_with(", ") {
            param_string.truncate(param_string.len() - 2);
        }

        let description = format!(
            "- {}: {}. Params: {}",
            tool.name(),
            tool.description(),
            if param_string.is_empty() { "none".to_string() } else { param_string }
        );

        tool_descriptions.push(description);
    }

    tools_description.push_str(&tool_descriptions.join("\n"));

    format!(
        r#"
const SYSTEM_PROMPT = `
You are **Minerve**, a shell assistant that behaves like a professional software developer.


## 🔧 TOOL SYNTAX

<tool name="TOOL_NAME">
{{
  "param1": "value",
  "param2": "value"
}}
</tool>

❗️ **DO NOT use** incorrect syntax like:
- \`<tool_name>...</tool_name>\` ❌
- \`<toolName="...">...</tool>\` ❌
- Missing or malformed JSON ❌

## 🧰 AVAILABLE TOOLS

{}

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

"#,
        tools_description
    )
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

    let messages: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(vec![
        ("system".into(), get_system_prompt()),
        ("user".into(), prompt.clone()),
    ]));

    rt.block_on(async {
        let messages = messages.lock().unwrap();
        let history: Vec<ChatCompletionMessage> = messages
                .iter()
                .map(|(role, content)| ChatCompletionMessage {
                    role: match role.as_str() {
                        "system" => ChatCompletionMessageRole::System,
                        "user" => ChatCompletionMessageRole::User,
                        "minerve" => ChatCompletionMessageRole::Assistant,
                        _ => ChatCompletionMessageRole::User,
                    },
                    content: Some(content.clone()),
                    name: None,
                    function_call: None,
                    tool_call_id: None,
                    tool_calls: None,
                })
                .collect();

        let mut should_continue = true;
        let mut tool_result: Option<String> = None;
        let credentials = minerve.credentials.clone();

        while should_continue {
            should_continue = false;

            let mut full_history = history.clone();

            if let Some(result) = &tool_result {
                full_history.push(ChatCompletionMessage {
                    role: ChatCompletionMessageRole::User,
                    content: Some(format!("Tool output:\n{}", result)),
                    name: None,
                    function_call: None,
                    tool_call_id: None,
                    tool_calls: None,
                });
            }

            let chat = ChatCompletion::builder("gpt-4o", full_history)
                .credentials(credentials.clone())
                .create()
                .await;

            match chat {
                Ok(response) => {
                    let reply = response.choices.first()
                        .and_then(|c| c.message.content.clone())
                        .unwrap_or("<empty response>".to_string());

                    println!("{}", reply);
                    tool_result = handle_tool_call(&reply).await;

                    if tool_result.is_some() {
                        should_continue = true;
                    }
                }
                Err(e) => {
                    eprintln!("Error: {e}");
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
    let scroll_chat_view = ScrollView::new(chat_view).scroll_strategy(cursive::view::ScrollStrategy::StickToBottom).with_name("chat_scroll");

    siv.add_fullscreen_layer(
        Dialog::around(
            LinearLayout::vertical()
                .child(scroll_chat_view)
                .child(input_view.full_width())
                .child(submit_button)

        ).title("minerve"),
    );

    let history_tracker_for_up = history_tracker.clone();

    siv.add_global_callback(cursive::event::Event::Key(cursive::event::Key::Up), move |s| {
        let previous_prompt = history_tracker_for_up.lock().unwrap().get_previous_prompt().unwrap_or_default();
        s.call_on_name("input", |view: &mut TextArea| view.set_content(previous_prompt));
    });

    let history_tracker_for_down = history_tracker.clone();

    siv.add_global_callback(cursive::event::Event::Key(cursive::event::Key::Down), move |s| {
        let next_prompt = history_tracker_for_down.lock().unwrap().get_next_prompt().unwrap_or_default();
        s.call_on_name("input", |view: &mut TextArea| view.set_content(next_prompt));
    });

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

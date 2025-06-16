use cursive::theme::ColorStyle;
use cursive::traits::*;
use cursive::utils::markup::StyledString;
use cursive::views::{Dialog, LinearLayout, TextArea, TextView};
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

        Self {
            messages: Arc::new(Mutex::new(vec![("system".into(), get_system_prompt())])),
            rt: Runtime::new().unwrap(),
            credentials,
        }
    }

    fn chat(&self, user_input: String, cb_sink: cursive::CbSink) {
        let mut msgs = self.messages.lock().unwrap();
        msgs.push(("user".to_string(), user_input.clone()));

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

                let chat = ChatCompletion::builder("gpt-4", history)
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

                        should_continue = true;
                    },
                    None => {}
                }
            }


            cb_sink
                .send(Box::new(move |s| {
                    let mut view = s
                        .find_name::<TextView>("chat")
                        .expect("TextView 'chat' not found");
                    let all_msgs = messages_clone.lock().unwrap();

                    let mut styled = StyledString::new();

                    for (role, content) in all_msgs.iter().filter(|(r, _)| r != "system") {
                        let (label_style, prefix) = match role.as_str() {
                            "user" => (ColorStyle::new(Color::Dark(BaseColor::Green), Color::TerminalDefault), "You"),
                            "minerve" => (ColorStyle::new(Color::Dark(BaseColor::Cyan), Color::TerminalDefault), "Minerve"),
                            _ => (ColorStyle::primary(), role.as_str()),
                        };

                        styled.append_styled(format!("{}:\n", prefix), label_style);
                        styled.append(format!("{}\n\n", content));
                    }

                    view.set_content(styled);
                }))
                .unwrap();
        });
    }
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
You are minerve, a shell assistant that acts like a pro software developper.

To use a tool, respond with:

<tool name="TOOL_NAME">
{{
"param1": "value",
"param2": "value"
}}
</tool>

You can also use the following tools:

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
`;

"#,
        tools_description
    )
}

fn main() {
    let mut siv = cursive::default();
    siv.set_theme(custom_theme());
    let minerve = Arc::new(Minerve::new());

    let submit_button = cursive::views::Button::new("Send (Ctrl+L or Tab-Enter)", move |s| {
        let content = s
            .call_on_name("input", |view: &mut TextArea| view.get_content().to_string())
            .unwrap();

        if content.trim().is_empty() {
            return;
        }

        minerve.chat(content, s.cb_sink().clone());

        // Clear input
        s.call_on_name("input", |view: &mut TextArea| view.set_content(""));

        s.focus_name("input").unwrap();
    });

    let chat_view = TextView::new("").with_name("chat").scrollable();
    let input_view = TextArea::new().with_name("input");

    siv.add_fullscreen_layer(
        Dialog::around(
            LinearLayout::vertical()
                .child(chat_view.full_height().scrollable())
                .child(input_view.full_width())
                .child(submit_button)

        ).title("minerve"),
    );

    let minerve = Arc::new(Minerve::new());

    siv.add_global_callback(cursive::event::Event::CtrlChar('l'), move |s| {
        let content = s
            .call_on_name("input", |v: &mut TextArea| v.get_content().to_string())
            .unwrap();

        if content.trim().is_empty() {
            return;
        }

        minerve.chat(content, s.cb_sink().clone());

        s.call_on_name("input", |v: &mut TextArea| v.set_content(""));

        s.focus_name("input").unwrap();
    });

    siv.run();
}

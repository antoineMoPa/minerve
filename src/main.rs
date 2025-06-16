use cursive::traits::*;
use cursive::views::{Dialog, LinearLayout, TextArea, TextView};
use dotenvy::from_path;
use openai::chat::{ChatCompletion, ChatCompletionMessage, ChatCompletionMessageRole};
use openai::Credentials;
use std::env;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use cursive::theme::{BaseColor::*, Color, Palette, PaletteColor, Theme};

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
struct Assistant {
    messages: Arc<Mutex<Vec<(String, String)>>>,
    rt: Runtime,
    credentials: Credentials,
}

impl Assistant {
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
                        "assistant" => ChatCompletionMessageRole::Assistant,
                        _ => ChatCompletionMessageRole::User,
                    },
                    content: Some(content.clone()),
                    name: None,
                    function_call: None,
                    tool_call_id: None,
                    tool_calls: None,
                })
                .collect();

            let chat = ChatCompletion::builder("gpt-4", history)
                .credentials(credentials)
                .create()
                .await;

            let reply = match chat {
                Ok(response) => response
                    .choices
                    .first()
                    .and_then(|c| c.message.content.clone())
                    .unwrap_or("<empty response>".to_string()),
                Err(err) => format!("Error: {}", err),
            };

            messages_clone
                .lock()
                .unwrap()
                .push(("assistant".to_string(), reply.clone()));

            cb_sink
                .send(Box::new(move |s| {
                    let mut view = s
                        .find_name::<TextView>("chat")
                        .expect("TextView 'chat' not found");
                    let all_msgs = messages_clone.lock().unwrap();

                    let text = all_msgs
                        .iter()
                        .filter(|(r, _)| r != "system")
                        .map(|(r, c)| format!("{}:\n{}\n\n", r, c))
                        .collect::<String>();

                    view.set_content(text);
                }))
                .unwrap();
        });
    }
}

fn get_system_prompt() -> String {
    r#"
You are a shell assistant that acts like a pro software developer.
Use tools effectively and act swiftly.
"#
    .trim()
    .to_string()
}

fn main() {
    let mut siv = cursive::default();
    siv.set_theme(custom_theme());
    let assistant = Arc::new(Assistant::new());

    let submit_button = cursive::views::Button::new("Send (Ctrl+L)", move |s| {
        let content = s
            .call_on_name("input", |view: &mut TextArea| view.get_content().to_string())
            .unwrap();

        if content.trim().is_empty() {
            return;
        }

        assistant.chat(content, s.cb_sink().clone());

        // Clear input
        s.call_on_name("input", |view: &mut TextArea| view.set_content(""));
    });

    let chat_view = TextView::new("").with_name("chat").scrollable();
    let input_view = TextArea::new().with_name("input");

    siv.add_fullscreen_layer(
        Dialog::around(
            LinearLayout::vertical()
                .child(chat_view.full_height().scrollable())
                .child(input_view.full_width())
                .child(submit_button)

        ).title("assistant"),
    );

    let assistant = Arc::new(Assistant::new());

    siv.add_global_callback(cursive::event::Event::CtrlChar('l'), move |s| {
        let content = s
            .call_on_name("input", |v: &mut TextArea| v.get_content().to_string())
            .unwrap();

        if content.trim().is_empty() {
            return;
        }

        assistant.chat(content, s.cb_sink().clone());

        s.call_on_name("input", |v: &mut TextArea| v.set_content(""));
    });

    siv.run();
}

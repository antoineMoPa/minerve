use cursive::event::EventResult;
use cursive::theme::{BaseColor, Color, ColorStyle};
use cursive::traits::*;
use cursive::utils::markup::StyledString;
use cursive::view::scroll::Scroller;
use cursive::views::{
    Dialog, LinearLayout, NamedView, OnEventView, ResizedView, ScrollView, TextArea, TextView,
};
use history::HistoryTracker;
use minerve::Minerve;
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use theme::custom_theme;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

static GLOBAL_RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub fn get_global_runtime() -> &'static Runtime {
    GLOBAL_RUNTIME.get_or_init(|| Runtime::new().unwrap())
}

mod chat;
mod history;
mod tools;

use chat::*;
use clap::Parser;

mod cli;
use cli::*;

mod utils;

mod minerve;
mod theme;

pub const MODEL_NAME: &str = "gpt-4o";
pub const HISTORY_PATH: &str = ".minerve/history.json";

pub const HISTORY_CUTOFF: usize = 30;

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

            let messages: Vec<(String, String)> = messages
                .into_iter()
                .rev()
                .take(HISTORY_CUTOFF)
                .rev()
                .collect();

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
                    if role != "minerve" {
                        format!("{}\n...[truncated]", &content[..MAX_OUTPUT_LEN])
                    } else {
                        content.trim().to_string()
                    }
                } else {
                    content.to_string()
                };

                styled.append(format!("{}\n\n", truncated_content));
            }

            view.set_content(styled);

            if let Some(mut scroll_view) =
                s.find_name::<ScrollView<ResizedView<NamedView<TextView>>>>("chat_scroll")
            {
                scroll_view.get_scroller_mut().scroll_to_bottom();
            } else {
                panic!("ScrollView 'chat_scroll' not found");
            }

            // Update working indicator visibility
            if let Some(mut view) = s.find_name::<ResizedView<TextView>>("working_textview") {
                if request_in_flight {
                    view.get_inner_mut().set_content("working...");
                } else {
                    view.get_inner_mut().set_content("");
                }
            } else {
                panic!("working_textview view not found");
            }
        }))
        .unwrap();
}

use std::fs::OpenOptions;
use std::io::Write;

pub fn run_headless(prompt: String) -> String {
    get_global_runtime().block_on(run_headless_with_capture(prompt, false, None))
}

use crate::minerve::get_system_prompt;

pub async fn run_headless_with_capture(
    prompt: String,
    capture_output: bool,
    output_sender: Option<mpsc::UnboundedSender<String>>,
) -> String {
    let system_prompt = if prompt.is_empty() {
        get_system_prompt()
    } else {
        prompt
    };

    if !capture_output {
        println!("run_headless started with prompt: {}", system_prompt);
    }

    // Create a Minerve instance to reuse existing initialization logic
    let minerve = Minerve::new();

    let user_message = ChatCompletionMessage {
        role: ChatCompletionMessageRole::User,
        content: Some(system_prompt.clone()),
        name: None,
        function_call: None,
        tool_call_id: None,
        tool_calls: None,
    };

    // Add user message to minerve's messages
    {
        let mut msgs = minerve.messages.lock().unwrap();
        msgs.push(user_message);
    }

    // Use minerve's chat_headless method
    let result = minerve.chat_headless(capture_output, output_sender).await;

    if capture_output {
        result
    } else {
        println!("run_headless completed.");
        String::new()
    }
}

fn launch_tui() {
    let is_headless = false;
    let mut siv = cursive::default();
    siv.set_theme(custom_theme());
    let minerve = Arc::new(Minerve::new());
    let history_tracker = Arc::new(Mutex::new(HistoryTracker::new()));

    let history_tracker_for_submit = history_tracker.clone();

    // Create a channel for receiving streaming output from subminerve processes
    let (stream_tx, mut stream_rx) = mpsc::unbounded_channel::<String>();
    let cb_sink_for_stream = siv.cb_sink().clone();

    // Spawn a task to handle streaming output
    let minerve_for_stream = minerve.clone();
    get_global_runtime().spawn(async move {
        while let Some(output) = stream_rx.recv().await {
            // Add the streaming output as an assistant message to the main chat
            {
                let mut msgs = minerve_for_stream.messages.lock().unwrap();
                msgs.push(ChatCompletionMessage {
                    role: ChatCompletionMessageRole::Assistant,
                    content: Some(format!("[Subminerve] {}", output)),
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

                // Update UI immediately when streaming message is received
                update_chat_ui(cb_sink_for_stream.clone(), ui_messages, false);
            }
        }
    });

    // Store the stream sender in minerve for use by tools
    minerve.set_stream_sender(stream_tx.clone());

    // Also store it globally for tools to access
    crate::tools::registry::set_global_stream_sender(stream_tx);

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
        minerve.chat(content, s.cb_sink().clone(), is_headless);

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

    let working_textview = TextView::new("")
        .center()
        .style(ColorStyle::new(
            Color::Dark(BaseColor::Magenta),
            Color::TerminalDefault,
        ))
        .fixed_height(3)
        .with_name("working_textview");
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
                .child(working_textview)
                .child(status_view)
                .child(input_view.full_width())
                .child(submit_button),
        )
        .title("minerve"),
    );

    siv.run();
}

fn main() {
    // Open panic.log file for appending
    // Create ~/.minerve folder if it doesn't exist
    if let Some(home_dir) = dirs::home_dir() {
        let minerve_dir = home_dir.join(".minerve");
        if !minerve_dir.exists() {
            std::fs::create_dir_all(&minerve_dir).expect("Failed to create ~/.minerve directory");
        }
    }
    // Create ~/.minerve/notes.md if it doesn't exist
    if let Some(home_dir) = dirs::home_dir() {
        let notes_path = home_dir.join(".minerve/notes.md");
        if !notes_path.exists() {
            std::fs::write(&notes_path, "# Notes\n").expect("Failed to create notes.md");
        }
    }

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
        let _ = run_headless(prompt);
        return;
    }

    // Otherwise, launch full TUI
    launch_tui();
}

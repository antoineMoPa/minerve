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
use token_counter::{get_global_token_counter, TokenCounter};
use tokio::runtime::Runtime;

static GLOBAL_RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub fn get_global_runtime() -> &'static Runtime {
    GLOBAL_RUNTIME.get_or_init(|| Runtime::new().unwrap())
}

mod chat;
mod history;
mod token_counter;
mod tools;

use chat::*;
use clap::Parser;

mod cli;
use cli::*;

mod utils;

mod minerve;
mod openrouter;
use openrouter::{read_openrouter_connection_from_env, OpenRouterConnection};
mod theme;

pub const HISTORY_PATH: &str = ".minerve/history.json";
pub const PANIC_LOG_PATH: &str = ".minerve/panic.log";

fn load_home_dotenv() {
    let dotenv_path = dirs::home_dir().expect("no home directory").join(".env");
    if dotenv_path.exists() {
        dotenvy::from_path(&dotenv_path).expect("Failed to load ~/.env");
    }
}

fn update_chat_ui(
    cb_sink: cursive::CbSink,
    messages: Vec<(String, String)>,
    request_in_flight: bool,
    token_counter: Arc<TokenCounter>,
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

            s.call_on_name("token_count", |view: &mut TextView| {
                view.set_content(token_counter.format_usage_summary());
            });
        }))
        .unwrap();
}

use std::fs::OpenOptions;
use std::io::Write;

pub fn run_headless(prompt: String, openrouter_connection: OpenRouterConnection) -> String {
    get_global_runtime().block_on(run_headless_with_capture(
        prompt,
        openrouter_connection,
        false,
    ))
}

pub async fn run_headless_with_capture(
    prompt: String,
    openrouter_connection: OpenRouterConnection,
    capture_output: bool,
) -> String {
    if !capture_output {
        println!("run_headless started with prompt: {}", prompt);
    }

    // Create a Minerve instance to reuse existing initialization logic
    let minerve = Minerve::new(openrouter_connection);

    let user_message = ChatCompletionMessage::user(prompt.clone());

    // Add user message to minerve's messages
    {
        let mut msgs = minerve.messages.lock().unwrap();
        msgs.push(user_message);
    }

    // Use minerve's chat_headless method
    let result = minerve.chat_headless(capture_output).await;

    if capture_output {
        result
    } else {
        println!("{}", minerve.token_counter.format_usage_summary());
        println!("run_headless completed.");
        String::new()
    }
}

fn launch_tui(openrouter_connection: OpenRouterConnection) {
    let is_headless = false;
    let mut siv = cursive::default();
    siv.set_theme(custom_theme());
    let minerve = Arc::new(Minerve::new(openrouter_connection));
    let history_tracker = Arc::new(Mutex::new(HistoryTracker::new()));
    let token_counter_ref = get_global_token_counter();

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

        // Increment sent tokens count
        history_tracker_for_submit
            .lock()
            .unwrap()
            .add_prompt(content.clone());
        minerve
            .clone()
            .chat_with_arc(content, s.cb_sink().clone(), is_headless);

        // Clear input
        s.call_on_name("input", |view: &mut TextArea| view.set_content(""));

        // Select the input for better UX after querying OpenRouter
        s.call_on_name("input", |view: &mut TextArea| {
            view.set_cursor(0);
        });
        s.focus_name("input").unwrap();
    });

    let chat_view = TextView::new("").with_name("chat").full_height();
    let token_count_view = TextView::new(token_counter_ref.format_usage_summary())
        .with_name("token_count")
        .fixed_height(1);

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
                .child(token_count_view)
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

    let panic_log_path = dirs::home_dir()
        .expect("no home directory")
        .join(PANIC_LOG_PATH);
    let panic_log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&panic_log_path)
        .unwrap_or_else(|e| panic!("Failed to open {}: {}", panic_log_path.display(), e));

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
        // Inside the TUI this lands on the alternate screen and is wiped, hence the log file.
        eprintln!(
            "minerve: {}\n(at {}, logged to ~/{})",
            msg, location, PANIC_LOG_PATH
        );
    }));

    let cli = Cli::parse();

    // Read configuration before the TUI takes over the terminal, so errors stay visible.
    load_home_dotenv();
    let openrouter_connection = read_openrouter_connection_from_env();

    if let Some(prompt) = cli.prompt {
        let _ = run_headless(prompt, openrouter_connection);
        return;
    }

    launch_tui(openrouter_connection);
}

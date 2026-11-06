use std::sync::OnceLock;

use tokio::runtime::Runtime;

static GLOBAL_RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub fn get_global_runtime() -> &'static Runtime {
    GLOBAL_RUNTIME.get_or_init(|| Runtime::new().unwrap())
}

mod agents_md;
mod boot;
mod chat;
mod chat_selection;
mod goal;
mod control_socket;
mod history;
mod images;
mod input_layout;
mod keys_file;
mod model_lists;
mod model_picker;
mod markdown;
mod mcp;
mod mcp_manager;
mod message_apps;
mod message_apps_runner;
mod pro_tips;
mod recent_models;
mod session_picker;
mod token_counter;
mod tools;
mod tetris;
mod threads_report;
mod tui;
mod ui;

use chat::*;
use clap::Parser;

mod cli;
use cli::*;

mod utils;

mod minerve;
mod anthropic;
mod openrouter;
mod provider;
mod session;
mod setup;
use minerve::Minerve;
use openrouter::{read_openrouter_connection_from_env, OpenRouterConnection};

pub const HISTORY_PATH: &str = ".minerve/history.json";
pub const PANIC_LOG_PATH: &str = ".minerve/panic.log";

fn load_home_dotenv() {
    let dotenv_path = dirs::home_dir().expect("no home directory").join(".env");
    if dotenv_path.exists() {
        dotenvy::from_path(&dotenv_path).expect("Failed to load ~/.env");
    }
}

use std::fs::OpenOptions;
use std::io::Write;

pub fn run_headless(
    prompt: String,
    openrouter_connection: OpenRouterConnection,
    start: session::SessionStart,
    extra_system_prompts: &[String],
) {
    let minerve = Minerve::new(openrouter_connection, start, extra_system_prompts, None);
    minerve
        .messages
        .lock()
        .unwrap()
        .push(ChatCompletionMessage::user(prompt));

    get_global_runtime().block_on(minerve.chat_headless());
    minerve.save_session();

    println!("{}", minerve.token_counter.format_usage_summary());
}

fn launch_tui(
    openrouter_connection: OpenRouterConnection,
    start: session::SessionStart,
    extra_system_prompts: &[String],
    force_setup: bool,
    control: Option<control_socket::ControlSocket>,
) {
    // Slack and the other message apps are served only while this TUI is open.
    message_apps_runner::start_if_configured();
    if let Err(err) = tui::run(openrouter_connection, start, extra_system_prompts, force_setup, control) {
        eprintln!("minerve: terminal UI failed: {}", err);
    }
    message_apps_runner::stop();
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

    if cli.threads {
        threads_report::print(cli.json);
        return;
    }

    // Read configuration before the TUI takes over the terminal, so errors stay visible.
    load_home_dotenv();
    let openrouter_connection = read_openrouter_connection_from_env();

    // Load or check the session before the TUI takes over, so a bad id is reported on stderr.
    // Clap refuses --resume together with --session-id.
    let start = match (cli.resume.as_deref(), cli.session_id) {
        (Some(arg), _) => session::load_for_resume(arg).map(session::SessionStart::Resume),
        (None, Some(id)) => session::check_new_session_id(&id).map(|()| session::SessionStart::NewWithId(id)),
        (None, None) => Ok(session::SessionStart::New),
    };
    let start = match start {
        Ok(start) => start,
        Err(err) => {
            eprintln!("minerve: {}", err);
            std::process::exit(1);
        }
    };

    // Read before the TUI takes over, so a bad config file is reported on stderr.
    match mcp::config::load_mcp_servers() {
        Ok(configs) => mcp::start_connecting(configs),
        Err(err) => {
            eprintln!("minerve: MCP config: {}", err);
            std::process::exit(1);
        }
    }

    if let Some(prompt) = cli.prompt {
        run_headless(prompt, openrouter_connection, start, &cli.system_prompt);
        return;
    }

    // Connected before the TUI takes over, so a parent that named a dead socket is reported on stderr.
    let control = match control_socket::ControlSocket::connect_from_env() {
        Ok(control) => control,
        Err(err) => {
            eprintln!("minerve: {}", err);
            std::process::exit(1);
        }
    };
    launch_tui(openrouter_connection, start, &cli.system_prompt, cli.setup, control);
}

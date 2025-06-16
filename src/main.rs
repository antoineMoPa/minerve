use dotenvy::from_path;
use openai::chat::{ChatCompletion, ChatCompletionMessage, ChatCompletionMessageRole};
use openai::Credentials;
use std::env;
use std::io::{self, Write};
use tokio::runtime::Runtime;

use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph},
    Terminal,
};

use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

struct Assistant {
    messages: Vec<(String, String)>,
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
        let base_url = env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1/".to_string());

        let credentials = Credentials::new(api_key, base_url);

        Self {
            messages: vec![("system".into(), get_system_prompt())],
            rt: Runtime::new().unwrap(),
            credentials,
        }
    }

    fn chat(&mut self, user_input: String) -> String {
        self.messages.push(("user".to_string(), user_input.clone()));

        let credentials = self.credentials.clone();
        let history = self
            .messages
            .iter()
            .map(|(r, c)| ChatCompletionMessage {
                role: match r.as_str() {
                    "system" => ChatCompletionMessageRole::System,
                    "user" => ChatCompletionMessageRole::User,
                    "assistant" => ChatCompletionMessageRole::Assistant,
                    _ => ChatCompletionMessageRole::User,
                },
                content: Some(c.clone()),
                name: None,
                function_call: None,
                tool_call_id: None,
                tool_calls: None,
            })
            .collect::<Vec<_>>();

        let assistant_reply = self.rt.block_on(async {
            let chat = ChatCompletion::builder("gpt-4", history)
                .credentials(credentials)
                .create()
                .await;

            match chat {
                Ok(response) => {
                    response
                        .choices
                        .first()
                        .and_then(|c| c.message.content.clone())
                        .unwrap_or("<empty response>".into())
                }
                Err(e) => format!("Error: {}", e),
            }
        });

        self.messages.push(("assistant".to_string(), assistant_reply.clone()));
        assistant_reply
    }
}

fn get_system_prompt() -> String {
    r#"
You are a shell assistant that acts like a pro software developer.
Use tools effectively and act swiftly.
"#
    .to_string()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut assistant = Assistant::new();
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut input = String::new();
    let mut is_last_key_enter = false;
    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([Constraint::Ratio(80, 100), Constraint::Ratio(20, 100)])
                .split(f.size());

            let chat_text = assistant
                .messages
                .iter()
                .map(|(r, c)| format!("{}:\n{}\n", r, c))
                .collect::<String>();

            let chat = Paragraph::new(chat_text)
                .block(Block::default().borders(Borders::ALL).title("Chat"))
                .style(Style::default().fg(Color::White));
            f.render_widget(chat, chunks[0]);

            let input_paragraph = Paragraph::new(input.clone())
                .block(Block::default().borders(Borders::ALL).title("Your Input"))
                .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

            f.render_widget(input_paragraph, chunks[1]);
        })?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Enter => {
                        if is_last_key_enter {
                            assistant.chat(input.clone());
                            input.clear();
                        } else {
                            input.push('\n');
                        }
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        disable_raw_mode()?;
                        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                        terminal.show_cursor()?;
                        return Ok(());
                    }
                    KeyCode::Char(c) => {
                        input.push(c)
                    }
                    KeyCode::Backspace => {
                        input.pop();
                    }
                    KeyCode::Esc => break,
                    _ => {}
                }
                is_last_key_enter = key.code == KeyCode::Enter;
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

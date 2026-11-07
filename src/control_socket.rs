//! Control socket: structured messages between minerve and the program that started it.
//!
//! A parent that runs minerve in a pty (moon, for one) also wants to know what the agent is
//! doing and to steer its UI, without reading the screen or typing keys. It listens on a Unix
//! socket and names its path in `MINERVE_CONTROL_SOCKET`; minerve connects at startup.
//!
//! Both directions carry one JSON object per line, told apart by `type`.
//!
//! minerve → parent ([`MinerveEvent`]):
//! - `{"type":"hello","version":1,"minerve_version":"0.7.0","features":["serve","cancel","new_session","intro","goal","models","sessions","api_keys","mcp","message_apps","images"],"cwd":"/home/me/project"}`
//!   first, once. `features` names the commands this minerve accepts beyond the original ones, so
//!   a parent sends a newer command only when it is listed (an unknown command stops the reader).
//! - `{"type":"session","session_id":"…"}` after `hello`, and again when `/resume` switches session.
//! - `{"type":"state","state":"working"|"waiting_for_input"}` as a turn starts and ends.
//!   `waiting_for_input` is also sent once after `session`.
//! - `{"type":"message","message":{…}}` for each message added to the conversation: the
//!   user's prompt, the assistant's replies (with their `tool_calls`) and each tool result.
//! - `{"type":"history","session_id":"…","messages":[…]}` in answer to `get_history`.
//!
//! parent → minerve ([`ParentCommand`]):
//! - `{"type":"scroll","to":"top"|"bottom"}`
//! - `{"type":"scroll_by","lines":-5}` (negative goes up, toward older messages)
//! - `{"type":"insert_text","text":"…"}` inserts at the cursor of the input box.
//! - `{"type":"submit","text":"…"}` sends `text` as a prompt. Refused while a turn runs.
//! - `{"type":"get_history"}`
//! - `{"type":"cancel"}` cancels the running turn.
//! - `{"type":"new_session"}` starts a fresh conversation. Refused while a turn runs.
//! - `{"type":"get_intro"}` asks for the `intro` event (answered by `--serve` only).
//!
//! `--serve` also answers these (each listed in `hello.features`):
//! - `{"type":"submit","text":"…","images":["/abs/a.png"]}` (`images`) attaches image files; `[Image #N]` tokens in the text are left as typed.
//! - `{"type":"goal","goal":"…"}` (`goal`) keeps taking turns until an evaluator judges the goal reached.
//! - `{"type":"list_models"}` (`models`) answered by `models`: `{"current":"…","recent":[…],"lists":[{"provider":"OpenRouter","models":[…],"error":null}]}`.
//! - `{"type":"set_model","model":"…"}` (`models`).
//! - `{"type":"list_sessions"}` (`sessions`) answered by `sessions`: `{"sessions":[{"id":"…","modified":0,"first_prompt":"…"}]}`.
//! - `{"type":"resume_session","id":"…"}` (`sessions`) answered by `session` and `transcript`.
//! - `{"type":"get_api_keys"}` / `{"type":"set_api_keys","openrouter":"…"}` (`api_keys`) answered by
//!   `api_keys` (which keys are set, never their values). A key left out is kept.
//!
//! - `{"type":"list_mcp"}` (`mcp`) answered by `mcp_servers`: `{"servers":[{"name","target","source","state","tools","detail"}]}`.
//!   `mcp_connect`, `mcp_disconnect`, `mcp_login`, `mcp_remove` (each `{"name":"…"}`) and
//!   `mcp_add` (`name`, `command_or_url`, `env_or_headers`) change a server, then send `mcp_servers` and a `notice`.
//!   A connection changes in the background: ask again to see it.
//!
//! - `{"type":"message_apps_status"}` (`message_apps`) answered by `message_apps`, one Slack object (see `MessageAppsOut`).
//!   `message_apps_save` (`bot_name`, optional `app_token`, `bot_token`), `message_apps_start`, `message_apps_stop`,
//!   `message_apps_link`, `message_apps_cancel_link` act, then send `message_apps`.
//!   `message_apps_manifest` (`bot_name`) saves the Slack manifest, opens Slack's apps page and answers
//!   `message_apps_manifest` (`manifest`, `url`). The gateway runs only after a start or save, and only in the process that holds its lock.
//!
//! Only `minerve --serve` (the GUI's minerve) also sends these events, so a parent that reads
//! the events above, like moon, never meets an unknown `type`:
//! - `{"type":"transcript","messages":[["user","hi"],…]}` the chat as the TUI shows it, as (role, content).
//! - `{"type":"status","status":"…"}` the status line; empty means idle.
//! - `{"type":"notice","notice":"…"}` a line about local work, not sent to the model.
//! - `{"type":"usage","summary":"…"}` the token and cost summary line.
//! - `{"type":"intro","fps":24,"caption":"…","frames":["…",…],"tips":["…",…]}` in answer to `get_intro`:
//!   the boot clip as plain-text frames (blank = transparent) and the pro tips, the same ones the TUI shows.
//!
//! A command minerve cannot parse is a broken parent: the reader thread panics (logged to
//! the panic log) and stops reading, and the chat goes on.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::ChatCompletionMessage;

pub const CONTROL_SOCKET_ENV_VAR: &str = "MINERVE_CONTROL_SOCKET";

/// Bumped when a message changes in a way an existing parent would misread.
pub const PROTOCOL_VERSION: u32 = 1;

/// Optional abilities, told to the parent in `hello`. Only `--serve` acts on these commands,
/// so the terminal app advertises none.
pub const SERVE_FEATURES: &[&str] = &["serve", "cancel", "new_session", "intro", "goal", "models", "sessions", "api_keys", "mcp", "message_apps", "images"];
pub const TUI_FEATURES: &[&str] = &[];

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Working,
    WaitingForInput,
}

#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MinerveEvent<'a> {
    /// `cwd`: the folder minerve works in, for a window to show.
    Hello { version: u32, minerve_version: &'static str, features: &'static [&'static str], cwd: String },
    Session { session_id: &'a str },
    State { state: AgentState },
    Message { message: &'a ChatCompletionMessage },
    History { session_id: &'a str, messages: &'a [ChatCompletionMessage] },
    // Sent by `--serve` only, see the module docs.
    Transcript { messages: &'a [(String, String)] },
    Status { status: &'a str },
    Notice { notice: &'a str },
    Usage { summary: &'a str },
    Intro { fps: u32, caption: &'a str, frames: &'a [&'static str], tips: &'a [&'static str] },
    Models { current: &'a str, recent: &'a [String], lists: &'a [ModelListOut] },
    Sessions { sessions: &'a [SessionOut] },
    ApiKeys { openrouter: bool, openai: bool, anthropic: bool },
    McpServers { servers: &'a [McpServerOut] },
    MessageApps { slack: &'a MessageAppsOut },
    /// The Slack app manifest, answering `message_apps_manifest`.
    MessageAppsManifest { manifest: &'a str, url: &'static str },
}

/// Slack in `/message-apps`. `state` is not_set_up, disconnected, connecting, connected or failed;
/// `link` is none, waiting or linked.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct MessageAppsOut {
    pub state: &'static str,
    /// The workspace and bot when connected, the error when failed.
    pub detail: String,
    pub bot_name: String,
    pub has_tokens: bool,
    /// Slack members already linked to you.
    pub members: Vec<String>,
    pub link: &'static str,
    /// The one-time code to send the bot, while `link` is waiting; the member once linked.
    pub link_detail: String,
    pub link_seconds_left: u64,
}

/// One MCP server as `/mcp` lists it. `state` is connecting, connected, failed, needs_login,
/// logging_in or disconnected; `detail` is the error, the login URL, or tools left out.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct McpServerOut {
    pub name: String,
    pub target: String,
    pub source: String,
    pub state: &'static str,
    pub tools: usize,
    pub detail: String,
}

/// One provider's models, as `/model` lists them. `error` is set when the provider could not be asked.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct ModelListOut {
    pub provider: &'static str,
    pub models: Vec<String>,
    pub error: Option<String>,
}

/// One saved session, as `/resume` lists them, most recent first.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct SessionOut {
    pub id: String,
    /// Seconds since the Unix epoch.
    pub modified: u64,
    pub first_prompt: String,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScrollTarget {
    Top,
    Bottom,
}

#[derive(Deserialize, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ParentCommand {
    Scroll { to: ScrollTarget },
    ScrollBy { lines: i64 },
    InsertText { text: String },
    Submit {
        text: String,
        /// Image files attached to the prompt (needs the `images` feature).
        #[serde(default)]
        images: Vec<std::path::PathBuf>,
    },
    GetHistory {},
    Cancel {},
    NewSession {},
    GetIntro {},
    Goal { goal: String },
    ListModels {},
    SetModel { model: String },
    ListSessions {},
    ResumeSession { id: String },
    GetApiKeys {},
    MessageAppsStatus {},
    MessageAppsSave { bot_name: String, app_token: Option<String>, bot_token: Option<String> },
    MessageAppsStart {},
    MessageAppsStop {},
    MessageAppsLink {},
    MessageAppsCancelLink {},
    MessageAppsManifest { bot_name: String },
    ListMcp {},
    McpConnect { name: String },
    McpDisconnect { name: String },
    McpLogin { name: String },
    McpRemove { name: String },
    McpAdd { name: String, command_or_url: String, env_or_headers: String },
    SetApiKeys { openrouter: Option<String>, openai: Option<String>, anthropic: Option<String> },
}

pub struct ControlSocket {
    writer: Mutex<UnixStream>,
}

impl ControlSocket {
    /// Connects to the socket the parent named. `None` when minerve was not started with one.
    /// A named socket that cannot be connected to is an error: the parent asked for it.
    pub fn connect_from_env(features: &'static [&'static str]) -> Result<Option<Self>, String> {
        let Some(path) = std::env::var_os(CONTROL_SOCKET_ENV_VAR) else {
            return Ok(None);
        };
        let stream = UnixStream::connect(&path).map_err(|err| {
            format!(
                "could not connect to {}={}: {}",
                CONTROL_SOCKET_ENV_VAR,
                path.to_string_lossy(),
                err
            )
        })?;
        let socket = Self { writer: Mutex::new(stream) };
        socket.send(&MinerveEvent::Hello {
            version: PROTOCOL_VERSION,
            minerve_version: env!("CARGO_PKG_VERSION"),
            features,
            cwd: std::env::current_dir().map(|dir| dir.display().to_string()).unwrap_or_default(),
        });
        Ok(Some(socket))
    }

    /// Writes one event line. A parent that went away is not minerve's problem: the chat
    /// goes on and the event is dropped.
    pub fn send(&self, event: &MinerveEvent) {
        let mut line = serde_json::to_vec(event).expect("control socket events always serialize");
        line.push(b'\n');
        let _ = self.writer.lock().unwrap().write_all(&line);
    }

    /// Reads the parent's commands on a thread of its own, handing each to `on_command`.
    /// Ends when the parent closes the socket.
    /// `on_close` runs once the reader ends.
    pub fn read_commands_in_background(
        &self,
        on_command: impl Fn(ParentCommand) + Send + 'static,
        on_close: impl FnOnce() + Send + 'static,
    ) {
        let reader = self
            .writer
            .lock()
            .unwrap()
            .try_clone()
            .expect("failed to clone the control socket for reading");
        std::thread::spawn(move || {
            // Also runs when a bad line panics this thread, so a parent never waits on a dead reader.
            let _closing = CallOnDrop(Some(on_close));
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                let command: ParentCommand = serde_json::from_str(&line)
                    .unwrap_or_else(|err| panic!("bad control socket command {:?}: {}", line, err));
                on_command(command);
            }
        });
    }
}

struct CallOnDrop<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Drop for CallOnDrop<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_tagged_by_type() {
        let state = serde_json::to_string(&MinerveEvent::State { state: AgentState::WaitingForInput }).unwrap();
        assert_eq!(state, r#"{"type":"state","state":"waiting_for_input"}"#);
        let hello = serde_json::to_string(&MinerveEvent::Hello {
            version: 1,
            minerve_version: env!("CARGO_PKG_VERSION"),
            features: SERVE_FEATURES,
            cwd: "/w".to_string(),
        })
        .unwrap();
        assert_eq!(
            hello,
            format!(
                r#"{{"type":"hello","version":1,"minerve_version":"{}","features":["serve","cancel","new_session","intro","goal","models","sessions","api_keys","mcp","message_apps","images"],"cwd":"/w"}}"#,
                env!("CARGO_PKG_VERSION")
            )
        );
        let intro = serde_json::to_string(&MinerveEvent::Intro { fps: 24, caption: "c", frames: &["f"], tips: &["t"] }).unwrap();
        assert_eq!(intro, r#"{"type":"intro","fps":24,"caption":"c","frames":["f"],"tips":["t"]}"#);
        let messages = [("user".to_string(), "hi".to_string())];
        let transcript = serde_json::to_string(&MinerveEvent::Transcript { messages: &messages }).unwrap();
        assert_eq!(transcript, r#"{"type":"transcript","messages":[["user","hi"]]}"#);
        let status = serde_json::to_string(&MinerveEvent::Status { status: "" }).unwrap();
        assert_eq!(status, r#"{"type":"status","status":""}"#);
        let notice = serde_json::to_string(&MinerveEvent::Notice { notice: "n" }).unwrap();
        assert_eq!(notice, r#"{"type":"notice","notice":"n"}"#);
        let usage = serde_json::to_string(&MinerveEvent::Usage { summary: "s" }).unwrap();
        assert_eq!(usage, r#"{"type":"usage","summary":"s"}"#);
    }

    #[test]
    fn commands_parse_from_their_documented_lines() {
        let parse = |line: &str| serde_json::from_str::<ParentCommand>(line).unwrap();
        assert_eq!(parse(r#"{"type":"scroll","to":"bottom"}"#), ParentCommand::Scroll { to: ScrollTarget::Bottom });
        assert_eq!(parse(r#"{"type":"scroll_by","lines":-5}"#), ParentCommand::ScrollBy { lines: -5 });
        assert_eq!(parse(r#"{"type":"submit","text":"hi"}"#), ParentCommand::Submit { text: "hi".into(), images: Vec::new() });
        assert_eq!(parse(r#"{"type":"get_history"}"#), ParentCommand::GetHistory {});
        assert_eq!(parse(r#"{"type":"cancel"}"#), ParentCommand::Cancel {});
        assert_eq!(parse(r#"{"type":"new_session"}"#), ParentCommand::NewSession {});
        assert_eq!(parse(r#"{"type":"get_intro"}"#), ParentCommand::GetIntro {});
    }

    #[test]
    fn unknown_commands_and_fields_are_refused() {
        assert!(serde_json::from_str::<ParentCommand>(r#"{"type":"dance"}"#).is_err());
        assert!(serde_json::from_str::<ParentCommand>(r#"{"type":"get_history","x":1}"#).is_err());
    }
}

//! Control socket: structured messages between minerve and the program that started it.
//!
//! A parent that runs minerve in a pty (moon, for one) also wants to know what the agent is
//! doing and to steer its UI, without reading the screen or typing keys. It listens on a Unix
//! socket and names its path in `MINERVE_CONTROL_SOCKET`; minerve connects at startup.
//!
//! Both directions carry one JSON object per line, told apart by `type`.
//!
//! minerve → parent ([`MinerveEvent`]):
//! - `{"type":"hello","version":1}` first, once.
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

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Working,
    WaitingForInput,
}

#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MinerveEvent<'a> {
    Hello { version: u32 },
    Session { session_id: &'a str },
    State { state: AgentState },
    Message { message: &'a ChatCompletionMessage },
    History { session_id: &'a str, messages: &'a [ChatCompletionMessage] },
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
    Submit { text: String },
    GetHistory {},
}

pub struct ControlSocket {
    writer: Mutex<UnixStream>,
}

impl ControlSocket {
    /// Connects to the socket the parent named. `None` when minerve was not started with one.
    /// A named socket that cannot be connected to is an error: the parent asked for it.
    pub fn connect_from_env() -> Result<Option<Self>, String> {
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
        socket.send(&MinerveEvent::Hello { version: PROTOCOL_VERSION });
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
    pub fn read_commands_in_background(&self, on_command: impl Fn(ParentCommand) + Send + 'static) {
        let reader = self
            .writer
            .lock()
            .unwrap()
            .try_clone()
            .expect("failed to clone the control socket for reading");
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { return };
                let command: ParentCommand = serde_json::from_str(&line)
                    .unwrap_or_else(|err| panic!("bad control socket command {:?}: {}", line, err));
                on_command(command);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_tagged_by_type() {
        let state = serde_json::to_string(&MinerveEvent::State { state: AgentState::WaitingForInput }).unwrap();
        assert_eq!(state, r#"{"type":"state","state":"waiting_for_input"}"#);
        let hello = serde_json::to_string(&MinerveEvent::Hello { version: 1 }).unwrap();
        assert_eq!(hello, r#"{"type":"hello","version":1}"#);
    }

    #[test]
    fn commands_parse_from_their_documented_lines() {
        let parse = |line: &str| serde_json::from_str::<ParentCommand>(line).unwrap();
        assert_eq!(parse(r#"{"type":"scroll","to":"bottom"}"#), ParentCommand::Scroll { to: ScrollTarget::Bottom });
        assert_eq!(parse(r#"{"type":"scroll_by","lines":-5}"#), ParentCommand::ScrollBy { lines: -5 });
        assert_eq!(parse(r#"{"type":"submit","text":"hi"}"#), ParentCommand::Submit { text: "hi".into() });
        assert_eq!(parse(r#"{"type":"get_history"}"#), ParentCommand::GetHistory {});
    }

    #[test]
    fn unknown_commands_and_fields_are_refused() {
        assert!(serde_json::from_str::<ParentCommand>(r#"{"type":"dance"}"#).is_err());
        assert!(serde_json::from_str::<ParentCommand>(r#"{"type":"get_history","x":1}"#).is_err());
    }
}

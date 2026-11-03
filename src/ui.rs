//! Messages from background work (chat requests, tool calls) to the terminal UI.
//!
//! The UI owns the terminal and runs on the main thread. Anything else talks to it
//! only through a `UiSender`, which keeps the agent code independent of the renderer.

use std::sync::mpsc::Sender;

pub enum UiEvent {
    /// Full chat transcript as (role, content). Replaces what the UI shows.
    Transcript(Vec<(String, String)>),
    /// One status line shown above the input. Empty means idle.
    Status(String),
}

pub type UiSender = Sender<UiEvent>;

pub fn update_chat_ui(ui: &UiSender, messages: Vec<(String, String)>) {
    // A send only fails once the UI has exited, and then nobody is left to show it.
    let _ = ui.send(UiEvent::Transcript(messages));
}

pub fn send_status(ui: &UiSender, status: impl Into<String>) {
    let _ = ui.send(UiEvent::Status(status.into()));
}

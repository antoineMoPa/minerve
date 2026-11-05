//! Provider-neutral types. Transports translate to and from these; nothing else in the
//! gateway knows which chat app a message came from.

use std::fmt;

/// Identifies one conversation on one transport. Maps to one minerve session.
/// `thread` is a Slack thread, a Discord thread, and so on; `None` for a flat chat.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConversationKey {
    pub transport: String,
    pub workspace: String,
    pub channel: String,
    pub thread: Option<String>,
}

impl fmt::Display for ConversationKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.transport, self.workspace, self.channel)?;
        if let Some(thread) = &self.thread {
            write!(f, ":{}", thread)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct InboundMessage {
    pub key: ConversationKey,
    /// Stable id of the sender on that transport (Slack user id, phone number).
    pub sender_id: String,
    pub text: String,
    /// A one-to-one chat with the bot. Link codes count only there, never in a shared channel.
    pub is_direct: bool,
}

/// Handle to a message the gateway sent, so it can be edited later.
#[derive(Debug, Clone)]
pub struct MessageRef {
    pub key: ConversationKey,
    pub id: String,
}

/// What to do with a reply longer than `max_len`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    /// Send it as several messages.
    Chunk,
    /// Send the start of it, and tell the agent it was cut.
    Truncate,
}

/// What a transport can do. The core adapts to this instead of asking which app it is.
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    /// Longest text one message may hold, in chars.
    pub max_len: usize,
    pub can_edit: bool,
    pub overflow: Overflow,
}

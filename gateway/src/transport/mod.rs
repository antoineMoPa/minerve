//! The seam between a chat app and the gateway core.

pub mod console;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::core::types::{Capabilities, ConversationKey, InboundMessage, MessageRef};

pub type TransportError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait]
pub trait Transport: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    /// Owns the connection to the chat app, pushing every message it receives into `inbound`.
    /// Returns only when the connection is gone for good.
    async fn run(&self, inbound: mpsc::Sender<InboundMessage>) -> Result<(), TransportError>;
    async fn send(&self, to: &ConversationKey, text: &str) -> Result<MessageRef, TransportError>;
    /// Replaces the text of a message sent earlier. Only called when `can_edit` is set.
    async fn edit(&self, message: &MessageRef, text: &str) -> Result<(), TransportError>;
}

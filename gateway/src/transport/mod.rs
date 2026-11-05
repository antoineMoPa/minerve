//! The seam between a chat app and the gateway core.

pub mod console;
pub mod slack;
pub mod slack_mrkdwn;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::engine::status::ConnectionReporter;
use crate::engine::types::{Capabilities, ConversationKey, InboundMessage, MessageRef};

pub type TransportError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait]
pub trait Transport: Send + Sync {
    fn name(&self) -> &'static str;
    /// How the agent is told where it runs: "You are running in <display name>."
    fn display_name(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    /// Converts the markdown minerve writes into the app's own syntax. Applied to each chunk.
    fn format(&self, text: &str) -> String {
        text.to_string()
    }
    /// Owns the connection to the chat app, pushing every message it receives into `inbound`.
    /// Reports its connection state through `reporter`. Returns only when the connection is gone for good.
    async fn run(&self, inbound: mpsc::Sender<InboundMessage>, reporter: ConnectionReporter) -> Result<(), TransportError>;
    async fn send(&self, to: &ConversationKey, text: &str) -> Result<MessageRef, TransportError>;
    /// Replaces the text of a message sent earlier. Only called when `can_edit` is set.
    async fn edit(&self, message: &MessageRef, text: &str) -> Result<(), TransportError>;
}

//! Stdin/stdout transport, to try the gateway without a chat app.
//! Each line typed is a message from user `me`; replies print to stdout.

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use super::{Transport, TransportError};
use crate::engine::status::{Connection, ConnectionReporter};
use crate::engine::types::{Capabilities, Overflow, ConversationKey, InboundMessage, MessageRef};

pub struct Console;

fn key() -> ConversationKey {
    ConversationKey { transport: "console".into(), workspace: "local".into(), channel: "tty".into(), thread: None }
}

#[async_trait]
impl Transport for Console {
    fn name(&self) -> &'static str {
        "console"
    }

    fn display_name(&self) -> &'static str {
        "a terminal console"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { max_len: 4000, can_edit: false, overflow: Overflow::Chunk }
    }

    async fn run(&self, inbound: mpsc::Sender<InboundMessage>, reporter: ConnectionReporter) -> Result<(), TransportError> {
        reporter.set(Connection::Connected { workspace: "local".into(), bot: "console".into() });
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Some(line) = lines.next_line().await? {
            let text = line.trim().to_string();
            if text.is_empty() {
                continue;
            }
            let message = InboundMessage { key: key(), sender_id: "me".into(), text, is_direct: true };
            if inbound.send(message).await.is_err() {
                break;
            }
        }
        Ok(())
    }

    async fn send(&self, to: &ConversationKey, text: &str) -> Result<MessageRef, TransportError> {
        println!("{}", text);
        Ok(MessageRef { key: to.clone(), id: "0".into() })
    }

    async fn edit(&self, _message: &MessageRef, _text: &str) -> Result<(), TransportError> {
        Err("console cannot edit messages".into())
    }
}

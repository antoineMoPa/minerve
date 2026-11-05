//! Stdin/stdout transport, to try the gateway without a chat app.
//! Each line typed is a message from user `me`; replies print to stdout.

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use super::{Transport, TransportError};
use crate::core::types::{Capabilities, ConversationKey, InboundMessage, MessageRef};

pub struct Console;

fn key() -> ConversationKey {
    ConversationKey { transport: "console".into(), workspace: "local".into(), channel: "tty".into(), thread: None }
}

#[async_trait]
impl Transport for Console {
    fn name(&self) -> &'static str {
        "console"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { max_len: 4000, can_edit: false }
    }

    async fn run(&self, inbound: mpsc::Sender<InboundMessage>) -> Result<(), TransportError> {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Some(line) = lines.next_line().await? {
            let text = line.trim().to_string();
            if text.is_empty() {
                continue;
            }
            let message = InboundMessage { key: key(), sender_id: "me".into(), text };
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

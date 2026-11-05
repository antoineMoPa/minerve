//! What the gateway is doing, read by minerve's `/message-apps` modal.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq)]
pub enum Connection {
    Connecting,
    Connected { workspace: String, bot: String },
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum LinkState {
    Waiting { code: String, expires_at: Instant },
    Linked { member: String },
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    /// By transport name.
    pub connections: BTreeMap<String, Connection>,
    pub conversations: usize,
    /// Turns running now.
    pub working: usize,
    pub link: Option<LinkState>,
}

pub type SharedStatus = Arc<Mutex<Status>>;

/// Handed to a transport so it can report its own connection.
#[derive(Clone)]
pub struct ConnectionReporter {
    status: SharedStatus,
    transport: &'static str,
}

impl ConnectionReporter {
    pub fn new(status: SharedStatus, transport: &'static str) -> Self {
        Self { status, transport }
    }

    pub fn set(&self, connection: Connection) {
        self.status.lock().unwrap().connections.insert(self.transport.to_string(), connection);
    }
}

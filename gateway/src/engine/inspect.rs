//! What `minerve --threads` reads: a snapshot of the running gateway, written next to the lock.
//!
//! The gateway's live state (turns running now, connections) exists only in its process. It
//! writes it here at each change, with its pid, so another process can show it. The snapshot
//! is advisory: a stale one whose pid is gone means the gateway stopped.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::status::{Connection, Status};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    pub pid: u32,
    pub workdir: String,
    /// By transport name, as the gateway reports them.
    pub connections: BTreeMap<String, String>,
    pub conversations: usize,
    pub working: usize,
    /// Conversation keys whose turn runs now.
    pub running: Vec<String>,
    pub cost_usd: f64,
    pub cost_incomplete: bool,
    pub updated_at_unix: u64,
}

pub fn default_path() -> PathBuf {
    dirs::home_dir().expect("no home directory").join(".minerve/message-apps-status.json")
}

impl Snapshot {
    pub fn from_status(status: &Status, pid: u32, workdir: &str) -> Self {
        Self {
            pid,
            workdir: workdir.to_string(),
            connections: status
                .connections
                .iter()
                .map(|(name, connection)| (name.clone(), connection_label(connection)))
                .collect(),
            conversations: status.conversations,
            working: status.working,
            running: status.running.iter().cloned().collect(),
            cost_usd: status.cost_usd,
            cost_incomplete: status.cost_incomplete,
            updated_at_unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }
}

fn connection_label(connection: &Connection) -> String {
    match connection {
        Connection::Connecting => "connecting".to_string(),
        Connection::Connected { workspace, bot } => format!("connected: {} ({})", bot, workspace),
        Connection::Failed(err) => format!("failed: {}", err),
    }
}

/// Writes the snapshot; a failure is ignored, since the gateway works without it.
pub fn write(path: &PathBuf, snapshot: &Snapshot) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(snapshot) {
        let _ = std::fs::write(path, text);
    }
}

pub fn read(path: &PathBuf) -> Option<Snapshot> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Removes the snapshot when the gateway stops, so no stale one is left behind.
pub fn remove(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_round_trips_through_its_file() {
        let path = std::env::temp_dir().join(format!("gw-status-{}.json", uuid::Uuid::new_v4()));
        let mut status = Status::default();
        status.running.insert("slack:T1:C1:123.4".into());
        status.working = 1;
        status.cost_usd = 0.5;
        let snapshot = Snapshot::from_status(&status, 42, "/tmp/work");
        write(&path, &snapshot);
        let back = read(&path).unwrap();
        assert_eq!(back.pid, 42);
        assert_eq!(back.running, vec!["slack:T1:C1:123.4".to_string()]);
        assert_eq!(back.workdir, "/tmp/work");
        remove(&path);
        assert!(read(&path).is_none());
    }
}

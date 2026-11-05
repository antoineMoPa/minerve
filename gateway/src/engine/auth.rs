//! Who may talk to the agent. It can run shell commands, so the default is nobody.
//! Entries are `<transport>:<sender id>`, e.g. `slack:U01ABC`: an id means nothing outside its app.

use std::collections::BTreeSet;
use std::sync::Mutex;

pub fn entry(transport: &str, sender_id: &str) -> String {
    format!("{}:{}", transport, sender_id)
}

pub struct Allowlist {
    allowed: Mutex<BTreeSet<String>>,
}

impl Allowlist {
    pub fn new(entries: impl IntoIterator<Item = String>) -> Self {
        Self { allowed: Mutex::new(entries.into_iter().collect()) }
    }

    pub fn allows(&self, transport: &str, sender_id: &str) -> bool {
        self.allowed.lock().unwrap().contains(&entry(transport, sender_id))
    }

    /// Adds a sender and returns every entry, to be saved.
    pub fn add(&self, transport: &str, sender_id: &str) -> Vec<String> {
        let mut allowed = self.allowed.lock().unwrap();
        allowed.insert(entry(transport, sender_id));
        allowed.iter().cloned().collect()
    }

    pub fn entries(&self) -> Vec<String> {
        self.allowed.lock().unwrap().iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_list_allows_nobody() {
        assert!(!Allowlist::new([]).allows("slack", "U1"));
    }

    #[test]
    fn an_id_is_only_allowed_on_its_own_app() {
        let list = Allowlist::new([entry("slack", "U1")]);
        assert!(list.allows("slack", "U1"));
        assert!(!list.allows("telegram", "U1"));
        assert_eq!(list.add("telegram", "42"), vec!["slack:U1", "telegram:42"]);
    }
}

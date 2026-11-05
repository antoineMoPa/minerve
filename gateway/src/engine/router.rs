//! Routes inbound messages to one actor per conversation, so a conversation runs one turn at
//! a time while different conversations run in parallel.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::mpsc;

use super::agent::Agent;
use super::auth::Allowlist;
use super::config;
use super::link::PendingLink;
use super::log::log;
use super::render::{length_instruction, plan_reply};
use super::sessions::SessionStore;
use super::status::{LinkState, SharedStatus};
use super::types::{ConversationKey, InboundMessage};
use crate::transport::Transport;

pub struct Router {
    pub allowlist: Allowlist,
    pub sessions: SessionStore,
    pub agent: Agent,
    pub status: SharedStatus,
    /// The code shown in minerve while it waits for someone to link.
    pub link: Mutex<Option<PendingLink>>,
    /// Where a newly linked sender is saved. `None` keeps it in memory (tests).
    pub config_path: Option<PathBuf>,
}

pub const LINKED_REPLY: &str = "Linked. You can talk to me now.";

impl Router {
    /// Handles messages from `inbound` until every sender is gone.
    pub async fn serve(
        self: Arc<Self>,
        transports: HashMap<&'static str, Arc<dyn Transport>>,
        mut inbound: mpsc::Receiver<InboundMessage>,
    ) {
        let mut actors: HashMap<ConversationKey, mpsc::UnboundedSender<InboundMessage>> = HashMap::new();
        while let Some(message) = inbound.recv().await {
            let Some(transport) = transports.get(message.key.transport.as_str()).cloned() else { continue };
            if !self.allowlist.allows(&message.key.transport, &message.sender_id) {
                // Never answered, unless it is the link code: then the sender is allowed from now on.
                if self.try_link(&message, Instant::now()) {
                    if let Err(err) = transport.send(&message.key, LINKED_REPLY).await {
                        log(format!("send to {} failed: {}", message.key, err));
                    }
                } else {
                    log(format!("ignored {} from unlisted sender {}", message.key, message.sender_id));
                }
                continue;
            }
            let sender = actors.entry(message.key.clone()).or_insert_with(|| {
                let (tx, rx) = mpsc::unbounded_channel();
                tokio::spawn(self.clone().conversation(transport.clone(), rx));
                tx
            });
            let _ = sender.send(message);
            self.status.lock().unwrap().conversations = actors.len();
        }
    }

    /// Allows the sender when `message` is the pending code, sent directly to the bot. The code
    /// then is used up; an expired one is dropped.
    pub fn try_link(&self, message: &InboundMessage, now: Instant) -> bool {
        let mut link = self.link.lock().unwrap();
        let Some(pending) = link.as_ref() else { return false };
        if now >= pending.expires_at {
            *link = None;
            return false;
        }
        if !pending.matches(&message.text, now) {
            return false;
        }
        if !message.is_direct {
            // Posted where others can read it: the code is burnt, nobody links with it.
            *link = None;
            self.status.lock().unwrap().link = None;
            log(format!("link code posted outside a direct message by {}; cancelled", message.sender_id));
            return false;
        }
        *link = None;
        let allowed = self.allowlist.add(&message.key.transport, &message.sender_id);
        if let Some(path) = &self.config_path {
            let saved = config::load_from(path).and_then(|mut config| {
                config.allowed = allowed;
                config::save_to(path, &config)
            });
            if let Err(err) = saved {
                log(format!("could not save the allowlist: {}", err));
            }
        }
        log(format!("linked {}:{}", message.key.transport, message.sender_id));
        self.status.lock().unwrap().link = Some(LinkState::Linked { member: message.sender_id.clone() });
        true
    }

    async fn conversation(self: Arc<Self>, transport: Arc<dyn Transport>, mut queue: mpsc::UnboundedReceiver<InboundMessage>) {
        // Set when a reply was cut, delivered to the agent with the next prompt.
        let mut pending_note: Option<String> = None;
        while let Some(message) = queue.recv().await {
            let session_id = self.sessions.session_for(&message.key);
            let capabilities = transport.capabilities();
            // Where the transport can edit, a placeholder shows at once and becomes the first piece of the reply.
            let placeholder = if capabilities.can_edit {
                transport.send(&message.key, "_working…_").await.ok()
            } else {
                None
            };
            let prompt = match pending_note.take() {
                Some(note) => format!("[gateway note: {}]\n\n{}", note, message.text),
                None => message.text.clone(),
            };
            // Given once, when the session starts: where the agent runs, and any length limit.
            let mut system_prompts = vec![format!("You are running in {}.", transport.display_name())];
            system_prompts.extend(length_instruction(capabilities));
            let reply = match self.agent.run_turn(&session_id, &system_prompts, &prompt).await {
                Ok(reply) => reply,
                Err(err) => format!("Error: {}", err),
            };
            let mut placeholder = placeholder;
            let (pieces, note) = plan_reply(&reply, capabilities);
            pending_note = note;
            for piece in pieces {
                let text = transport.format(&piece);
                let edited = match placeholder.take() {
                    Some(status) => transport.edit(&status, &text).await.is_ok(),
                    None => false,
                };
                if edited {
                    continue;
                }
                if let Err(err) = transport.send(&message.key, &text).await {
                    log(format!("send to {} failed: {}", message.key, err));
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::link::LINK_TTL;
    use crate::engine::types::ConversationKey;

    fn router() -> Router {
        Router {
            allowlist: Allowlist::new([]),
            sessions: SessionStore::open(std::env::temp_dir().join(format!("gw-{}.json", uuid::Uuid::new_v4()))),
            agent: Agent::new("minerve".into(), ".".into()),
            status: SharedStatus::default(),
            link: Mutex::new(None),
            config_path: None,
        }
    }

    fn message(text: &str, is_direct: bool) -> InboundMessage {
        InboundMessage {
            key: ConversationKey { transport: "slack".into(), workspace: "T1".into(), channel: "D1".into(), thread: None },
            sender_id: "U1".into(),
            text: text.into(),
            is_direct,
        }
    }

    #[test]
    fn the_code_links_once_and_only_in_a_direct_message() {
        let router = router();
        let now = Instant::now();
        assert!(!router.try_link(&message("ABCD-EF23", true), now), "no code pending");
        *router.link.lock().unwrap() = Some(PendingLink { code: "ABCD-EF23".into(), expires_at: now + LINK_TTL });
        assert!(!router.try_link(&message("nope", true), now));
        assert!(router.try_link(&message("abcd-ef23", true), now));
        assert!(router.allowlist.allows("slack", "U1"));
        assert!(!router.try_link(&message("ABCD-EF23", true), now), "used up");
        assert_eq!(router.status.lock().unwrap().link, Some(LinkState::Linked { member: "U1".into() }));
    }

    #[test]
    fn a_code_posted_in_a_channel_is_burnt() {
        let router = router();
        let now = Instant::now();
        *router.link.lock().unwrap() = Some(PendingLink { code: "ABCD-EF23".into(), expires_at: now + LINK_TTL });
        assert!(!router.try_link(&message("ABCD-EF23", false), now), "a channel never links");
        assert!(!router.try_link(&message("ABCD-EF23", true), now), "and the code is gone");
        assert!(!router.allowlist.allows("slack", "U1"));
    }

    #[test]
    fn an_expired_code_links_nobody() {
        let router = router();
        let now = Instant::now();
        *router.link.lock().unwrap() = Some(PendingLink { code: "ABCD-EF23".into(), expires_at: now });
        assert!(!router.try_link(&message("ABCD-EF23", true), now));
        assert!(router.link.lock().unwrap().is_none());
    }
}

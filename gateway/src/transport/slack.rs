//! Slack over Socket Mode: the gateway dials out, so no public URL is needed.
//!
//! Needs a Slack app with Socket Mode on, an app-level token (`xapp-…`, scope
//! `connections:write`), a bot token (`xoxb-…`, scopes `chat:write`, `app_mentions:read`,
//! `im:history`), and the events `app_mention` and `message.im` subscribed.
//!
//! Conversations: a DM is one flat conversation; a mention in a channel starts (or continues)
//! a thread, and the thread is the conversation.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use super::slack_mrkdwn::to_mrkdwn;
use super::{Transport, TransportError};
use crate::engine::config::SlackTokens;
use crate::engine::log::log;
use crate::engine::status::{Connection, ConnectionReporter};
use crate::engine::types::{Capabilities, Overflow, ConversationKey, InboundMessage, MessageRef};

const API: &str = "https://slack.com/api";
const SEEN_EVENTS: usize = 200;

#[derive(Clone)]
pub struct Identity {
    pub team_id: String,
    pub bot_user_id: String,
    /// Shown in minerve: the workspace's name and the bot's handle.
    pub team_name: String,
    pub bot_name: String,
}

/// The Slack app manifest for a bot named `bot_name`, to paste at https://api.slack.com/apps
/// (Create New App → From a manifest). JSON, which Slack's JSON tab takes and is valid YAML too.
pub fn manifest(bot_name: &str) -> String {
    let manifest = json!({
        "display_information": { "name": bot_name },
        "features": {
            "app_home": { "messages_tab_enabled": true, "messages_tab_read_only_enabled": false },
            "bot_user": { "display_name": bot_name, "always_online": true }
        },
        "oauth_config": {
            "scopes": { "bot": ["app_mentions:read", "chat:write", "im:history", "im:read", "im:write"] }
        },
        "settings": {
            "event_subscriptions": { "bot_events": ["app_mention", "message.im"] },
            "socket_mode_enabled": true,
            "org_deploy_enabled": false,
            "token_rotation_enabled": false
        }
    });
    serde_json::to_string_pretty(&manifest).expect("a JSON value always serializes")
}

/// Slack answered `ok: false`: the request itself is wrong (bad token, missing scope), so
/// retrying it will not help, unlike a network error.
#[derive(Debug)]
struct SlackApiError(String);

impl std::fmt::Display for SlackApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SlackApiError {}

pub struct Slack {
    app_token: String,
    bot_token: String,
    http: reqwest::Client,
    seen_events: Mutex<VecDeque<String>>,
}

impl Slack {
    pub fn new(tokens: SlackTokens) -> Self {
        Self { app_token: tokens.app_token, bot_token: tokens.bot_token, http: reqwest::Client::new(), seen_events: Mutex::new(VecDeque::new()) }
    }

    async fn call(&self, method: &str, token: &str, body: Value) -> Result<Value, TransportError> {
        let response: Value = self
            .http
            .post(format!("{}/{}", API, method))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await?
            .json()
            .await?;
        if response["ok"] == true {
            Ok(response)
        } else {
            Err(Box::new(SlackApiError(format!("slack {}: {}", method, response["error"].as_str().unwrap_or("unknown error")))))
        }
    }

    async fn whoami(&self) -> Result<Identity, TransportError> {
        let me = self.call("auth.test", &self.bot_token, json!({})).await?;
        Ok(Identity {
            team_id: me["team_id"].as_str().ok_or("auth.test without team_id")?.to_string(),
            bot_user_id: me["user_id"].as_str().ok_or("auth.test without user_id")?.to_string(),
            team_name: me["team"].as_str().unwrap_or_default().to_string(),
            bot_name: me["user"].as_str().unwrap_or_default().to_string(),
        })
    }

    /// True the first time an event id is seen: Slack redelivers when an ack is late.
    fn first_delivery(&self, event_id: &str) -> bool {
        let mut seen = self.seen_events.lock().unwrap();
        if seen.iter().any(|id| id == event_id) {
            return false;
        }
        seen.push_back(event_id.to_string());
        if seen.len() > SEEN_EVENTS {
            seen.pop_front();
        }
        true
    }

    /// One websocket session. `Ok` when Slack asked us to reconnect, `Err` when it broke.
    /// Sets `connected` once Slack says hello, so a long-lived connection that drops resets the backoff.
    async fn session(
        &self,
        me: &Identity,
        inbound: &mpsc::Sender<InboundMessage>,
        reporter: &ConnectionReporter,
        connected: &mut bool,
    ) -> Result<(), TransportError> {
        let opened = self.call("apps.connections.open", &self.app_token, json!({})).await?;
        let url = opened["url"].as_str().ok_or("apps.connections.open without url")?;
        let (mut socket, _) = tokio_tungstenite::connect_async(url).await?;
        while let Some(frame) = socket.next().await {
            let Message::Text(text) = frame? else { continue };
            let envelope: Value = serde_json::from_str(&text)?;
            if let Some(envelope_id) = envelope["envelope_id"].as_str() {
                socket.send(Message::text(json!({ "envelope_id": envelope_id }).to_string())).await?;
            }
            match envelope["type"].as_str() {
                Some("hello") => {
                    *connected = true;
                    reporter.set(Connection::Connected { workspace: me.team_name.clone(), bot: me.bot_name.clone() });
                }
                Some("disconnect") => return Ok(()),
                Some("events_api") => {
                    let first = envelope["payload"]["event_id"].as_str().map(|id| self.first_delivery(id)).unwrap_or(true);
                    if let Some(message) = first.then(|| parse_event(&envelope["payload"], me)).flatten() {
                        if inbound.send(message).await.is_err() {
                            return Ok(());
                        }
                    }
                }
                _ => {}
            }
        }
        Err("slack closed the socket".into())
    }
}

/// Turns an `events_api` payload into a message, or `None` for events the gateway ignores:
/// bots, edits and other subtypes, channel chatter that is not a mention, and anything from
/// outside the bot's own workspace (a Slack Connect guest is another team's user).
pub fn parse_event(payload: &Value, me: &Identity) -> Option<InboundMessage> {
    let bot_user_id = &me.bot_user_id;
    let event = &payload["event"];
    let foreign = |team: &Value| team.as_str().is_some_and(|team| team != me.team_id);
    if foreign(&payload["team_id"]) || foreign(&event["user_team"]) || foreign(&event["team"]) {
        return None;
    }
    // `file_share` is a message with an attachment: its text still counts. Other subtypes are
    // edits, joins and the like.
    let subtype = event["subtype"].as_str();
    if event.get("bot_id").is_some() || subtype.is_some_and(|subtype| subtype != "file_share") {
        return None;
    }
    let is_dm = event["channel_type"] == "im";
    match event["type"].as_str()? {
        // In a channel the mention arrives as both `message` and `app_mention`; keep the latter.
        "message" if is_dm => {}
        "app_mention" => {}
        _ => return None,
    }
    let sender_id = event["user"].as_str()?.to_string();
    let mention = format!("<@{}>", bot_user_id);
    let text = event["text"].as_str()?.replace(&mention, "").trim().to_string();
    if text.is_empty() {
        return None;
    }
    let thread = match (event["thread_ts"].as_str(), is_dm) {
        (Some(ts), _) => Some(ts.to_string()),
        (None, true) => None,
        (None, false) => Some(event["ts"].as_str()?.to_string()),
    };
    Some(InboundMessage {
        key: ConversationKey {
            transport: "slack".into(),
            workspace: payload["team_id"].as_str()?.to_string(),
            channel: event["channel"].as_str()?.to_string(),
            thread,
        },
        sender_id,
        text,
        is_direct: is_dm,
    })
}

#[async_trait]
impl Transport for Slack {
    fn name(&self) -> &'static str {
        "slack"
    }

    fn display_name(&self) -> &'static str {
        "Slack"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { max_len: 3500, can_edit: true, overflow: Overflow::Chunk }
    }

    fn format(&self, text: &str) -> String {
        to_mrkdwn(text)
    }

    async fn run(&self, inbound: mpsc::Sender<InboundMessage>, reporter: ConnectionReporter) -> Result<(), TransportError> {
        reporter.set(Connection::Connecting);
        let mut failures = 0u32;
        // Bad tokens are not worth retrying: report them and stop. A network that is down (a
        // laptop just waking up) is: try again.
        let me = loop {
            match self.whoami().await {
                Ok(me) => break me,
                Err(err) if err.downcast_ref::<SlackApiError>().is_some() => {
                    reporter.set(Connection::Failed(err.to_string()));
                    return Err(err);
                }
                Err(err) => {
                    failures += 1;
                    reporter.set(Connection::Failed(err.to_string()));
                    tokio::time::sleep(Duration::from_secs(2u64.pow(failures.min(5)))).await;
                    reporter.set(Connection::Connecting);
                }
            }
        };
        failures = 0;
        loop {
            let mut connected = false;
            let result = self.session(&me, &inbound, &reporter, &mut connected).await;
            if connected {
                failures = 0;
            }
            match result {
                Ok(()) => failures = 0,
                Err(err) => {
                    failures += 1;
                    log(format!("slack connection lost: {}", err));
                    reporter.set(Connection::Failed(err.to_string()));
                }
            }
            if inbound.is_closed() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(2u64.pow(failures.min(5)))).await;
            reporter.set(Connection::Connecting);
        }
    }

    async fn send(&self, to: &ConversationKey, text: &str) -> Result<MessageRef, TransportError> {
        let mut body = json!({ "channel": to.channel, "text": text });
        if let Some(thread) = &to.thread {
            body["thread_ts"] = json!(thread);
        }
        let response = self.call("chat.postMessage", &self.bot_token, body).await?;
        let ts = response["ts"].as_str().ok_or("chat.postMessage without ts")?;
        Ok(MessageRef { key: to.clone(), id: ts.to_string() })
    }

    async fn edit(&self, message: &MessageRef, text: &str) -> Result<(), TransportError> {
        let body = json!({ "channel": message.key.channel, "ts": message.id, "text": text });
        self.call("chat.update", &self.bot_token, body).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> Identity {
        Identity { team_id: "T1".into(), bot_user_id: "UBOT".into(), team_name: "Acme".into(), bot_name: "bot".into() }
    }

    fn payload(event: Value) -> Value {
        json!({ "team_id": "T1", "event_id": "Ev1", "event": event })
    }

    #[test]
    fn manifest_is_plain_json_with_the_bot_name() {
        let manifest: Value = serde_json::from_str(&manifest("antoine-bot")).unwrap();
        assert_eq!(manifest["display_information"]["name"], "antoine-bot");
        assert_eq!(manifest["features"]["bot_user"]["display_name"], "antoine-bot");
        assert_eq!(manifest["settings"]["socket_mode_enabled"], true);
    }

    #[test]
    fn dm_is_flat() {
        let message = parse_event(
            &payload(json!({"type":"message","channel_type":"im","user":"U1","channel":"D1","text":"hi","ts":"1.0"})),
            &me(),
        )
        .unwrap();
        assert_eq!((message.key.thread, message.text.as_str(), message.sender_id.as_str()), (None, "hi", "U1"));
    }

    #[test]
    fn mention_starts_a_thread_and_strips_the_mention() {
        let message = parse_event(
            &payload(json!({"type":"app_mention","user":"U1","channel":"C1","text":"<@UBOT> do it","ts":"5.5"})),
            &me(),
        )
        .unwrap();
        assert_eq!((message.key.thread.as_deref(), message.text.as_str()), (Some("5.5"), "do it"));
    }

    #[test]
    fn mention_inside_a_thread_keeps_it() {
        let message = parse_event(
            &payload(json!({"type":"app_mention","user":"U1","channel":"C1","text":"<@UBOT> x","ts":"6.0","thread_ts":"5.5"})),
            &me(),
        )
        .unwrap();
        assert_eq!(message.key.thread.as_deref(), Some("5.5"));
    }

    #[test]
    fn ignores_other_workspaces_and_connect_guests() {
        let mention = |event: Value, team: &str| parse_event(&json!({"team_id": team, "event": event}), &me());
        let event = json!({"type":"app_mention","user":"U1","channel":"C1","text":"<@UBOT> x","ts":"1"});
        assert!(mention(event.clone(), "T1").is_some());
        assert!(mention(event.clone(), "T2").is_none());
        let mut guest = event;
        guest["user_team"] = json!("T9");
        assert!(mention(guest, "T1").is_none());
    }

    #[test]
    fn ignores_bots_edits_and_channel_chatter() {
        let ignored = |event: Value| parse_event(&payload(event), &me()).is_none();
        assert!(ignored(json!({"type":"message","channel_type":"im","user":"U1","channel":"D1","text":"x","ts":"1","bot_id":"B1"})));
        assert!(ignored(json!({"type":"message","channel_type":"im","user":"U1","channel":"D1","text":"x","ts":"1","subtype":"message_changed"})));
        assert!(!ignored(json!({"type":"message","channel_type":"im","user":"U1","channel":"D1","text":"x","ts":"1","subtype":"file_share"})));
        assert!(ignored(json!({"type":"message","channel_type":"channel","user":"U1","channel":"C1","text":"<@UBOT> x","ts":"1"})));
        assert!(ignored(json!({"type":"app_mention","user":"U1","channel":"C1","text":"<@UBOT>","ts":"1"})));
    }
}

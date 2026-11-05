//! MCP (Model Context Protocol) servers whose tools the model can call.
//!
//! Servers are listed in `~/.minerve/mcp.json` and `./.mcp.json` (see `config`). Minerve starts
//! connecting at launch; a turn waits until no server is still connecting. `/mcp` adds, removes,
//! connects and disconnects servers while minerve runs (see `mcp_manager`).
//! Server tools are offered to the model as `mcp__<server>__<tool>`.

pub mod client;
pub mod config;
pub mod oauth;

use crate::ChatCompletionFunctionDefinition;
use client::{ConnectError, McpServer};
use config::{ConfigSource, McpServerConfig};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::watch;

const TOOL_NAME_PREFIX: &str = "mcp__";
const TOOL_NAME_SEPARATOR: &str = "__";
/// OpenAI rejects longer function names.
const MAX_TOOL_NAME_LEN: usize = 64;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub enum McpConnection {
    Connecting,
    Connected(Arc<McpServer>),
    Failed(String),
    /// The server answered 401. Holds its `WWW-Authenticate` header, which starts the login.
    NeedsLogin { challenge: String },
    /// Browser login running. The URL is set once the browser is sent there.
    LoggingIn { authorize_url: Option<String> },
    /// Disconnected from `/mcp`. Lasts until minerve exits or the server is connected again.
    Disconnected,
}

struct McpServerSlot {
    config: McpServerConfig,
    source: ConfigSource,
    connection: McpConnection,
    /// Bumped on every connect. A connect that finishes after a newer connect or a disconnect
    /// started is dropped.
    attempt: u64,
}

/// What `/mcp` shows of one server.
pub struct McpServerRow {
    pub name: String,
    pub config: McpServerConfig,
    pub source: ConfigSource,
    pub connection: McpConnection,
}

struct Registry {
    slots: Mutex<BTreeMap<String, McpServerSlot>>,
    /// Bumped after every connection change, to wake turns waiting for connects to finish.
    changes: watch::Sender<u64>,
}

static REGISTRY: OnceLock<Registry> = OnceLock::new();

fn registry() -> &'static Registry {
    REGISTRY.get().expect("mcp::start_connecting not called")
}

fn notify_change() {
    registry().changes.send_modify(|count| *count += 1);
}

fn tool_function_name(server_name: &str, tool_name: &str) -> String {
    format!("{}{}{}{}", TOOL_NAME_PREFIX, server_name, TOOL_NAME_SEPARATOR, tool_name)
}

fn is_valid_function_name(function_name: &str) -> bool {
    function_name.len() <= MAX_TOOL_NAME_LEN
        && function_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Names of the tools of `server_name` that cannot be function names, e.g. with a `.` or too long.
/// They are left out of requests.
pub fn skipped_tool_names(server_name: &str, server: &McpServer) -> Vec<String> {
    server
        .tools
        .iter()
        .filter(|tool| !is_valid_function_name(&tool_function_name(server_name, &tool.name)))
        .map(|tool| tool.name.to_string())
        .collect()
}

/// Starts connecting to the configured servers in the background. Called once, at launch.
pub fn start_connecting(servers: BTreeMap<String, (McpServerConfig, ConfigSource)>) {
    let slots = servers
        .into_iter()
        .map(|(name, (config, source))| {
            (name, McpServerSlot { config, source, connection: McpConnection::Disconnected, attempt: 0 })
        })
        .collect::<BTreeMap<_, _>>();
    let names: Vec<String> = slots.keys().cloned().collect();
    let registry = Registry { slots: Mutex::new(slots), changes: watch::channel(0).0 };
    if REGISTRY.set(registry).is_err() {
        panic!("mcp::start_connecting called twice");
    }
    for name in names {
        connect(&name);
    }
}

/// Connects (or reconnects) a configured server in the background.
pub fn connect(name: &str) {
    let (config, attempt) = {
        let mut slots = registry().slots.lock().unwrap();
        let slot = slots.get_mut(name).unwrap_or_else(|| panic!("unknown MCP server '{}'", name));
        slot.attempt += 1;
        slot.connection = McpConnection::Connecting;
        (slot.config.clone(), slot.attempt)
    };
    notify_change();
    let name = name.to_string();
    crate::get_global_runtime().spawn(async move {
        let connection = match tokio::time::timeout(CONNECT_TIMEOUT, McpServer::connect(&config)).await {
            Ok(Ok(server)) => McpConnection::Connected(Arc::new(server)),
            Ok(Err(ConnectError::NeedsLogin { challenge })) => McpConnection::NeedsLogin { challenge },
            Ok(Err(ConnectError::Failed(err))) => McpConnection::Failed(err),
            Err(_) => McpConnection::Failed(format!("no answer within {}s", CONNECT_TIMEOUT.as_secs())),
        };
        set_connection_if_current(&name, attempt, connection);
    });
}

/// Sets the state of a server unless it was removed, disconnected or reconnected since
/// `attempt` started: then the result is not wanted.
fn set_connection_if_current(name: &str, attempt: u64, connection: McpConnection) {
    {
        let mut slots = registry().slots.lock().unwrap();
        let Some(slot) = slots.get_mut(name).filter(|slot| slot.attempt == attempt) else {
            return;
        };
        slot.connection = connection;
    }
    notify_change();
}

/// Starts the browser login of a server that answered 401, then connects with the new token.
pub fn log_in(name: &str) {
    let (url, challenge, attempt) = {
        let mut slots = registry().slots.lock().unwrap();
        let slot = slots.get_mut(name).unwrap_or_else(|| panic!("unknown MCP server '{}'", name));
        let McpServerConfig::Http { url, .. } = &slot.config else {
            panic!("login of stdio MCP server '{}'", name);
        };
        let McpConnection::NeedsLogin { challenge } = &slot.connection else {
            panic!("login of MCP server '{}' that does not need one", name);
        };
        let login = (url.clone(), challenge.clone());
        slot.attempt += 1;
        slot.connection = McpConnection::LoggingIn { authorize_url: None };
        (login.0, login.1, slot.attempt)
    };
    notify_change();
    let name = name.to_string();
    crate::get_global_runtime().spawn(async move {
        let show_url = |authorize_url: String| {
            set_connection_if_current(&name, attempt, McpConnection::LoggingIn { authorize_url: Some(authorize_url) });
        };
        match oauth::log_in(&url, &challenge, show_url).await {
            Ok(()) => {
                let still_current = registry().slots.lock().unwrap().get(&name).is_some_and(|slot| slot.attempt == attempt);
                if still_current {
                    connect(&name);
                }
            }
            Err(err) => set_connection_if_current(&name, attempt, McpConnection::Failed(format!("login: {}", err))),
        }
    });
}

/// Drops the connection. A tool call already running keeps the server until it answers.
pub fn disconnect(name: &str) {
    {
        let mut slots = registry().slots.lock().unwrap();
        let slot = slots.get_mut(name).unwrap_or_else(|| panic!("unknown MCP server '{}'", name));
        slot.attempt += 1;
        slot.connection = McpConnection::Disconnected;
    }
    notify_change();
}

/// Saves a new server to `~/.minerve/mcp.json` and connects to it.
pub fn add_server(name: &str, config: McpServerConfig) -> Result<(), String> {
    config::check_server_name(name)?;
    if registry().slots.lock().unwrap().contains_key(name) {
        return Err(format!("a server named '{}' already exists", name));
    }
    config::write_server_to_file(&ConfigSource::User.path(), name, &config)?;
    registry().slots.lock().unwrap().insert(
        name.to_string(),
        McpServerSlot { config, source: ConfigSource::User, connection: McpConnection::Disconnected, attempt: 0 },
    );
    connect(name);
    Ok(())
}

/// Disconnects the server, deletes it from the config file it was read from and forgets its login.
pub fn remove_server(name: &str) -> Result<(), String> {
    let (source, config) = {
        let slots = registry().slots.lock().unwrap();
        let slot = slots.get(name).unwrap_or_else(|| panic!("unknown MCP server '{}'", name));
        (slot.source, slot.config.clone())
    };
    config::remove_server_from_file(&source.path(), name)?;
    if let McpServerConfig::Http { url, .. } = &config {
        oauth::forget_login(url)?;
    }
    registry().slots.lock().unwrap().remove(name);
    notify_change();
    Ok(())
}

pub fn server_rows() -> Vec<McpServerRow> {
    registry()
        .slots
        .lock()
        .unwrap()
        .iter()
        .map(|(name, slot)| McpServerRow {
            name: name.clone(),
            config: slot.config.clone(),
            source: slot.source,
            connection: slot.connection.clone(),
        })
        .collect()
}

pub fn is_any_connecting() -> bool {
    registry()
        .slots
        .lock()
        .unwrap()
        .values()
        .any(|slot| matches!(slot.connection, McpConnection::Connecting))
}

/// Waits until no server is connecting, then returns the servers connected at that point.
/// A turn takes this once per request, so `/mcp` changes apply from the next request on.
pub async fn connected_servers() -> McpServers {
    let mut changes = registry().changes.subscribe();
    while is_any_connecting() {
        // Every connect ends with a change, and connects time out, so this wait ends.
        changes.changed().await.expect("MCP registry dropped");
    }
    let servers = registry()
        .slots
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(name, slot)| match &slot.connection {
            McpConnection::Connected(server) => Some((name.clone(), Arc::clone(server))),
            _ => None,
        })
        .collect();
    McpServers { servers }
}

/// Servers connected when a request was built. Tool calls of that request go to these.
pub struct McpServers {
    servers: Vec<(String, Arc<McpServer>)>,
}

impl McpServers {
    /// Function definitions of every tool of every connected server, sorted by function name
    /// so requests keep the same tool order and the prompt cache holds.
    pub fn function_definitions(&self) -> Vec<ChatCompletionFunctionDefinition> {
        let mut definitions: Vec<ChatCompletionFunctionDefinition> = self
            .servers
            .iter()
            .flat_map(|(server_name, server)| {
                server.tools.iter().filter_map(move |tool| {
                    let function_name = tool_function_name(server_name, &tool.name);
                    if !is_valid_function_name(&function_name) {
                        return None;
                    }
                    let description = match &tool.description {
                        Some(description) => format!("[MCP server {}] {}", server_name, description),
                        None => format!("[MCP server {}]", server_name),
                    };
                    Some(ChatCompletionFunctionDefinition {
                        name: function_name,
                        description: Some(description),
                        parameters: Some(Value::Object(tool.input_schema.as_ref().clone())),
                    })
                })
            })
            .collect();
        definitions.sort_by(|a, b| a.name.cmp(&b.name));
        definitions
    }

    /// Runs the MCP tool behind `function_name`. None when no connected server has that tool.
    /// `arguments` is the JSON object the model wrote; models send an empty string for no arguments.
    pub async fn call(&self, function_name: &str, arguments: &str) -> Option<Result<String, String>> {
        let (server, tool_name) = self.servers.iter().find_map(|(server_name, server)| {
            let tool_name = function_name
                .strip_prefix(TOOL_NAME_PREFIX)?
                .strip_prefix(server_name.as_str())?
                .strip_prefix(TOOL_NAME_SEPARATOR)?;
            server.tools.iter().any(|tool| tool.name == tool_name).then_some((server, tool_name))
        })?;
        let arguments = if arguments.trim().is_empty() {
            serde_json::Map::new()
        } else {
            match serde_json::from_str(arguments) {
                Ok(Value::Object(map)) => map,
                Ok(other) => return Some(Err(format!("arguments must be a JSON object, got {}", other))),
                Err(err) => return Some(Err(format!("arguments are not valid JSON: {}", err))),
            }
        };
        Some(server.call_tool(tool_name, arguments).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_names_follow_provider_limits() {
        assert!(is_valid_function_name(&tool_function_name("fs", "read_file")));
        assert!(!is_valid_function_name(&tool_function_name("fs", "read.file")));
        assert!(!is_valid_function_name(&tool_function_name("fs", &"x".repeat(60))));
    }
}

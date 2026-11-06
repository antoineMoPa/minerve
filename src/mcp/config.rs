//! MCP servers to connect to, read from `~/.minerve/mcp.json` and `./.mcp.json`.
//!
//! Same shape as Claude Code's `.mcp.json`:
//! ```json
//! {
//!   "mcpServers": {
//!     "filesystem": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "."] },
//!     "remote": { "type": "http", "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer ..." } }
//!   }
//! }
//! ```
//! A server without `type` is a stdio server. A server named in both files uses the `./.mcp.json` entry.

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const USER_CONFIG_PATH: &str = ".minerve/mcp.json";
pub const PROJECT_CONFIG_PATH: &str = ".mcp.json";

/// The file a server was read from, and where `/mcp` removes it from.
/// Servers added from `/mcp` go to `User`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConfigSource {
    User,
    Project,
}

impl ConfigSource {
    pub fn path(&self) -> PathBuf {
        match self {
            ConfigSource::User => dirs::home_dir().expect("no home directory").join(USER_CONFIG_PATH),
            ConfigSource::Project => PathBuf::from(PROJECT_CONFIG_PATH),
        }
    }

    pub fn label(&self) -> String {
        match self {
            ConfigSource::User => format!("~/{}", USER_CONFIG_PATH),
            ConfigSource::Project => format!("./{}", PROJECT_CONFIG_PATH),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum McpServerConfig {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
        oauth: Option<OAuthClient>,
    },
}

/// A pre-registered OAuth client, for servers that do not let clients register themselves
/// (Slack's, for one). Written as Claude Code writes it in `.mcp.json`:
/// `"oauth": {"clientId": "…", "callbackPort": 3118}`. The login then comes back to
/// `http://localhost:<callbackPort>/callback`, the redirect the client was registered with.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthClient {
    pub client_id: String,
    #[serde(default)]
    pub callback_port: Option<u16>,
}

#[derive(Debug, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
enum TransportKind {
    #[default]
    Stdio,
    Http,
}

/// One entry of `mcpServers` as written in the file. Turned into `McpServerConfig`, which
/// only holds the fields its transport uses.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpServerEntry {
    #[serde(rename = "type", default)]
    transport: TransportKind,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    url: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    oauth: Option<OAuthClient>,
}

#[derive(Debug, Deserialize)]
struct McpConfigFile {
    #[serde(rename = "mcpServers")]
    mcp_servers: BTreeMap<String, McpServerEntry>,
}

impl McpServerEntry {
    fn into_config(self, server_name: &str) -> Result<McpServerConfig, String> {
        match self.transport {
            TransportKind::Stdio => {
                if self.url.is_some() || !self.headers.is_empty() || self.oauth.is_some() {
                    return Err(format!("server '{}': url, headers and oauth need \"type\": \"http\"", server_name));
                }
                let command = self
                    .command
                    .ok_or_else(|| format!("server '{}': stdio server without \"command\"", server_name))?;
                Ok(McpServerConfig::Stdio { command, args: self.args, env: self.env })
            }
            TransportKind::Http => {
                if self.command.is_some() || !self.args.is_empty() || !self.env.is_empty() {
                    return Err(format!("server '{}': command, args and env are for stdio servers", server_name));
                }
                let url = self
                    .url
                    .ok_or_else(|| format!("server '{}': http server without \"url\"", server_name))?;
                Ok(McpServerConfig::Http { url, headers: self.headers, oauth: self.oauth })
            }
        }
    }
}

/// Server names become part of tool names (`mcp__<server>__<tool>`), which providers limit to
/// `[a-zA-Z0-9_-]`. A `__` inside the name would make the tool name ambiguous.
pub fn check_server_name(server_name: &str) -> Result<(), String> {
    let valid_chars = server_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if server_name.is_empty() || !valid_chars || server_name.contains("__") {
        return Err(format!(
            "server name '{}' must use only letters, digits, '-' and '_', without '__'",
            server_name
        ));
    }
    Ok(())
}

fn parse_config(text: &str) -> Result<BTreeMap<String, McpServerConfig>, String> {
    let file: McpConfigFile = serde_json::from_str(text).map_err(|err| err.to_string())?;
    file.mcp_servers
        .into_iter()
        .map(|(name, entry)| {
            check_server_name(&name)?;
            let config = entry.into_config(&name)?;
            Ok((name, config))
        })
        .collect()
}

fn read_config_file(path: &Path) -> Result<BTreeMap<String, McpServerConfig>, String> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {}", path.display(), err))?;
    parse_config(&text).map_err(|err| format!("{}: {}", path.display(), err))
}

/// Servers of `~/.minerve/mcp.json`, then of `./.mcp.json`, with the file each one comes from.
/// Missing files mean no servers.
pub fn load_mcp_servers() -> Result<BTreeMap<String, (McpServerConfig, ConfigSource)>, String> {
    let mut servers = BTreeMap::new();
    for source in [ConfigSource::User, ConfigSource::Project] {
        for (name, config) in read_config_file(&source.path())? {
            servers.insert(name, (config, source));
        }
    }
    Ok(servers)
}

/// The `mcpServers` entry written for `config`, leaving out empty fields.
fn config_as_entry_json(config: &McpServerConfig) -> Value {
    let mut entry = serde_json::Map::new();
    match config {
        McpServerConfig::Stdio { command, args, env } => {
            entry.insert("command".into(), json!(command));
            if !args.is_empty() {
                entry.insert("args".into(), json!(args));
            }
            if !env.is_empty() {
                entry.insert("env".into(), json!(env));
            }
        }
        McpServerConfig::Http { url, headers, oauth } => {
            entry.insert("type".into(), json!("http"));
            entry.insert("url".into(), json!(url));
            if !headers.is_empty() {
                entry.insert("headers".into(), json!(headers));
            }
            if let Some(oauth) = oauth {
                let mut client = json!({ "clientId": oauth.client_id });
                if let Some(port) = oauth.callback_port {
                    client["callbackPort"] = json!(port);
                }
                entry.insert("oauth".into(), client);
            }
        }
    }
    Value::Object(entry)
}

/// The file as JSON, or an empty object when it does not exist yet.
fn read_file_json(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {}", path.display(), err))?;
    serde_json::from_str(&text).map_err(|err| format!("{}: {}", path.display(), err))
}

fn write_file_json(path: &Path, file: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(file).expect("JSON value serializes") + "\n";
    std::fs::write(path, text).map_err(|err| format!("{}: {}", path.display(), err))
}

/// Adds the server to the file's `mcpServers`. Other keys of the file are kept as they are.
pub fn write_server_to_file(path: &Path, name: &str, config: &McpServerConfig) -> Result<(), String> {
    let mut file = read_file_json(path)?;
    let root = file
        .as_object_mut()
        .ok_or_else(|| format!("{}: not a JSON object", path.display()))?;
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("{}: \"mcpServers\" is not an object", path.display()))?;
    servers.insert(name.to_string(), config_as_entry_json(config));
    write_file_json(path, &file)
}

pub fn remove_server_from_file(path: &Path, name: &str) -> Result<(), String> {
    let mut file = read_file_json(path)?;
    let removed = file
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .and_then(|servers| servers.remove(name));
    if removed.is_none() {
        return Err(format!("{}: no server named '{}'", path.display(), name));
    }
    write_file_json(path, &file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_stdio_and_http_servers() {
        let servers = parse_config(
            r#"{"mcpServers": {
                "fs": {"command": "npx", "args": ["-y", "server"], "env": {"A": "1"}},
                "remote": {"type": "http", "url": "https://x/mcp", "headers": {"Authorization": "Bearer t"}}
            }}"#,
        )
        .unwrap();
        assert_eq!(
            servers["fs"],
            McpServerConfig::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "server".into()],
                env: BTreeMap::from([("A".into(), "1".into())]),
            }
        );
        assert_eq!(
            servers["remote"],
            McpServerConfig::Http {
                url: "https://x/mcp".into(),
                headers: BTreeMap::from([("Authorization".into(), "Bearer t".into())]),
                oauth: None,
            }
        );
    }

    #[test]
    fn reads_a_preregistered_oauth_client_like_claude_code_writes_it() {
        let servers = parse_config(
            r#"{"mcpServers": {"slack": {"type": "http", "url": "https://mcp.slack.com/mcp",
                "oauth": {"clientId": "123.456", "callbackPort": 3118}}}}"#,
        )
        .unwrap();
        let McpServerConfig::Http { oauth, .. } = &servers["slack"] else { panic!("not http") };
        assert_eq!(oauth, &Some(OAuthClient { client_id: "123.456".into(), callback_port: Some(3118) }));
        let entry = config_as_entry_json(&servers["slack"]);
        assert_eq!(entry["oauth"], json!({"clientId": "123.456", "callbackPort": 3118}));
    }

    #[test]
    fn rejects_mixed_transport_fields() {
        let err = parse_config(r#"{"mcpServers": {"fs": {"command": "x", "url": "https://x"}}}"#).unwrap_err();
        assert!(err.contains("need \"type\": \"http\""), "{}", err);
    }

    #[test]
    fn rejects_server_names_that_break_tool_names() {
        assert!(parse_config(r#"{"mcpServers": {"a.b": {"command": "x"}}}"#).is_err());
        assert!(parse_config(r#"{"mcpServers": {"a__b": {"command": "x"}}}"#).is_err());
    }

    #[test]
    fn written_servers_read_back_and_removal_keeps_others() {
        let dir = std::env::temp_dir().join(format!("minerve-mcp-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mcp.json");
        std::fs::write(&path, r#"{"other": 1, "mcpServers": {"keep": {"command": "k"}}}"#).unwrap();
        let stdio = McpServerConfig::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "a b".into()],
            env: BTreeMap::from([("K".into(), "v".into())]),
        };
        let http = McpServerConfig::Http { url: "https://x/mcp".into(), headers: BTreeMap::new(), oauth: None };
        write_server_to_file(&path, "s", &stdio).unwrap();
        write_server_to_file(&path, "h", &http).unwrap();
        let servers = read_config_file(&path).unwrap();
        assert_eq!(servers["s"], stdio);
        assert_eq!(servers["h"], http);

        remove_server_from_file(&path, "s").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"other\": 1"), "{}", text);
        assert_eq!(read_config_file(&path).unwrap().keys().collect::<Vec<_>>(), vec!["h", "keep"]);
        assert!(remove_server_from_file(&path, "s").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_unknown_transport() {
        assert!(parse_config(r#"{"mcpServers": {"a": {"type": "sse", "url": "https://x"}}}"#).is_err());
    }
}

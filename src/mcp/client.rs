//! One connected MCP server, through rmcp: its tool list and tool calls.

use super::config::McpServerConfig;
use super::oauth::FileCredentialStore;
use rmcp::model::{CallToolRequestParams, Tool};
use rmcp::service::{ClientInitializeError, RoleClient, RunningService, ServiceExt};
use rmcp::transport::auth::{AuthClient, AuthorizationManager};
use rmcp::transport::streamable_http_client::{StreamableHttpClientTransportConfig, StreamableHttpError};
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

pub enum ConnectError {
    /// The server answered 401: it needs a browser login. Holds its `WWW-Authenticate` header.
    NeedsLogin { challenge: String },
    Failed(String),
}

impl From<String> for ConnectError {
    fn from(message: String) -> Self {
        ConnectError::Failed(message)
    }
}

pub struct McpServer {
    service: RunningService<RoleClient, ()>,
    pub tools: Vec<Tool>,
}

/// The `WWW-Authenticate` challenge of a 401 answer to `initialize`. The transport error sits
/// in `TransportError.error`, which `source()` does not reach.
fn auth_challenge(error: &ClientInitializeError) -> Option<String> {
    let ClientInitializeError::TransportError { error, .. } = error else {
        return None;
    };
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error.error.as_ref());
    while let Some(error) = current {
        if let Some(http_error) = error.downcast_ref::<StreamableHttpError<reqwest_rmcp::Error>>() {
            return http_error.auth_challenge().map(str::to_string);
        }
        current = error.source();
    }
    None
}

fn header_map(headers: &BTreeMap<String, String>) -> Result<HashMap<http::HeaderName, http::HeaderValue>, String> {
    headers
        .iter()
        .map(|(name, value)| {
            let header_name = http::HeaderName::from_bytes(name.as_bytes()).map_err(|err| format!("header '{}': {}", name, err))?;
            let header_value = http::HeaderValue::from_str(value).map_err(|err| format!("header '{}': {}", name, err))?;
            Ok((header_name, header_value))
        })
        .collect()
}

async fn connect_stdio(command: &str, args: &[String], env: &BTreeMap<String, String>) -> Result<RunningService<RoleClient, ()>, ConnectError> {
    let mut process = tokio::process::Command::new(command);
    process.args(args).envs(env);
    // Its stderr is dropped: inherited, it would draw over the TUI.
    let (transport, _stderr) = TokioChildProcess::builder(process)
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|err| format!("could not start '{}': {}", command, err))?;
    Ok(().serve(transport).await.map_err(|err| err.to_string())?)
}

/// Servers with a configured `Authorization` header use it as is. Others get rmcp's OAuth
/// client, which sends the stored login when there is one; without one, the 401 of a server
/// that needs a login becomes `ConnectError::NeedsLogin`.
async fn connect_http(url: &str, headers: &BTreeMap<String, String>) -> Result<RunningService<RoleClient, ()>, ConnectError> {
    let has_authorization_header = headers.keys().any(|name| name.eq_ignore_ascii_case("authorization"));
    let transport_config = StreamableHttpClientTransportConfig::with_uri(url.to_string()).custom_headers(header_map(headers)?);
    let http_client = reqwest_rmcp::Client::new();
    let result = if has_authorization_header {
        ().serve(StreamableHttpClientTransport::with_client(http_client, transport_config)).await
    } else {
        let mut manager = AuthorizationManager::new(url).await.map_err(|err| err.to_string())?;
        manager.set_credential_store(FileCredentialStore::new(url));
        // Loads the stored login, if any, so requests carry its token.
        manager.initialize_from_store().await.map_err(|err| err.to_string())?;
        let auth_client = AuthClient::new(http_client, manager);
        ().serve(StreamableHttpClientTransport::with_client(auth_client, transport_config)).await
    };
    result.map_err(|err| match auth_challenge(&err) {
        Some(challenge) if !has_authorization_header => ConnectError::NeedsLogin { challenge },
        _ => ConnectError::Failed(err.to_string()),
    })
}

impl McpServer {
    pub async fn connect(config: &McpServerConfig) -> Result<Self, ConnectError> {
        let service = match config {
            McpServerConfig::Stdio { command, args, env } => connect_stdio(command, args, env).await?,
            McpServerConfig::Http { url, headers, .. } => connect_http(url, headers).await?,
        };
        let tools = service.list_all_tools().await.map_err(|err| format!("tools/list: {}", err))?;
        Ok(Self { service, tools })
    }

    /// Runs a tool and returns its output as text for the model.
    /// A tool reporting `isError` still answers Ok: the model reads the error and can retry.
    pub async fn call_tool(&self, tool_name: &str, arguments: serde_json::Map<String, Value>) -> Result<String, String> {
        let params = CallToolRequestParams::new(tool_name.to_string()).with_arguments(arguments);
        let result = self.service.call_tool(params).await.map_err(|err| err.to_string())?;
        let content = serde_json::to_value(&result.content).expect("tool content serializes");
        let text = content_as_text(content.as_array().expect("tool content is a list"));
        Ok(if result.is_error == Some(true) { format!("Error: {}", text) } else { text })
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ToolContent {
    Text { text: String },
    Image { #[serde(rename = "mimeType")] mime_type: String },
    Audio { #[serde(rename = "mimeType")] mime_type: String },
    ResourceLink { uri: String },
    Resource { resource: EmbeddedResource },
}

#[derive(Deserialize)]
struct EmbeddedResource {
    uri: String,
    text: Option<String>,
}

/// Text blocks are kept as is. Minerve only sends text to the model, so other blocks are
/// replaced by a line saying what was left out; block types newer than minerve are passed as JSON.
fn content_as_text(blocks: &[Value]) -> String {
    blocks
        .iter()
        .map(|block| match serde_json::from_value::<ToolContent>(block.clone()) {
            Ok(ToolContent::Text { text }) => text,
            Ok(ToolContent::Image { mime_type }) => format!("[image ({}) not shown]", mime_type),
            Ok(ToolContent::Audio { mime_type }) => format!("[audio ({}) not shown]", mime_type),
            Ok(ToolContent::ResourceLink { uri }) => format!("[resource link: {}]", uri),
            Ok(ToolContent::Resource { resource }) => match &resource.text {
                Some(text) => format!("[resource {}]\n{}", resource.uri, text),
                None => format!("[binary resource {} not shown]", resource.uri),
            },
            Err(_) => block.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn joins_content_blocks() {
        let blocks = json!([
            { "type": "text", "text": "hello" },
            { "type": "image", "data": "AAAA", "mimeType": "image/png" },
            { "type": "resource", "resource": { "uri": "file:///a", "text": "body" } },
            { "type": "future_kind", "x": 1 }
        ]);
        assert_eq!(
            content_as_text(blocks.as_array().unwrap()),
            "hello\n[image (image/png) not shown]\n[resource file:///a]\nbody\n{\"type\":\"future_kind\",\"x\":1}"
        );
    }
}

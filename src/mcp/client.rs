//! One connected MCP server: the `initialize` handshake, its tool list and tool calls.

use super::config::McpServerConfig;
use super::transport::{HttpTransport, StdioTransport, Transport};
use serde::Deserialize;
use serde_json::{json, Value};

/// Latest MCP revision minerve speaks. The server answers with the revision it picked.
const PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug, Clone, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

#[derive(Deserialize)]
struct ToolsListResult {
    tools: Vec<McpTool>,
    #[serde(rename = "nextCursor")]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    protocol_version: String,
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

#[derive(Deserialize)]
struct CallToolResult {
    content: Vec<ToolContent>,
    #[serde(rename = "isError", default)]
    is_error: bool,
}

pub struct McpServer {
    transport: Transport,
    pub tools: Vec<McpTool>,
}

impl McpServer {
    pub async fn connect(config: &McpServerConfig, http_client: reqwest::Client) -> Result<Self, String> {
        let transport = match config {
            McpServerConfig::Stdio { command, args, env } => {
                Transport::Stdio(StdioTransport::spawn(command, args, env)?)
            }
            McpServerConfig::Http { url, headers } => Transport::Http(HttpTransport::new(http_client, url, headers)?),
        };
        let initialize_result = transport
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": "minerve", "version": env!("CARGO_PKG_VERSION") }
                }),
            )
            .await?;
        let initialize_result: InitializeResult = serde_json::from_value(initialize_result)
            .map_err(|err| format!("unexpected initialize answer: {}", err))?;
        transport.set_protocol_version(&initialize_result.protocol_version);
        transport.notify("notifications/initialized", json!({})).await?;

        let tools = list_tools(&transport).await?;
        Ok(Self { transport, tools })
    }

    /// Runs a tool and returns its output as text for the model.
    /// A tool reporting `isError` still answers Ok: the model reads the error and can retry.
    pub async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<String, String> {
        let result = self
            .transport
            .request("tools/call", json!({ "name": tool_name, "arguments": arguments }))
            .await?;
        let result: CallToolResult =
            serde_json::from_value(result).map_err(|err| format!("unexpected tools/call answer: {}", err))?;
        let text = content_as_text(&result.content);
        Ok(if result.is_error { format!("Error: {}", text) } else { text })
    }
}

async fn list_tools(transport: &Transport) -> Result<Vec<McpTool>, String> {
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let params = match &cursor {
            Some(cursor) => json!({ "cursor": cursor }),
            None => json!({}),
        };
        let page: ToolsListResult = serde_json::from_value(transport.request("tools/list", params).await?)
            .map_err(|err| format!("unexpected tools/list answer: {}", err))?;
        tools.extend(page.tools);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(tools),
        }
    }
}

/// Text blocks are kept as is. Minerve only sends text to the model, so other blocks are
/// replaced by a line saying what was left out.
fn content_as_text(content: &[ToolContent]) -> String {
    content
        .iter()
        .map(|block| match block {
            ToolContent::Text { text } => text.clone(),
            ToolContent::Image { mime_type } => format!("[image ({}) not shown]", mime_type),
            ToolContent::Audio { mime_type } => format!("[audio ({}) not shown]", mime_type),
            ToolContent::ResourceLink { uri } => format!("[resource link: {}]", uri),
            ToolContent::Resource { resource } => match &resource.text {
                Some(text) => format!("[resource {}]\n{}", resource.uri, text),
                None => format!("[binary resource {} not shown]", resource.uri),
            },
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_content_blocks() {
        let result: CallToolResult = serde_json::from_value(json!({
            "content": [
                { "type": "text", "text": "hello" },
                { "type": "image", "data": "AAAA", "mimeType": "image/png" },
                { "type": "resource", "resource": { "uri": "file:///a", "text": "body" } }
            ]
        }))
        .unwrap();
        assert_eq!(
            content_as_text(&result.content),
            "hello\n[image (image/png) not shown]\n[resource file:///a]\nbody"
        );
    }
}

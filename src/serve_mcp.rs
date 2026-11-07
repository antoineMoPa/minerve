//! `/mcp` for `--serve`: the same actions as the TUI's MCP manager, as commands.

use crate::control_socket::{ControlSocket, McpServerOut, MinerveEvent, ParentCommand};
use crate::mcp::config::McpServerConfig;
use crate::mcp::{self, McpConnection, McpServerRow};
use crate::mcp_manager::server_config_from_form;

/// True when `command` was an MCP one.
pub fn handle(command: &ParentCommand, control: &ControlSocket) -> bool {
    let outcome: Result<String, String> = match command {
        ParentCommand::ListMcp {} => {
            send_servers(control);
            return true;
        }
        ParentCommand::McpConnect { name } => known(name).map(|()| {
            mcp::connect(name);
            format!("Connecting '{name}'.")
        }),
        ParentCommand::McpDisconnect { name } => known(name).map(|()| {
            mcp::disconnect(name);
            format!("Disconnected '{name}'.")
        }),
        ParentCommand::McpLogin { name } => known(name).map(|()| {
            mcp::log_in(name);
            format!("Logging in to '{name}' in your browser.")
        }),
        ParentCommand::McpRemove { name } => known(name).and_then(|()| mcp::remove_server(name)).map(|()| format!("Removed '{name}'.")),
        ParentCommand::McpAdd { name, command_or_url, env_or_headers } => {
            server_config_from_form(command_or_url, env_or_headers)
                .and_then(|config| mcp::add_server(name.trim(), config))
                .map(|()| format!("Added '{}'.", name.trim()))
        }
        _ => return crate::serve_message_apps::handle(command, control),
    };
    let notice = outcome.unwrap_or_else(|err| err);
    control.send(&MinerveEvent::Notice { notice: &notice });
    send_servers(control);
    true
}

/// The mcp functions panic on a name they do not know, and the parent's word is not checked.
fn known(name: &str) -> Result<(), String> {
    if mcp::server_rows().iter().any(|row| row.name == name) {
        Ok(())
    } else {
        Err(format!("no MCP server named '{name}'"))
    }
}

fn send_servers(control: &ControlSocket) {
    let servers: Vec<McpServerOut> = mcp::server_rows().iter().map(describe).collect();
    control.send(&MinerveEvent::McpServers { servers: &servers });
}

fn describe(row: &McpServerRow) -> McpServerOut {
    let target = match &row.config {
        McpServerConfig::Stdio { command, args, .. } => format!("{command} {}", args.join(" ")).trim_end().to_string(),
        McpServerConfig::Http { url, .. } => url.clone(),
    };
    let (state, tools, detail) = match &row.connection {
        McpConnection::Connecting => ("connecting", 0, String::new()),
        McpConnection::Connected(server) => {
            let skipped = mcp::skipped_tool_names(&row.name, server);
            let detail = if skipped.is_empty() {
                String::new()
            } else {
                format!("Not offered to the model (invalid function names): {}", skipped.join(", "))
            };
            ("connected", server.tools.len() - skipped.len(), detail)
        }
        McpConnection::Failed(err) => ("failed", 0, err.clone()),
        McpConnection::NeedsLogin { .. } => ("needs_login", 0, String::new()),
        McpConnection::LoggingIn { authorize_url } => ("logging_in", 0, authorize_url.clone().unwrap_or_default()),
        McpConnection::Disconnected => ("disconnected", 0, String::new()),
    };
    McpServerOut { name: row.name.clone(), target, source: row.source.label(), state, tools, detail }
}

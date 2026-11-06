//! `/mcp` modal: lists MCP servers with their connection state, connects, disconnects,
//! removes them, and adds new ones to `~/.minerve/mcp.json`.

use std::collections::BTreeMap;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::mcp::config::{ConfigSource, McpServerConfig};
use crate::mcp::{self, McpConnection, McpServerRow};
use crate::setup::centered;

const MODAL_WIDTH: u16 = 100;
const MAX_MODAL_HEIGHT: u16 = 30;
const NAME_COLUMN_WIDTH: usize = 22;

pub enum McpManagerStep {
    Stay,
    Close,
}

#[derive(Clone, Copy, PartialEq)]
enum FormField {
    Name,
    CommandOrUrl,
    EnvOrHeaders,
}

const FORM_FIELDS: [FormField; 3] = [FormField::Name, FormField::CommandOrUrl, FormField::EnvOrHeaders];

#[derive(Default)]
struct AddServerForm {
    name: String,
    command_or_url: String,
    env_or_headers: String,
    field_index: usize,
    error: Option<String>,
}

enum View {
    /// `selected` indexes the server rows; one past the last server is the "add a server" row.
    List { selected: usize, confirm_remove: Option<String> },
    AddForm(AddServerForm),
}

/// Result of the last action, shown under the list.
enum Message {
    Info(String),
    Error(String),
}

pub struct McpManager {
    view: View,
    message: Option<Message>,
}

impl McpManager {
    pub fn new() -> Self {
        Self { view: View::List { selected: 0, confirm_remove: None }, message: None }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> McpManagerStep {
        match &mut self.view {
            View::List { .. } => self.handle_list_key(key),
            View::AddForm(_) => {
                self.handle_form_key(key);
                McpManagerStep::Stay
            }
        }
    }

    pub fn paste(&mut self, text: &str) {
        if let View::AddForm(form) = &mut self.view {
            form.active_text().extend(text.chars().filter(|c| !c.is_control()));
        }
    }

    fn handle_list_key(&mut self, key: KeyEvent) -> McpManagerStep {
        let rows = mcp::server_rows();
        let View::List { selected, confirm_remove } = &mut self.view else {
            unreachable!("list key outside the list view");
        };
        *selected = (*selected).min(rows.len());

        if let Some(name) = confirm_remove.take() {
            if key.code == KeyCode::Char('y') {
                self.message = Some(match mcp::remove_server(&name) {
                    Ok(()) => Message::Info(format!("Removed '{}'.", name)),
                    Err(err) => Message::Error(err),
                });
            } else {
                self.message = Some(Message::Info(String::from("Not removed.")));
            }
            return McpManagerStep::Stay;
        }

        let selected_row = rows.get(*selected);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return McpManagerStep::Close,
            KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => *selected = (*selected + 1).min(rows.len()),
            KeyCode::Char('a') => self.open_form(),
            KeyCode::Enter | KeyCode::Char(' ') => match selected_row {
                None => self.open_form(),
                Some(row) => match row.connection {
                    McpConnection::Connected(_) | McpConnection::Connecting | McpConnection::LoggingIn { .. } => {
                        mcp::disconnect(&row.name);
                        self.message = Some(Message::Info(format!("Disconnected '{}'.", row.name)));
                    }
                    McpConnection::NeedsLogin { .. } => {
                        mcp::log_in(&row.name);
                        self.message = None;
                    }
                    McpConnection::Failed(_) | McpConnection::Disconnected => {
                        mcp::connect(&row.name);
                        self.message = None;
                    }
                },
            },
            KeyCode::Char('r') => {
                if let Some(row) = selected_row {
                    mcp::connect(&row.name);
                    self.message = None;
                }
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                if let Some(row) = selected_row {
                    *confirm_remove = Some(row.name.clone());
                }
            }
            _ => {}
        }
        McpManagerStep::Stay
    }

    fn open_form(&mut self) {
        self.view = View::AddForm(AddServerForm::default());
        self.message = None;
    }

    fn handle_form_key(&mut self, key: KeyEvent) {
        let View::AddForm(form) = &mut self.view else {
            unreachable!("form key outside the form view");
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let last_field = form.field_index == FORM_FIELDS.len() - 1;
        match key.code {
            KeyCode::Esc => self.view = View::List { selected: 0, confirm_remove: None },
            KeyCode::Enter if last_field => self.save_form(),
            KeyCode::Tab | KeyCode::Down | KeyCode::Enter => form.field_index = (form.field_index + 1) % FORM_FIELDS.len(),
            KeyCode::BackTab | KeyCode::Up => {
                form.field_index = (form.field_index + FORM_FIELDS.len() - 1) % FORM_FIELDS.len()
            }
            KeyCode::Backspace => {
                form.active_text().pop();
            }
            KeyCode::Char(c) if !ctrl => form.active_text().push(c),
            _ => {}
        }
    }

    fn save_form(&mut self) {
        let View::AddForm(form) = &mut self.view else {
            unreachable!("save outside the form view");
        };
        let name = form.name.trim().to_string();
        let added = server_config_from_form(&form.command_or_url, &form.env_or_headers)
            .and_then(|config| mcp::add_server(&name, config));
        if let Err(err) = added {
            form.error = Some(err);
            return;
        }
        let selected = mcp::server_rows()
            .iter()
            .position(|row| row.name == name)
            .expect("added server is listed");
        self.view = View::List { selected, confirm_remove: None };
        self.message = Some(Message::Info(format!(
            "Added '{}' to {}.",
            name,
            ConfigSource::User.label()
        )));
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        match &self.view {
            View::List { selected, confirm_remove } => self.draw_list(frame, area, *selected, confirm_remove.as_deref()),
            View::AddForm(form) => draw_form(frame, area, form),
        }
    }

    fn draw_list(&self, frame: &mut Frame, area: Rect, selected: usize, confirm_remove: Option<&str>) {
        let rows = mcp::server_rows();
        let selected = selected.min(rows.len());
        let dim = Style::default().fg(Color::DarkGray);

        let mut lines = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            lines.push(server_line(row, index == selected));
        }
        let add_marker = if selected == rows.len() { "> " } else { "  " };
        lines.push(Line::styled(format!("{}+ Add a server", add_marker), Style::default().fg(Color::Cyan)));
        lines.push(Line::from(""));

        if let Some(row) = rows.get(selected) {
            lines.extend(server_details(row));
            lines.push(Line::from(""));
        }
        match &self.message {
            Some(Message::Info(text)) => lines.push(Line::styled(text.clone(), Style::default().fg(Color::Green))),
            Some(Message::Error(text)) => lines.push(Line::styled(text.clone(), Style::default().fg(Color::Red))),
            None => {}
        }

        let hint = match confirm_remove {
            Some(name) => {
                let source = rows.iter().find(|row| row.name == name).map(|row| row.source.label()).unwrap_or_default();
                Line::styled(
                    format!("Remove '{}' from {}? y to confirm, any other key cancels.", name, source),
                    Style::default().fg(Color::Yellow),
                )
            }
            None => Line::styled(
                "↑↓ select | Enter connect/disconnect | r reconnect | a add | x remove | Esc close",
                dim,
            ),
        };
        let inner = draw_modal_frame(frame, area, " MCP servers ", &lines);
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        let hint_area = Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1);
        frame.render_widget(Paragraph::new(hint), hint_area);
    }
}

/// Draws the border and returns the inside. The modal is as tall as `lines` once wrapped,
/// plus a blank row and the hint row.
pub(crate) fn draw_modal_frame(frame: &mut Frame, area: Rect, title: &str, lines: &[Line]) -> Rect {
    let inner_width = MODAL_WIDTH.min(area.width).saturating_sub(2).max(1) as usize;
    let wrapped_rows: usize = lines.iter().map(|line| line.width().max(1).div_ceil(inner_width)).sum();
    let height = (wrapped_rows as u16 + 4).min(MAX_MODAL_HEIGHT);
    let modal = centered(area, MODAL_WIDTH, height);
    frame.render_widget(Clear, modal);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title.to_string())
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(modal);
    frame.render_widget(block, modal);
    inner
}

fn connection_label(name: &str, connection: &McpConnection) -> (String, Color) {
    match connection {
        McpConnection::Connecting => (String::from("connecting..."), Color::Yellow),
        McpConnection::Connected(server) => {
            let tool_count = server.tools.len() - mcp::skipped_tool_names(name, server).len();
            (format!("connected, {} tools", tool_count), Color::Green)
        }
        McpConnection::Failed(_) => (String::from("failed"), Color::Red),
        McpConnection::NeedsLogin { .. } => (String::from("needs login"), Color::Yellow),
        McpConnection::LoggingIn { .. } => (String::from("logging in..."), Color::Yellow),
        McpConnection::Disconnected => (String::from("disconnected"), Color::DarkGray),
    }
}

fn server_line(row: &McpServerRow, is_selected: bool) -> Line<'static> {
    let marker = if is_selected { "> " } else { "  " };
    let (label, color) = connection_label(&row.name, &row.connection);
    let name_style = if is_selected { Style::default().add_modifier(Modifier::BOLD) } else { Style::default() };
    Line::from(vec![
        Span::raw(marker),
        Span::styled(format!("{:<width$}", row.name, width = NAME_COLUMN_WIDTH), name_style),
        Span::styled(format!("{:<24}", label), Style::default().fg(color)),
        Span::styled(row.source.label(), Style::default().fg(Color::DarkGray)),
    ])
}

/// What the server runs or calls, then its connection error or tools left out of requests.
fn server_details(row: &McpServerRow) -> Vec<Line<'static>> {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let target = match &row.config {
        McpServerConfig::Stdio { command, args, .. } => {
            format!("Command: {} {}", command, args.join(" ")).trim_end().to_string()
        }
        McpServerConfig::Http { url, .. } => format!("URL: {}", url),
    };
    let mut lines = vec![Line::styled(target, bold)];
    match &row.connection {
        McpConnection::Connected(server) => {
            let skipped = mcp::skipped_tool_names(&row.name, server);
            if !skipped.is_empty() {
                lines.push(Line::styled(
                    format!("Not offered to the model (invalid function names): {}", skipped.join(", ")),
                    Style::default().fg(Color::Yellow),
                ));
            }
        }
        McpConnection::Failed(err) => lines.push(Line::styled(err.clone(), Style::default().fg(Color::Red))),
        McpConnection::NeedsLogin { .. } => {
            lines.push(Line::styled("Press Enter to log in with your browser.", Style::default().fg(Color::Yellow)))
        }
        McpConnection::LoggingIn { authorize_url: None } => lines.push(Line::from("Preparing the login...")),
        McpConnection::LoggingIn { authorize_url: Some(url) } => {
            lines.push(Line::from("Waiting for the browser login. If no browser opened, visit:"));
            lines.push(Line::from(url.clone()));
        }
        McpConnection::Connecting | McpConnection::Disconnected => {}
    }
    lines
}

impl AddServerForm {
    fn text(&self, field: FormField) -> &str {
        match field {
            FormField::Name => &self.name,
            FormField::CommandOrUrl => &self.command_or_url,
            FormField::EnvOrHeaders => &self.env_or_headers,
        }
    }

    fn active_text(&mut self) -> &mut String {
        match FORM_FIELDS[self.field_index] {
            FormField::Name => &mut self.name,
            FormField::CommandOrUrl => &mut self.command_or_url,
            FormField::EnvOrHeaders => &mut self.env_or_headers,
        }
    }
}

fn form_field_label(field: FormField) -> &'static str {
    match field {
        FormField::Name => "Name",
        FormField::CommandOrUrl => "Command or URL",
        FormField::EnvOrHeaders => "Env or headers",
    }
}

fn draw_form(frame: &mut Frame, area: Rect, form: &AddServerForm) {
    let label = Style::default().add_modifier(Modifier::BOLD);
    let mut lines = Vec::new();
    for (index, field) in FORM_FIELDS.iter().enumerate() {
        lines.push(Line::styled(form_field_label(*field), label));
        let marker = if index == form.field_index { "> " } else { "  " };
        lines.push(Line::from(format!("{}{}", marker, form.text(*field))));
        lines.push(Line::from(""));
    }
    lines.push(Line::styled(form.error.clone().unwrap_or_default(), Style::default().fg(Color::Red)));
    let inner = draw_modal_frame(frame, area, " Add MCP server ", &lines);
    frame.render_widget(Paragraph::new(lines), inner);
    let hint_area = Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1);
    frame.render_widget(
        Paragraph::new(Line::styled(
            "Tab/arrows move | Enter on last field saves | Esc back",
            Style::default().fg(Color::DarkGray),
        )),
        hint_area,
    );

    // Field rows: label at 3i, value right under it.
    let row = 1 + 3 * form.field_index as u16;
    let text_len = form.text(FORM_FIELDS[form.field_index]).chars().count() as u16;
    let cursor_x = (inner.x + 2 + text_len).min(inner.right().saturating_sub(1));
    frame.set_cursor_position((cursor_x, inner.y + row));
}

/// A URL makes an HTTP server whose pairs are headers; anything else is a command line whose
/// pairs are environment variables.
fn server_config_from_form(command_or_url: &str, env_or_headers: &str) -> Result<McpServerConfig, String> {
    let command_or_url = command_or_url.trim();
    if command_or_url.is_empty() {
        return Err(String::from("Enter a command or a URL."));
    }
    let pairs = parse_pairs(env_or_headers)?;
    if command_or_url.starts_with("http://") || command_or_url.starts_with("https://") {
        if command_or_url.contains(char::is_whitespace) {
            return Err(String::from("A URL cannot contain spaces."));
        }
        return Ok(McpServerConfig::Http { url: command_or_url.to_string(), headers: pairs, oauth: None });
    }
    let mut words = split_command_line(command_or_url)?.into_iter();
    let command = words.next().expect("non-empty command line has a first word");
    Ok(McpServerConfig::Stdio { command, args: words.collect(), env: pairs })
}

/// `KEY=value, KEY2=value 2` into a map. Values may hold spaces and `=`, not commas.
fn parse_pairs(text: &str) -> Result<BTreeMap<String, String>, String> {
    text.split(',')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| format!("'{}' is not KEY=value.", pair))?;
            let key = key.trim();
            if key.is_empty() {
                return Err(format!("'{}' has no name before '='.", pair));
            }
            Ok((key.to_string(), value.trim().to_string()))
        })
        .collect()
}

/// Splits a command line into words like a shell would for plain arguments: whitespace
/// separates words, single and double quotes group them, a backslash escapes the next char
/// outside single quotes. No variables, globs or pipes.
fn split_command_line(text: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (Some('"') | None, '\\') => {
                word.push(chars.next().ok_or("The command ends with a backslash.")?);
                in_word = true;
            }
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err(String::from("The command has an unclosed quote."));
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_command_lines_with_quotes() {
        assert_eq!(
            split_command_line(r#"npx -y "@scope/server name" 'a b' c\ d"#).unwrap(),
            vec!["npx", "-y", "@scope/server name", "a b", "c d"]
        );
        assert_eq!(split_command_line(r#"x "" y"#).unwrap(), vec!["x", "", "y"]);
        assert!(split_command_line(r#"x "open"#).is_err());
    }

    #[test]
    fn url_makes_http_server_with_headers() {
        assert_eq!(
            server_config_from_form(" https://x/mcp ", "Authorization=Bearer abc").unwrap(),
            McpServerConfig::Http {
                url: "https://x/mcp".into(),
                headers: BTreeMap::from([("Authorization".into(), "Bearer abc".into())]),
                oauth: None,
            }
        );
    }

    #[test]
    fn command_makes_stdio_server_with_env() {
        assert_eq!(
            server_config_from_form("npx -y server", "A=1, B = x=y").unwrap(),
            McpServerConfig::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "server".into()],
                env: BTreeMap::from([("A".into(), "1".into()), ("B".into(), "x=y".into())]),
            }
        );
    }

    #[test]
    fn rejects_bad_form_input() {
        assert!(server_config_from_form("  ", "").is_err());
        assert!(server_config_from_form("npx", "NOVALUE").is_err());
        assert!(server_config_from_form("https://x/ y", "").is_err());
    }
}

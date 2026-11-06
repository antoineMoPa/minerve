//! `/message-apps` modal: sets up Slack, shows the gateway's state, and links your chat account
//! with a one-time code. Settings go to `~/.minerve/message-apps.toml`, tokens to `keys.json`.

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Instant;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;

use minerve_gateway::engine::config::{self, MessageAppsConfig, SlackSettings};
use minerve_gateway::engine::status::{Connection, LinkState};
use minerve_gateway::transport::slack;

use crate::chat_selection::{copy_with_system_tool, osc52_copy};
use crate::mcp_manager::draw_modal_frame;
use crate::message_apps_runner::{self as runner, RunnerView};

const TRANSPORT: &str = "slack";
const LABEL_WIDTH: usize = 17;
const MANIFEST_FILE: &str = ".minerve/slack-manifest.json";
const SLACK_APPS_URL: &str = "https://api.slack.com/apps";

pub enum MessageAppsStep {
    Stay,
    Close,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Field {
    BotName,
    SlackApp,
    AppToken,
    BotToken,
}

const FIELDS: [Field; 4] = [Field::BotName, Field::SlackApp, Field::AppToken, Field::BotToken];

#[derive(Default)]
struct SetupForm {
    bot_name: String,
    app_token: String,
    bot_token: String,
    field_index: usize,
    /// A token is already saved: a blank field keeps it.
    has_app_token: bool,
    has_bot_token: bool,
}

enum Note {
    Info(String),
    Error(String),
}

enum View {
    List,
    Form(SetupForm),
    Link,
    /// The manifest on its own screen, to select by hand when copying did not reach the clipboard.
    /// Keeps the form to go back to.
    Manifest { form: SetupForm, text: String, scroll: u16 },
}

/// Slack's row in the list, from the config and the running gateway.
#[derive(Debug, PartialEq)]
enum SlackRow {
    NotSetUp,
    Disconnected,
    Connecting,
    Connected { workspace: String, bot: String },
    Failed(String),
}

pub struct MessageApps {
    view: View,
    note: Option<Note>,
    config: MessageAppsConfig,
    has_tokens: bool,
}

impl MessageApps {
    pub fn new() -> Self {
        let mut apps = Self { view: View::List, note: None, config: MessageAppsConfig::default(), has_tokens: false };
        apps.reload();
        apps
    }

    fn reload(&mut self) {
        match config::load() {
            Ok(config) => self.config = config,
            Err(err) => self.note = Some(Note::Error(err)),
        }
        self.has_tokens = config::load_slack_tokens().is_some();
    }

    fn row(&self, view: &RunnerView) -> SlackRow {
        if self.config.slack.is_none() || !self.has_tokens {
            return SlackRow::NotSetUp;
        }
        match view {
            RunnerView::Stopped => SlackRow::Disconnected,
            RunnerView::Blocked(err) => SlackRow::Failed(err.clone()),
            RunnerView::Running { status, .. } => match status.connections.get(TRANSPORT) {
                None | Some(Connection::Connecting) => SlackRow::Connecting,
                Some(Connection::Connected { workspace, bot }) => {
                    SlackRow::Connected { workspace: workspace.clone(), bot: bot.clone() }
                }
                Some(Connection::Failed(err)) => SlackRow::Failed(err.clone()),
            },
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> MessageAppsStep {
        match self.view {
            View::List => return self.handle_list_key(key),
            View::Form(_) => self.handle_form_key(key),
            View::Manifest { .. } => self.handle_manifest_key(key),
            View::Link => self.handle_link_key(key),
        }
        MessageAppsStep::Stay
    }

    pub fn paste(&mut self, text: &str) {
        if let View::Form(form) = &mut self.view {
            if let Some(field) = form.active_text() {
                field.extend(text.chars().filter(|c| !c.is_control()));
            }
        }
    }

    fn handle_list_key(&mut self, key: KeyEvent) -> MessageAppsStep {
        let row = self.row(&runner::view());
        match (key.code, &row) {
            (KeyCode::Esc | KeyCode::Char('q'), _) => return MessageAppsStep::Close,
            (KeyCode::Enter, SlackRow::NotSetUp) | (KeyCode::Char('e'), _) => self.open_form(),
            (KeyCode::Enter, SlackRow::Disconnected | SlackRow::Failed(_)) => {
                runner::start();
                self.note = None;
            }
            (KeyCode::Char('l'), SlackRow::Connected { .. } | SlackRow::Connecting) => self.open_link(),
            (KeyCode::Char('d'), SlackRow::Connected { .. } | SlackRow::Connecting) => {
                runner::stop();
                self.note = Some(Note::Info(String::from("Disconnected.")));
            }
            _ => {}
        }
        MessageAppsStep::Stay
    }

    fn open_form(&mut self) {
        let tokens = config::load_slack_tokens();
        let bot_name = self.config.slack.as_ref().map(|slack| slack.bot_name.clone()).unwrap_or_default();
        self.view = View::Form(SetupForm {
            bot_name,
            has_app_token: tokens.is_some(),
            has_bot_token: tokens.is_some(),
            ..SetupForm::default()
        });
        self.note = None;
    }

    fn open_link(&mut self) {
        match runner::start_link() {
            Some(_) => {
                self.view = View::Link;
                self.note = None;
            }
            None => self.note = Some(Note::Error(String::from("Not running."))),
        }
    }

    fn handle_form_key(&mut self, key: KeyEvent) {
        let View::Form(form) = &mut self.view else {
            unreachable!("form key outside the form view");
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let on_slack_app = FIELDS[form.field_index] == Field::SlackApp;
        match key.code {
            KeyCode::Esc => {
                self.view = View::List;
                self.note = None;
            }
            KeyCode::Tab | KeyCode::Down => form.field_index = (form.field_index + 1) % FIELDS.len(),
            KeyCode::BackTab | KeyCode::Up => form.field_index = (form.field_index + FIELDS.len() - 1) % FIELDS.len(),
            KeyCode::Enter | KeyCode::Char('c') if on_slack_app => self.copy_manifest(),
            KeyCode::Char('v') if on_slack_app => self.view_manifest(),
            KeyCode::Enter => self.save_form(),
            KeyCode::Backspace => {
                if let Some(field) = form.active_text() {
                    field.pop();
                }
            }
            KeyCode::Char(c) if !ctrl => {
                if let Some(field) = form.active_text() {
                    field.push(c);
                }
            }
            _ => {}
        }
    }

    fn view_manifest(&mut self) {
        let View::Form(form) = &self.view else { return };
        let name = form.bot_name.trim().to_string();
        if let Err(err) = check_bot_name(&name) {
            self.note = Some(Note::Error(err));
            return;
        }
        let View::Form(form) = std::mem::replace(&mut self.view, View::List) else { unreachable!() };
        self.view = View::Manifest { form, text: slack::manifest(&name), scroll: 0 };
        // Let the terminal select text with the mouse while the manifest shows.
        crate::tui::set_mouse_capture(false);
    }

    fn handle_manifest_key(&mut self, key: KeyEvent) {
        let View::Manifest { scroll, text, .. } = &mut self.view else { return };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => *scroll = (*scroll + 1).min(text.lines().count() as u16),
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('v') => {
                let View::Manifest { form, .. } = std::mem::replace(&mut self.view, View::List) else { unreachable!() };
                self.view = View::Form(form);
                crate::tui::set_mouse_capture(true);
            }
            _ => {}
        }
    }

    fn copy_manifest(&mut self) {
        let View::Form(form) = &self.view else { return };
        let name = form.bot_name.trim();
        if let Err(err) = check_bot_name(name) {
            self.note = Some(Note::Error(err));
            return;
        }
        let manifest = slack::manifest(name);
        copy_to_clipboard(&manifest);
        let path = dirs::home_dir().expect("no home directory").join(MANIFEST_FILE);
        let saved = std::fs::write(&path, &manifest).is_ok();
        crate::mcp::oauth::open_in_browser(SLACK_APPS_URL);
        // The URL stays in the note: the way in when no browser opened.
        self.note = Some(Note::Info(if saved {
            format!("Manifest copied (also in ~/{}). Opened {}", MANIFEST_FILE, SLACK_APPS_URL)
        } else {
            format!("Manifest copied. Opened {}", SLACK_APPS_URL)
        }));
    }

    fn save_form(&mut self) {
        let View::Form(form) = &self.view else { return };
        let checked = match check_form(form) {
            Ok(checked) => checked,
            Err(err) => {
                self.note = Some(Note::Error(err));
                return;
            }
        };
        // From disk, not the copy loaded when the modal opened: someone may have linked since.
        let mut config = match config::load() {
            Ok(config) => config,
            Err(err) => {
                self.note = Some(Note::Error(err));
                return;
            }
        };
        // The form has no icon field: keep an icon set by hand in the file.
        let icon_url = config.slack.as_ref().and_then(|slack| slack.icon_url.clone());
        config.slack = Some(SlackSettings { icon_url, ..checked.settings });
        let saved = config::save(&config)
            .and_then(|()| config::save_slack_tokens(checked.app_token.as_deref(), checked.bot_token.as_deref()));
        if let Err(err) = saved {
            self.note = Some(Note::Error(err));
            return;
        }
        self.reload();
        runner::start();
        let linked = self.config.allowed.iter().any(|entry| entry.starts_with("slack:"));
        if !linked && runner::start_link().is_some() {
            self.view = View::Link;
            self.note = None;
        } else {
            self.view = View::List;
            self.note = Some(Note::Info(String::from("Saved.")));
        }
    }

    fn handle_link_key(&mut self, key: KeyEvent) {
        let link = match runner::view() {
            RunnerView::Running { status, .. } => status.link,
            _ => None,
        };
        match (key.code, link) {
            (KeyCode::Enter | KeyCode::Esc, Some(LinkState::Linked { .. })) => {
                self.view = View::List;
                self.reload();
            }
            (KeyCode::Esc, _) => {
                runner::cancel_link();
                self.view = View::List;
            }
            (KeyCode::Char('l'), Some(LinkState::Waiting { expires_at, .. })) if Instant::now() >= expires_at => {
                self.open_link()
            }
            (KeyCode::Char('c'), Some(LinkState::Waiting { code, expires_at })) if Instant::now() < expires_at => {
                copy_to_clipboard(&code);
                self.note = Some(Note::Info(String::from("Code copied.")));
            }
            _ => {}
        }
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        let view = runner::view();
        match &self.view {
            View::List => self.draw_list(frame, area, &view),
            View::Form(form) => self.draw_form(frame, area, form),
            View::Manifest { text, scroll, .. } => draw_manifest(frame, area, text, *scroll),
            View::Link => self.draw_link(frame, area, &view),
        }
    }

    fn draw_list(&self, frame: &mut Frame, area: Rect, view: &RunnerView) {
        let dim = Style::default().fg(Color::DarkGray);
        let row = self.row(view);
        let (label, color, detail) = match &row {
            SlackRow::NotSetUp => ("not set up", Color::DarkGray, String::new()),
            SlackRow::Disconnected => ("disconnected", Color::DarkGray, String::new()),
            SlackRow::Connecting => ("connecting...", Color::Yellow, String::new()),
            SlackRow::Connected { workspace, bot } => ("connected", Color::Green, format!("{} · @{}", workspace, bot)),
            SlackRow::Failed(_) => ("failed", Color::Red, String::new()),
        };
        let mut lines = vec![
            Line::from(vec![
                Span::raw("> "),
                Span::styled("slack   ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(label, Style::default().fg(color)),
                Span::styled(if detail.is_empty() { String::new() } else { format!("   {}", detail) }, dim),
            ]),
            Line::from(""),
        ];
        if let SlackRow::Failed(err) = &row {
            lines.push(Line::styled(err.clone(), Style::default().fg(Color::Red)));
            lines.push(Line::from(""));
        }
        if self.config.slack.is_some() && row != SlackRow::NotSetUp {
            let allowed = match view {
                RunnerView::Running { allowed, .. } => slack_members(allowed),
                _ => slack_members(&self.config.allowed),
            };
            let nobody = match row {
                SlackRow::Connected { .. } => "nobody yet · l link your account",
                _ => "nobody yet",
            };
            lines.push(match allowed.is_empty() {
                true => Line::from(vec![Span::raw("  Allowed   "), Span::styled(nobody, dim)]),
                false => Line::from(format!("  Allowed   {}", allowed.join(", "))),
            });
            if let RunnerView::Running { workdir, .. } = view {
                lines.push(Line::from(format!("  Workdir   {}", tilde_path(workdir))));
            }
            if let (RunnerView::Running { status, .. }, SlackRow::Connected { .. }) = (view, &row) {
                lines.push(Line::from(format!("  Served    {} conversations", status.conversations)));
            }
            lines.push(Line::from(""));
        }
        push_note(&mut lines, &self.note);
        let hint = match row {
            SlackRow::NotSetUp => "Enter set up | Esc close",
            SlackRow::Connected { .. } | SlackRow::Connecting => "e edit | l link an account | d disconnect | Esc close",
            SlackRow::Disconnected => "e edit | Enter connect | Esc close",
            SlackRow::Failed(_) => "e edit | Enter retry | Esc close",
        };
        draw_with_hint(frame, area, " Message apps ", lines, hint);
    }

    fn draw_form(&self, frame: &mut Frame, area: Rect, form: &SetupForm) {
        let dim = Style::default().fg(Color::DarkGray);
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let label = |text: &str| Span::styled(format!("{:<width$}", text, width = LABEL_WIDTH), bold);
        let token = |typed: &str, saved: bool, placeholder: &'static str| match (typed.is_empty(), saved) {
            (false, _) => Span::raw(mask_token(typed)),
            (true, true) => Span::styled("saved · type to replace", dim),
            (true, false) => Span::styled(placeholder, dim),
        };
        let on_slack_app = FIELDS[form.field_index] == Field::SlackApp;
        let copy_style = match on_slack_app {
            true => Style::default().fg(Color::Black).bg(Color::Cyan),
            false => Style::default().fg(Color::Cyan),
        };
        let mut lines = vec![
            Line::from(vec![label("Bot name"), Span::raw(form.bot_name.clone())]),
            Line::from(vec![
                label("Slack app"),
                Span::styled("c copy manifest and open Slack", copy_style),
                Span::styled("  then Create New App → From a manifest", dim),
            ]),
            // Slack's own names, and where its pages show each token.
            Line::from(vec![
                label("App-Level Token"),
                token(&form.app_token, form.has_app_token, "xapp-…  Basic Information → App-Level Tokens, scope connections:write"),
            ]),
            Line::from(vec![
                label("Bot OAuth Token"),
                token(&form.bot_token, form.has_bot_token, "xoxb-…  Install App → Bot User OAuth Token"),
            ]),
            Line::from(""),
        ];
        push_note(&mut lines, &self.note);
        let hint = match on_slack_app {
            true => "Tab next | c copy manifest and open Slack | v view manifest | Esc back",
            false => "Tab next | Enter save | Esc back",
        };
        let inner = draw_with_hint(frame, area, " Set up slack ", lines, hint);
        if !on_slack_app {
            let shown = match FIELDS[form.field_index] {
                Field::BotName => form.bot_name.chars().count(),
                Field::AppToken => mask_token(&form.app_token).chars().count(),
                Field::BotToken => mask_token(&form.bot_token).chars().count(),
                Field::SlackApp => 0,
            } as u16;
            let x = (inner.x + LABEL_WIDTH as u16 + shown).min(inner.right().saturating_sub(1));
            frame.set_cursor_position((x, inner.y + form.field_index as u16));
        }
    }

    fn draw_link(&self, frame: &mut Frame, area: Rect, view: &RunnerView) {
        let dim = Style::default().fg(Color::DarkGray);
        let (link, bot) = match view {
            RunnerView::Running { status, .. } => {
                let bot = match status.connections.get(TRANSPORT) {
                    Some(Connection::Connected { bot, .. }) => bot.clone(),
                    _ => self.config.slack.as_ref().map(|slack| slack.bot_name.clone()).unwrap_or_default(),
                };
                (status.link.clone(), bot)
            }
            _ => (None, String::new()),
        };
        // A code is no use while Slack is down: say why instead.
        if let SlackRow::Failed(err) = self.row(view) {
            let lines = vec![Line::styled(format!("Slack is not connected: {}", err), Style::default().fg(Color::Red)), Line::from("")];
            draw_with_hint(frame, area, " Link your slack account ", lines, "Esc back");
            return;
        }
        let waiting = match self.row(view) {
            SlackRow::Connected { .. } => "Waiting…",
            _ => "Connecting to Slack…",
        };
        let now = Instant::now();
        let (lines, hint) = match link {
            Some(LinkState::Waiting { code, expires_at }) if now < expires_at => {
                let left = expires_at.duration_since(now).as_secs();
                let lines = vec![
                    Line::from(vec![
                        Span::raw("Send this to "),
                        Span::styled(format!("@{}", bot), Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw(" in a DM:"),
                    ]),
                    Line::from(""),
                    Line::from(vec![
                        Span::raw("      "),
                        Span::styled(code, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                        Span::styled(format!("      expires in {}:{:02}", left / 60, left % 60), dim),
                    ]),
                    Line::from(""),
                    Line::styled(waiting, dim),
                    Line::from(""),
                ];
                let mut lines = lines;
                push_note(&mut lines, &self.note);
                (lines, "c copy code | Esc cancel")
            }
            Some(LinkState::Linked { member }) => {
                let lines = vec![
                    Line::from(vec![
                        Span::styled(format!("Linked to {}.", member), Style::default().fg(Color::Green)),
                        Span::styled(" The code is used up.", dim),
                    ]),
                    Line::from(""),
                ];
                (lines, "Enter done")
            }
            _ => (vec![Line::styled("The code expired.", Style::default().fg(Color::Yellow)), Line::from("")], "l new code | Esc back"),
        };
        draw_with_hint(frame, area, " Link your slack account ", lines, hint);
    }
}

impl SetupForm {
    /// The text the cursor edits; `None` on the "copy manifest" row.
    fn active_text(&mut self) -> Option<&mut String> {
        match FIELDS[self.field_index] {
            Field::BotName => Some(&mut self.bot_name),
            Field::SlackApp => None,
            Field::AppToken => Some(&mut self.app_token),
            Field::BotToken => Some(&mut self.bot_token),
        }
    }
}

struct CheckedForm {
    settings: SlackSettings,
    /// `None` keeps the saved token.
    app_token: Option<String>,
    bot_token: Option<String>,
}

fn check_form(form: &SetupForm) -> Result<CheckedForm, String> {
    let bot_name = form.bot_name.trim();
    check_bot_name(bot_name)?;
    let app_token = check_token(&form.app_token, form.has_app_token, "xapp-", "app")?;
    let bot_token = check_token(&form.bot_token, form.has_bot_token, "xoxb-", "bot")?;
    Ok(CheckedForm { settings: SlackSettings { bot_name: bot_name.to_string(), icon_url: None }, app_token, bot_token })
}

/// Slack's rule for a bot's display name, so the manifest is accepted.
fn check_bot_name(name: &str) -> Result<(), String> {
    let valid = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.';
    if name.is_empty() || !name.chars().all(valid) {
        return Err(String::from("Bot name: lowercase letters, digits, - _ . only, e.g. antoine-bot."));
    }
    Ok(())
}

fn check_token(typed: &str, saved: bool, prefix: &str, kind: &str) -> Result<Option<String>, String> {
    let typed = typed.trim();
    match (typed.is_empty(), saved) {
        (true, true) => Ok(None),
        (true, false) => Err(format!("Paste the {} token ({}…).", kind, prefix)),
        (false, _) if !typed.starts_with(prefix) => Err(format!("The {} token starts with {}", kind, prefix)),
        (false, _) => Ok(Some(typed.to_string())),
    }
}

/// The first 10 chars, then dots: enough to tell tokens apart, not to reuse one off the screen.
fn mask_token(token: &str) -> String {
    let shown: String = token.chars().take(10).collect();
    let hidden = token.chars().count().saturating_sub(10).min(20);
    format!("{}{}", shown, "•".repeat(hidden))
}

fn slack_members(allowed: &[String]) -> Vec<String> {
    allowed.iter().filter_map(|entry| entry.strip_prefix("slack:")).map(String::from).collect()
}

pub(crate) fn tilde_path(path: &std::path::Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(PathBuf::from)) {
        Some(rest) if rest.as_os_str().is_empty() => String::from("~"),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

fn push_note(lines: &mut Vec<Line<'static>>, note: &Option<Note>) {
    match note {
        Some(Note::Info(text)) => lines.push(Line::styled(text.clone(), Style::default().fg(Color::Green))),
        Some(Note::Error(text)) => lines.push(Line::styled(text.clone(), Style::default().fg(Color::Red))),
        None => {}
    }
}

/// The OS clipboard tool first; OSC 52 (written straight to the terminal) when there is none.
fn copy_to_clipboard(text: &str) {
    if !copy_with_system_tool(text) {
        let _ = io::stdout().write_all(osc52_copy(text).as_bytes());
    }
}

/// The manifest alone on the screen, no border, so a mouse selection takes only its lines.
fn draw_manifest(frame: &mut Frame, area: Rect, text: &str, scroll: u16) {
    let dim = Style::default().fg(Color::DarkGray);
    frame.render_widget(Clear, area);
    let total = text.lines().count() as u16;
    let body = Rect::new(area.x, area.y + 2, area.width, area.height.saturating_sub(4));
    let scroll = scroll.min(total.saturating_sub(body.height));
    let header = format!("Slack manifest · select it with the mouse · also in ~/{}", MANIFEST_FILE);
    frame.render_widget(Paragraph::new(Line::styled(header, dim)), Rect::new(area.x, area.y, area.width, 1));
    frame.render_widget(Paragraph::new(text.to_string()).scroll((scroll, 0)), body);
    let hint_area = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
    frame.render_widget(Paragraph::new(Line::styled("↑↓ scroll | Esc back", dim)), hint_area);
}

/// Draws the modal with `hint` on its last row; returns the inside.
fn draw_with_hint(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>, hint: &str) -> Rect {
    let inner = draw_modal_frame(frame, area, title, &lines);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    let hint_area = Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1);
    frame.render_widget(Paragraph::new(Line::styled(hint.to_string(), Style::default().fg(Color::DarkGray))), hint_area);
    inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> SetupForm {
        SetupForm {
            bot_name: "antoine-bot".into(),
            app_token: "xapp-1-A".into(),
            bot_token: "xoxb-2".into(),
            ..SetupForm::default()
        }
    }

    #[test]
    fn a_complete_form_checks_out() {
        let checked = check_form(&form()).unwrap();
        assert_eq!(checked.settings.bot_name, "antoine-bot");
        assert_eq!(checked.app_token.as_deref(), Some("xapp-1-A"));
    }

    #[test]
    fn blank_tokens_keep_saved_ones_and_are_required_otherwise() {
        let mut blank = SetupForm { app_token: String::new(), bot_token: String::new(), ..form() };
        assert!(check_form(&blank).is_err());
        blank.has_app_token = true;
        blank.has_bot_token = true;
        let checked = check_form(&blank).unwrap();
        assert_eq!((checked.app_token, checked.bot_token), (None, None));
    }

    #[test]
    fn rejects_swapped_tokens_and_bad_names() {
        assert!(check_form(&SetupForm { app_token: "xoxb-2".into(), ..form() }).is_err());
        assert!(check_form(&SetupForm { bot_name: "Antoine Bot".into(), ..form() }).is_err());
    }

    #[test]
    fn tokens_are_masked_after_ten_chars() {
        assert_eq!(mask_token("xoxb-12345"), "xoxb-12345");
        assert_eq!(mask_token("xoxb-1234567"), "xoxb-12345••");
    }

    #[test]
    fn home_shows_as_tilde() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(tilde_path(&home.join("prog")), "~/prog");
        assert_eq!(tilde_path(&home), "~");
    }

    #[test]
    fn only_slack_members_are_listed() {
        assert_eq!(slack_members(&["slack:U1".into(), "telegram:9".into()]), vec!["U1"]);
    }
}

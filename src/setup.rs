//! Setup modal: asks for an API key per provider. Models are picked with `/model`.
//! Drawn over the chat by `tui.rs`; on save the keys go to `~/.minerve/keys.json` and to the running
//! `Minerve`. Any one key is enough to continue.

use std::io;
use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::keys_file;
use crate::provider::{
    ApiKeys, Provider, ANTHROPIC_API_KEY_ENV_VAR, OPENAI_API_KEY_ENV_VAR, OPENROUTER_API_KEY_ENV_VAR,
};

const OPENROUTER_KEYS_URL: &str = "https://openrouter.ai/keys";
const OPENAI_KEYS_URL: &str = "https://platform.openai.com/api-keys";
const ANTHROPIC_KEYS_URL: &str = "https://console.anthropic.com/settings/keys";
const MODAL_WIDTH: u16 = 76;
const MODAL_HEIGHT: u16 = 15;

/// Keys the person entered. A key is None when its field was left empty.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Settings {
    pub openrouter_key: Option<String>,
    pub openai_key: Option<String>,
    pub anthropic_key: Option<String>,
}

impl Settings {
    /// Each set key with the env var it is saved under.
    fn env_pairs(&self) -> Vec<(&'static str, String)> {
        let keys = [
            (OPENROUTER_API_KEY_ENV_VAR, &self.openrouter_key),
            (OPENAI_API_KEY_ENV_VAR, &self.openai_key),
            (ANTHROPIC_API_KEY_ENV_VAR, &self.anthropic_key),
        ];
        keys.into_iter()
            .filter_map(|(name, key)| key.clone().map(|key| (name, key)))
            .collect()
    }
}

/// What a key press did to the modal.
#[derive(Debug, PartialEq)]
pub enum SetupStep {
    /// Still open; the caller redraws.
    Stay,
    /// Esc: closed without saving.
    Cancelled,
    /// Enter on the last field: the caller saves and closes the modal.
    Completed(Settings),
}

/// Key fields top to bottom. Tab and arrows cycle through them; Enter on the last one saves.
const FIELDS: [Provider; 3] = [Provider::OpenRouter, Provider::OpenAi, Provider::Anthropic];

pub struct SetupModal {
    openrouter_key: String,
    openai_key: String,
    anthropic_key: String,
    /// Provider whose key field has the cursor.
    field: Provider,
    /// Which keys are already set. A set key with an empty field shows `***` and keeps its value on save.
    stored: ApiKeys,
    /// Shown under the fields, e.g. when saving the keys failed.
    error: Option<String>,
}

impl SetupModal {
    pub fn new(stored: ApiKeys) -> Self {
        Self {
            openrouter_key: String::new(),
            openai_key: String::new(),
            anthropic_key: String::new(),
            field: FIELDS[0],
            stored,
            error: None,
        }
    }

    pub fn set_error(&mut self, message: String) {
        self.error = Some(message);
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SetupStep {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => SetupStep::Cancelled,
            KeyCode::Tab | KeyCode::Down => {
                self.move_field(1);
                SetupStep::Stay
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.move_field(-1);
                SetupStep::Stay
            }
            KeyCode::Enter if self.field == FIELDS[FIELDS.len() - 1] => self.complete(),
            KeyCode::Enter => {
                self.move_field(1);
                SetupStep::Stay
            }
            KeyCode::Backspace => {
                self.active_text().pop();
                SetupStep::Stay
            }
            KeyCode::Char(c) if !ctrl => {
                self.active_text().push(c);
                SetupStep::Stay
            }
            _ => SetupStep::Stay,
        }
    }

    /// Pasted text goes into the active field, without line breaks or other control characters.
    pub fn paste(&mut self, text: &str) {
        let field = self.active_text();
        field.extend(text.chars().filter(|c| !c.is_control()));
    }

    /// Completes only when at least one key is entered; otherwise keeps the modal open with an error.
    fn complete(&mut self) -> SetupStep {
        let settings = self.settings();
        if settings.env_pairs().is_empty() {
            self.error = Some(String::from("Enter at least one API key."));
            return SetupStep::Stay;
        }
        self.error = None;
        SetupStep::Completed(settings)
    }

    fn settings(&self) -> Settings {
        let filled = |text: &str| {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        };
        Settings {
            openrouter_key: filled(&self.openrouter_key),
            openai_key: filled(&self.openai_key),
            anthropic_key: filled(&self.anthropic_key),
        }
    }

    fn move_field(&mut self, step: isize) {
        let index = FIELDS.iter().position(|field| *field == self.field).unwrap_or(0) as isize;
        let next = (index + step).rem_euclid(FIELDS.len() as isize) as usize;
        self.field = FIELDS[next];
    }

    fn active_text(&mut self) -> &mut String {
        match self.field {
            Provider::OpenRouter => &mut self.openrouter_key,
            Provider::OpenAi => &mut self.openai_key,
            Provider::Anthropic => &mut self.anthropic_key,
        }
    }

    /// Draws the modal centered over `area`.
    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        let modal = centered(area, MODAL_WIDTH, MODAL_HEIGHT);
        frame.render_widget(Clear, modal);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Set up minerve ")
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(modal);
        frame.render_widget(block, modal);

        let label = Style::default().add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(Color::DarkGray);
        let error = self.error.clone().unwrap_or_default();
        let lines = vec![
            Line::from("Enter API keys for your LLM providers. Each key is optional."),
            Line::from(""),
            Line::styled(format!("OpenRouter API key  {}", OPENROUTER_KEYS_URL), label),
            self.field_line(Provider::OpenRouter),
            Line::styled(format!("OpenAI API key      {}", OPENAI_KEYS_URL), label),
            self.field_line(Provider::OpenAi),
            Line::styled(format!("Anthropic API key   {}", ANTHROPIC_KEYS_URL), label),
            self.field_line(Provider::Anthropic),
            Line::from(""),
            Line::styled(error, Style::default().fg(Color::Red)),
            Line::styled("Tab/arrows nav | Enter on last field saves | Esc to quit", dim),
        ];
        frame.render_widget(Paragraph::new(lines), inner);

        // Cursor after the text of the active field. Rows match the order of `lines` above.
        let row = match self.field {
            Provider::OpenRouter => 3,
            Provider::OpenAi => 5,
            Provider::Anthropic => 7,
        };
        let text_len = self.key_display(self.field).chars().count();
        let cursor_x = inner.x + 2 + text_len as u16;
        frame.set_cursor_position((cursor_x.min(inner.right().saturating_sub(1)), inner.y + row));
    }

    /// One key input row, with a marker on the active one.
    fn field_line(&self, provider: Provider) -> Line<'static> {
        let marker = if self.field == provider { "> " } else { "  " };
        Line::from(vec![Span::raw(marker), Span::raw(self.key_display(provider))])
    }
}

/// Stars for a key, so it never shows on screen.
fn mask(key: &str) -> String {
    "*".repeat(key.chars().count())
}

impl SetupModal {
    /// Text of a key field. Typed text is starred; an empty field whose key is already stored shows `***`.
    fn key_display(&self, provider: Provider) -> String {
        let (typed, stored) = match provider {
            Provider::OpenRouter => (&self.openrouter_key, &self.stored.openrouter),
            Provider::OpenAi => (&self.openai_key, &self.stored.openai),
            Provider::Anthropic => (&self.anthropic_key, &self.stored.anthropic),
        };
        if typed.is_empty() && stored.is_some() {
            String::from("***")
        } else {
            mask(typed)
        }
    }
}

/// Writes the keys to `~/.minerve/keys.json` and sets them in this process, so the running session
/// uses them right away. `~/.env` is never written.
pub fn save(settings: &Settings) -> io::Result<PathBuf> {
    let saved = keys_to_store(settings, keys_file::load());
    let keys_path = keys_file::save(&saved)?;
    for (name, value) in settings.env_pairs() {
        std::env::set_var(name, value);
    }
    Ok(keys_path)
}

/// The keys to write: a typed key replaces the stored one, and a blank field keeps it.
fn keys_to_store(settings: &Settings, stored: keys_file::SavedKeys) -> keys_file::SavedKeys {
    keys_file::SavedKeys {
        openrouter_api_key: settings.openrouter_key.clone().or(stored.openrouter_api_key),
        openai_api_key: settings.openai_key.clone().or(stored.openai_api_key),
        anthropic_api_key: settings.anthropic_key.clone().or(stored.anthropic_api_key),
    }
}

pub(crate) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

/// True when no provider key is set, so the modal should open at start.
pub fn needs_setup(keys: &ApiKeys) -> bool {
    [&keys.openrouter, &keys.openai, &keys.anthropic]
        .iter()
        .all(|key| key.as_deref().map_or(true, |key| key.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(modal: &mut SetupModal, text: &str) {
        for c in text.chars() {
            modal.handle_key(press(KeyCode::Char(c)));
        }
    }

    #[test]
    fn needs_setup_only_when_no_provider_key_is_set() {
        assert!(needs_setup(&ApiKeys::default()));
        let blank = ApiKeys {
            openrouter: Some(String::from("   ")),
            ..ApiKeys::default()
        };
        assert!(needs_setup(&blank));
        let anthropic_only = ApiKeys {
            anthropic: Some(String::from("sk-ant-1")),
            ..ApiKeys::default()
        };
        assert!(!needs_setup(&anthropic_only));
    }

    #[test]
    fn enter_with_no_keys_keeps_the_modal_open_with_an_error() {
        let mut modal = SetupModal::new(ApiKeys::default());
        modal.handle_key(press(KeyCode::Tab));
        modal.handle_key(press(KeyCode::Tab));
        assert_eq!(modal.field, Provider::Anthropic);
        assert_eq!(modal.handle_key(press(KeyCode::Enter)), SetupStep::Stay);
        assert!(modal.error.is_some());
    }

    #[test]
    fn enter_on_the_last_key_field_completes_with_the_typed_keys() {
        let mut modal = SetupModal::new(ApiKeys::default());
        modal.handle_key(press(KeyCode::Enter));
        modal.handle_key(press(KeyCode::Enter));
        assert_eq!(modal.field, Provider::Anthropic);
        type_text(&mut modal, "sk-ant-1");
        assert_eq!(
            modal.handle_key(press(KeyCode::Enter)),
            SetupStep::Completed(Settings {
                anthropic_key: Some(String::from("sk-ant-1")),
                ..Settings::default()
            })
        );
    }

    #[test]
    fn blank_fields_keep_stored_keys_and_typed_ones_replace_them() {
        let stored = keys_file::SavedKeys {
            openrouter_api_key: Some(String::from("sk-or-old")),
            openai_api_key: Some(String::from("sk-openai-old")),
            anthropic_api_key: None,
        };
        let settings = Settings {
            openai_key: Some(String::from("sk-openai-new")),
            ..Settings::default()
        };
        assert_eq!(
            keys_to_store(&settings, stored),
            keys_file::SavedKeys {
                openrouter_api_key: Some(String::from("sk-or-old")),
                openai_api_key: Some(String::from("sk-openai-new")),
                anthropic_api_key: None,
            }
        );
    }

    #[test]
    fn env_pairs_skip_empty_fields() {
        let settings = Settings {
            openai_key: Some(String::from("sk-1")),
            ..Settings::default()
        };
        assert_eq!(settings.env_pairs(), vec![(OPENAI_API_KEY_ENV_VAR, String::from("sk-1"))]);
    }

    #[test]
    fn esc_cancels() {
        let mut modal = SetupModal::new(ApiKeys::default());
        assert_eq!(modal.handle_key(press(KeyCode::Esc)), SetupStep::Cancelled);
    }

    #[test]
    fn paste_drops_newlines() {
        let mut modal = SetupModal::new(ApiKeys::default());
        modal.paste("sk-or-2\n");
        assert_eq!(modal.openrouter_key, "sk-or-2");
    }
}

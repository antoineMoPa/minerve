//! Terminal UI built with ratatui. Runs on the main thread and owns the terminal.
//!
//! Background work reports through `UiEvent`s (see `ui.rs`); key presses are handled here.

use std::io;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::time::Duration;

use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::history::HistoryTracker;
use crate::minerve::Minerve;
use crate::openrouter::OpenRouterConnection;
use crate::ui::{UiEvent, UiSender};

/// Tool results longer than this are cut short in the chat view (the model still gets them all).
const MAX_OUTPUT_LEN: usize = 500;
/// Only the last messages are shown, as before the ratatui refactor.
const VISIBLE_MESSAGES: usize = 10;
const SCROLL_STEP: usize = 10;

struct PendingConfirm {
    command: String,
    reply: SyncSender<bool>,
}

struct App {
    minerve: Arc<Minerve>,
    ui_tx: UiSender,
    ui_rx: Receiver<UiEvent>,
    history: HistoryTracker,
    /// Copy of the chat as last reported by `Minerve`.
    transcript: Vec<(String, String)>,
    status: String,
    pending_confirm: Option<PendingConfirm>,
    input: String,
    /// Cursor position in chars, not bytes.
    cursor: usize,
    /// Lines scrolled up from the bottom of the chat. 0 follows the latest message.
    scroll_up: usize,
    /// Tool outputs are collapsed to one line unless this is set (Ctrl+O).
    tools_expanded: bool,
    should_quit: bool,
}

/// Runs the interactive UI until the user quits.
pub fn run(openrouter_connection: OpenRouterConnection) -> io::Result<()> {
    let (ui_tx, ui_rx) = mpsc::channel();
    let mut app = App {
        minerve: Arc::new(Minerve::new(openrouter_connection)),
        ui_tx,
        ui_rx,
        history: HistoryTracker::new(),
        transcript: Vec::new(),
        status: String::new(),
        pending_confirm: None,
        input: String::new(),
        cursor: 0,
        scroll_up: 0,
        tools_expanded: false,
        should_quit: false,
    };

    let mut terminal = ratatui::init();
    // Pasted text arrives as one `Event::Paste` instead of many key presses,
    // so newlines in a paste do not submit the prompt.
    let _ = execute!(io::stdout(), EnableBracketedPaste);
    let result = app.run_loop(&mut terminal);
    // Restore even when the loop failed, so the shell is usable again.
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

impl App {
    fn run_loop(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.should_quit {
            while let Ok(event) = self.ui_rx.try_recv() {
                self.handle_ui_event(event);
            }

            terminal.draw(|frame| self.draw(frame))?;

            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    // Windows also reports key releases; only presses count.
                    Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key(key),
                    Event::Paste(text) => self.handle_paste(&text),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Inserts pasted text at the cursor without submitting, keeping its newlines.
    fn handle_paste(&mut self, text: &str) {
        if self.pending_confirm.is_some() {
            return;
        }
        for c in text.replace("\r\n", "\n").replace('\r', "\n").chars() {
            self.insert_char(c);
        }
    }

    fn handle_ui_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Transcript(messages) => self.transcript = messages,
            UiEvent::Status(status) => self.status = status,
            UiEvent::ConfirmShell { command, reply } => {
                self.pending_confirm = Some(PendingConfirm { command, reply });
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        if ctrl && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        // While a shell command waits for approval, only the answer keys count.
        if let Some(pending) = self.pending_confirm.take() {
            let answer = match key.code {
                KeyCode::Char('y' | 'Y') => Some(true),
                KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(false),
                _ => None,
            };
            match answer {
                Some(accepted) => {
                    let _ = pending.reply.send(accepted);
                }
                None => self.pending_confirm = Some(pending),
            }
            return;
        }

        match key.code {
            KeyCode::Enter if alt || shift => self.insert_char('\n'),
            KeyCode::Enter => self.submit(),
            KeyCode::Char('j') if ctrl => self.insert_char('\n'),
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.input_len(),
            KeyCode::Char('k') if ctrl => {
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Char('q') if ctrl => self.should_quit = true,
            KeyCode::Char('o') if ctrl => self.tools_expanded = !self.tools_expanded,
            KeyCode::Char(c) if !ctrl && !alt => self.insert_char(c),
            KeyCode::Backspace => self.delete_before_cursor(),
            KeyCode::Delete => self.delete_at_cursor(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input_len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input_len(),
            KeyCode::Up => {
                if let Some(prompt) = self.history.get_previous_prompt() {
                    self.set_input(prompt);
                }
            }
            KeyCode::Down => {
                let prompt = self.history.get_next_prompt().unwrap_or_default();
                self.set_input(prompt);
            }
            KeyCode::PageUp => self.scroll_up = self.scroll_up.saturating_add(SCROLL_STEP),
            KeyCode::PageDown => self.scroll_up = self.scroll_up.saturating_sub(SCROLL_STEP),
            _ => {}
        }
    }

    fn submit(&mut self) {
        if self.input.trim().is_empty() {
            return;
        }
        let content = self.input.clone();

        self.history.add_prompt(content.clone());
        self.minerve
            .clone()
            .chat_with_arc(content, self.ui_tx.clone(), false);

        self.input.clear();
        self.cursor = 0;
        self.scroll_up = 0;
    }

    fn set_input(&mut self, text: String) {
        self.cursor = text.chars().count();
        self.input = text;
    }

    fn input_len(&self) -> usize {
        self.input.chars().count()
    }

    /// Byte index of the char at `char_idx`, or the end of the input.
    fn byte_index(&self, char_idx: usize) -> usize {
        self.input
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.input.len())
    }

    fn insert_char(&mut self, c: char) {
        let idx = self.byte_index(self.cursor);
        self.input.insert(idx, c);
        self.cursor += 1;
    }

    fn delete_before_cursor(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let idx = self.byte_index(self.cursor - 1);
        self.input.remove(idx);
        self.cursor -= 1;
    }

    fn delete_at_cursor(&mut self) {
        if self.cursor >= self.input_len() {
            return;
        }
        let idx = self.byte_index(self.cursor);
        self.input.remove(idx);
    }

    fn draw(&mut self, frame: &mut Frame) {
        // Input grows with its wrapped text: 1 line minimum, at most 8 lines of text.
        let inner_width = frame.area().width.saturating_sub(2);
        let (_, last_row) = self.cursor_offset(self.input_len(), inner_width);
        let input_height = (last_row as u16 + 3).clamp(3, 10);

        let [chat_area, token_area, status_area, input_area, hint_area] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        self.draw_chat(frame, chat_area);

        frame.render_widget(
            Paragraph::new(self.minerve.token_counter.format_usage_summary())
                .style(Style::default().fg(Color::Gray)),
            token_area,
        );
        frame.render_widget(
            Paragraph::new(self.status.as_str())
                .style(Style::default().fg(Color::Magenta)),
            status_area,
        );

        self.draw_input(frame, input_area);

        frame.render_widget(
            Paragraph::new(
                "Enter send | Alt+Enter newline | Up/Down history | PgUp/PgDn scroll | Ctrl+O tool output | Ctrl+C quit",
            )
            .style(Style::default().fg(Color::DarkGray)),
            hint_area,
        );
    }

    fn draw_chat(&mut self, frame: &mut Frame, area: Rect) {
        let lines = self.chat_lines();
        let width = area.width.max(1) as usize;
        let total: usize = lines
            .iter()
            .map(|line| line.width().div_ceil(width).max(1))
            .sum();

        let max_scroll = total.saturating_sub(area.height as usize);
        self.scroll_up = self.scroll_up.min(max_scroll);
        let top = (max_scroll - self.scroll_up).min(u16::MAX as usize) as u16;

        let paragraph = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((top, 0));
        frame.render_widget(paragraph, area);
    }

    fn chat_lines(&self) -> Vec<Line<'static>> {
        let shown: Vec<&(String, String)> = self
            .transcript
            .iter()
            .filter(|(role, _)| role != "system")
            .collect();
        let start = shown.len().saturating_sub(VISIBLE_MESSAGES);

        let mut lines = Vec::new();
        for (role, content) in &shown[start..] {
            let (label, style) = match role.as_str() {
                "user" => (
                    "You",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                "minerve" => (
                    "Minerve",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                other => (other, Style::default().add_modifier(Modifier::BOLD)),
            };
            lines.push(Line::styled(format!("{}:", label), style));

            // Tool results are labeled with the function name; collapse them by default.
            let is_tool = !matches!(role.as_str(), "user" | "minerve" | "system");
            if is_tool && !self.tools_expanded {
                let first = content.lines().next().unwrap_or_default();
                let lines_count = content.lines().count();
                let summary = format!("  [{} line(s)] {}", lines_count, first);
                lines.push(Line::styled(
                    format!("{} ▸ collapsed (Ctrl+O to expand)", summary.trim_end()),
                    Style::default().fg(Color::DarkGray),
                ));
                lines.push(Line::default());
                continue;
            }

            let text = if content.chars().count() > MAX_OUTPUT_LEN {
                if role.as_str() == "minerve" {
                    content.trim().to_string()
                } else {
                    let head: String = content.chars().take(MAX_OUTPUT_LEN).collect();
                    format!("{}\n...[truncated]", head)
                }
            } else {
                content.clone()
            };
            lines.extend(text.split('\n').map(|part| Line::raw(part.to_string())));
            lines.push(Line::default());
        }
        lines
    }

    fn draw_input(&self, frame: &mut Frame, area: Rect) {
        let (title, text, border_color) = match &self.pending_confirm {
            Some(pending) => (
                " Run this shell command? [y/n] ",
                pending.command.clone(),
                Color::Yellow,
            ),
            None => (
                " Message ",
                self.input.clone(),
                Color::Cyan,
            ),
        };

        // Inside the borders. Approximate: wraps by char count, like a monospace terminal.
        let inner_width = area.width.saturating_sub(2);
        let inner_height = area.height.saturating_sub(2);
        let (col, row) = self.cursor_offset(self.cursor, inner_width);
        // Keep the cursor row visible once the wrapped input is taller than the box.
        let scroll_row = if self.pending_confirm.is_none() {
            row.saturating_add(1).saturating_sub(inner_height)
        } else {
            0
        };

        let input = Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((scroll_row, 0))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Style::default().fg(border_color)),
            );
        frame.render_widget(input, area);

        if self.pending_confirm.is_none() {
            frame.set_cursor_position((area.x + 1 + col, area.y + 1 + row - scroll_row));
        }
    }

    /// (column, row) of the char at `cursor` within the input text wrapped at `width`.
    fn cursor_offset(&self, cursor: usize, width: u16) -> (u16, u16) {
        let width = width.max(1) as usize;
        let (mut row, mut col) = (0usize, 0usize);
        for c in self.input.chars().take(cursor) {
            if c == '\n' {
                row += 1;
                col = 0;
            } else {
                col += 1;
                if col == width {
                    row += 1;
                    col = 0;
                }
            }
        }
        (col.min(u16::MAX as usize) as u16, row.min(u16::MAX as usize) as u16)
    }
}

//! Terminal UI built with ratatui. Runs on the main thread and owns the terminal.
//!
//! Background work reports through `UiEvent`s (see `ui.rs`); key presses are handled here.

use std::io;
use std::sync::mpsc::{self, Receiver};
use std::sync::atomic::Ordering;
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

use crate::boot::BootAnimation;
use crate::history::HistoryTracker;
use crate::minerve::Minerve;
use crate::openrouter::OpenRouterConnection;
use crate::session::SessionFile;
use crate::ui::{UiEvent, UiSender};

/// Tool results longer than this are cut short in the chat view (the model still gets them all).
const MAX_OUTPUT_LEN: usize = 500;
/// Only the last messages are shown, as before the ratatui refactor.
const VISIBLE_MESSAGES: usize = 10;
const SCROLL_STEP: usize = 10;

struct App {
    minerve: Arc<Minerve>,
    ui_tx: UiSender,
    ui_rx: Receiver<UiEvent>,
    history: HistoryTracker,
    /// Copy of the chat as last reported by `Minerve`.
    transcript: Vec<(String, String)>,
    status: String,
    input: String,
    /// Cursor position in chars, not bytes.
    cursor: usize,
    /// Lines scrolled up from the bottom of the chat. 0 follows the latest message.
    scroll_up: usize,
    /// Tool outputs are collapsed to one line unless this is set (Ctrl+O).
    tools_expanded: bool,
    should_quit: bool,
    /// Boot clip shown until the first key press or paste (or until it ends).
    boot: Option<BootAnimation>,
}

/// Runs the interactive UI until the user quits.
pub fn run(
    openrouter_connection: OpenRouterConnection,
    resume: Option<SessionFile>,
    extra_system_prompts: &[String],
) -> io::Result<()> {
    let (ui_tx, ui_rx) = mpsc::channel();
    // The boot clip is for fresh sessions only; resuming one goes straight to the chat.
    let boot = if resume.is_none() {
        BootAnimation::start()
    } else {
        None
    };
    let minerve = Arc::new(Minerve::new(
        openrouter_connection,
        resume,
        extra_system_prompts,
    ));
    let transcript = minerve.ui_messages();
    let mut app = App {
        minerve,
        ui_tx,
        ui_rx,
        history: HistoryTracker::new(),
        transcript,
        status: String::new(),
        input: String::new(),
        cursor: 0,
        scroll_up: 0,
        tools_expanded: false,
        should_quit: false,
        boot,
    };

    let mut terminal = ratatui::init();
    // Pasted text arrives as one `Event::Paste` instead of many key presses,
    // so newlines in a paste do not submit the prompt.
    let _ = execute!(io::stdout(), EnableBracketedPaste);
    let result = app.run_loop(&mut terminal);
    // Restore even when the loop failed, so the shell is usable again.
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    // Printed after the restore so it lands on the normal screen, not the alternate one.
    println!("{}", app.minerve.session_id);
    result
}

impl App {
    fn run_loop(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.should_quit {
            while let Ok(event) = self.ui_rx.try_recv() {
                self.handle_ui_event(event);
            }

            terminal.draw(|frame| self.draw(frame))?;

            // While the boot clip plays, wake up at the next frame boundary.
            let timeout = match &self.boot {
                Some(boot) => boot.time_to_next_frame().min(Duration::from_millis(50)),
                None => Duration::from_millis(50),
            };
            if event::poll(timeout)? {
                match event::read()? {
                    // Windows also reports key releases; only presses count.
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        self.stop_boot();
                        self.handle_key(key);
                    }
                    Event::Paste(text) => {
                        self.stop_boot();
                        self.handle_paste(&text);
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Inserts pasted text at the cursor without submitting, keeping its newlines.
    fn handle_paste(&mut self, text: &str) {
        for c in text.replace("\r\n", "\n").replace('\r', "\n").chars() {
            self.insert_char(c);
        }
    }

    fn handle_ui_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Transcript(messages) => self.transcript = messages,
            UiEvent::Status(status) => self.status = status,
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

        // Esc stops the running turn; with no turn running it does nothing.
        if key.code == KeyCode::Esc {
            if self.minerve.request_in_flight.load(Ordering::SeqCst) {
                self.minerve.cancel_turn();
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

    /// Stops the video now (key press). The dotted ring then erodes away; the boot state is
    /// dropped and the screen cleared once the erosion finishes (see `draw_boot_overlay`).
    fn stop_boot(&mut self) {
        self.boot = None;
    }

    /// Draws the boot overlay in `area`: a dotted ring around the centered video, and the
    /// current video frame. Only non-blank video characters are written, so the rest of the UI
    /// shows through the transparent background. Once the ring has eroded away, drops the boot
    /// state and schedules a screen clear.
    fn draw_boot_overlay(&mut self, frame: &mut Frame, area: Rect) {
        let Some(boot) = self.boot.as_ref() else {
            return;
        };
        let erosion = boot.erosion();
        let sweep = boot.sweep_progress();
        let finished = boot.is_finished();
        let (width, height) = boot.size();
        let text = boot.current_frame();
        if finished {
            self.boot = None;
            return;
        }

        // Fit the video into the area, keeping its aspect ratio. Reserve 2 columns on each side
        // for the ring, and 4 rows for the ring (1 above and below) plus the caption block
        // (1 blank row, then the caption). Work in i32 so the ring can extend past the area.
        let (ax, ay) = (area.x as i32, area.y as i32);
        let (aw, ah) = (area.width as i32, area.height as i32);
        let (sw, sh) = (width as i32, height as i32);
        // 0.8 shrinks the fitted video by 20%, leaving more of the dot pattern visible around it.
        let scale = 0.8 * ((aw - 4).max(1) as f32 / sw as f32).min((ah - 4).max(1) as f32 / sh as f32);
        let w = ((sw as f32 * scale) as i32).max(1);
        let h = ((sh as f32 * scale) as i32).max(1);
        let vx = ax + (aw - w) / 2;
        let vy = ay + ((ah - h - 4).max(0)) / 2 + 1;
        let in_area = |x: i32, y: i32| x >= ax && x < ax + aw && y >= ay && y < ay + ah;
        let buf = frame.buffer_mut();

        // Dot pattern filling the whole chat area, except the video's own cells (drawn over it
        // below). Dots sit on every other cell. Erosion hides cells whose hashed threshold is
        // below the progress, so the pattern disappears unevenly.
        // Distance from the video border, in cells. Vertical distance is doubled because a
        // terminal cell is about twice as tall as it is wide. Cells closer than EDGE_BLEND to the
        // border are the blend band.
        const EDGE_BLEND: i32 = 7;
        let edge_distance = |c: i32, r: i32| (c.min(w - 1 - c)).min(2 * r.min(h - 1 - r));
        // Stable pseudo-random value in [0, 1) per cell.
        let cell_hash = |x: i32, y: i32| {
            let h = ((x as u32).wrapping_mul(73_856_093) ^ (y as u32).wrapping_mul(19_349_663)) % 1000;
            h as f32 / 1000.0
        };

        // Sparse dot grid: one dot every 3 columns and every 2 rows, across the whole area, including
        // behind the video. The video's blank cells show these dots, so the pattern runs seamlessly
        // into the video. Erosion hides dots unevenly.
        let dot_grid = |x: i32, y: i32| x % 3 == 0 && y % 2 == 0;
        for y in ay..ay + ah {
            for x in ax..ax + aw {
                if !dot_grid(x, y) {
                    continue;
                }
                let visible = erosion.map_or(true, |e| cell_hash(x, y) >= e);
                if visible {
                    buf[(x as u16, y as u16)].set_char('·').set_fg(ratatui::style::Color::DarkGray);
                }
            }
        }

        // Video frame, centered inside the ring. Blank characters are transparent.
        if let Some(text) = text {
            // Nearest-neighbour resample of the source characters to the fitted size.
            let src: Vec<Vec<char>> = text.lines().map(|l| l.chars().collect()).collect();
            for r in 0..h {
                let sr = (r * sh / h) as usize;
                for c in 0..w {
                    let sc = (c * sw / w) as usize;
                    let ch = src.get(sr).and_then(|row| row.get(sc)).copied().unwrap_or(' ');
                    let (x, y) = (vx + c, vy + r);
                    // Video cells in the blend band show with a chance rising from the border
                    // inward, so the edge dissolves into the dots.
                    let d = edge_distance(c, r);
                    let show = d >= EDGE_BLEND || cell_hash(x, y) < (d as f32 + 0.5) / EDGE_BLEND as f32;
                    if show && ch != ' ' && in_area(x, y) {
                        buf[(x as u16, y as u16)].set_char(ch);
                    }
                }
            }

        }

        // Caption centered under the video, one blank line below the ring. It stays for the
        // whole boot screen (also after the video stops) until the dash sweep erases it.
        const CAPTION: &str = "loading ancient knowledge";
        let caption_y = vy + h + 2;
        let caption_len = CAPTION.chars().count() as i32;
        let cap_l = vx + (w - caption_len) / 2;
        let cap_r = cap_l + caption_len - 1;
        for (col, ch) in CAPTION.chars().enumerate() {
            let x = cap_l + col as i32;
            if in_area(x, caption_y) {
                buf[(x as u16, caption_y as u16)].set_char(ch).set_fg(ratatui::style::Color::Gray);
            }
        }

        // Dashes frame the caption from the start. After the dots erode, they move in along the
        // caption row toward its center. Every caption cell they pass is blanked; they vanish at
        // the center.
        let dash_l = cap_l - 2;
        let dash_r = cap_r + 2;
        let mid = (cap_l + cap_r) / 2;
        let p = sweep.unwrap_or(0.0);
        let lf = dash_l + ((mid - dash_l) as f32 * p) as i32;
        let rf = dash_r - ((dash_r - mid) as f32 * p) as i32;
        if sweep.is_some() {
            for x in (dash_l..lf).chain(rf + 1..=dash_r) {
                if in_area(x, caption_y) {
                    buf[(x as u16, caption_y as u16)].set_char(' ');
                }
            }
        }
        for x in [lf, rf] {
            if in_area(x, caption_y) {
                buf[(x as u16, caption_y as u16)].set_char('-').set_fg(ratatui::style::Color::White);
            }
        }
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
                "Enter send | Esc cancel turn | Alt+Enter newline | Up/Down history | PgUp/PgDn scroll | Ctrl+O tool output | Ctrl+C quit",
            )
            .style(Style::default().fg(Color::DarkGray)),
            hint_area,
        );

        if self.boot.is_some() {
            self.draw_boot_overlay(frame, chat_area);
        }
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
        let title = " Message ";
        let border_color = Color::Cyan;

        // Inside the borders. Approximate: wraps by char count, like a monospace terminal.
        let inner_width = area.width.saturating_sub(2);
        let inner_height = area.height.saturating_sub(2);
        let (col, row) = self.cursor_offset(self.cursor, inner_width);
        // Keep the cursor row visible once the wrapped input is taller than the box.
        let scroll_row = row.saturating_add(1).saturating_sub(inner_height);

        // Wrap by char count ourselves (no word wrap) so the rendered rows match `cursor_offset`.
        let text = self.wrap_input(inner_width);
        let input = Paragraph::new(text)
            .scroll((scroll_row, 0))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Style::default().fg(border_color)),
            );
        frame.render_widget(input, area);

        frame.set_cursor_position((area.x + 1 + col, area.y + 1 + row - scroll_row));
    }

    /// The input with a newline inserted wherever it wraps at `width`, matching `cursor_offset`.
    fn wrap_input(&self, width: u16) -> String {
        let width = width.max(1) as usize;
        let mut wrapped = String::with_capacity(self.input.len() + 8);
        let mut col = 0usize;
        for c in self.input.chars() {
            if c == '\n' {
                wrapped.push('\n');
                col = 0;
            } else {
                wrapped.push(c);
                col += 1;
                if col == width {
                    wrapped.push('\n');
                    col = 0;
                }
            }
        }
        wrapped
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

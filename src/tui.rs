//! Terminal UI built with ratatui. Runs on the main thread and owns the terminal.
//!
//! Background work reports through `UiEvent`s (see `ui.rs`); key presses are handled here.

use std::io::{self, Write};
use std::sync::mpsc::{self, Receiver};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::input_layout::layout_input;
use crate::model_picker::ModelPicker;
use crate::recent_models::RecentModels;

use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
    KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::boot::BootAnimation;
use crate::history::HistoryTracker;
use crate::minerve::Minerve;
use crate::openrouter::OpenRouterConnection;
use crate::pacman::Pacman;
use crate::session::SessionFile;
use crate::tetris::Tetris;
use crate::ui::{UiEvent, UiSender};

/// Tool results longer than this are cut short in the chat view (the model still gets them all).
const MAX_OUTPUT_LEN: usize = 500;
/// Only the last messages are shown, as before the ratatui refactor.
const VISIBLE_MESSAGES: usize = 10;
const SCROLL_STEP: usize = 10;
/// Chat lines scrolled per mouse wheel notch.
const WHEEL_STEP: usize = 3;
/// Slash commands offered by the `/` menu: (name, one-line description).
const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/about", "describe Minerve"),
    ("/model", "switch the OpenRouter model"),
    ("/matrix", "falling letters; any key to leave"),
    ("/tetris", "play a mini Tetris; esc to leave"),
    ("/pacman", "eat dots, dodge ghosts; esc to leave"),
];

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
    /// Text removed with Ctrl+K, oldest first. Yanked back with Ctrl+Y / Alt+Y.
    kill_ring: Vec<String>,
    /// Set right after a yank so that Alt+Y can replace the yanked text with an older kill.
    last_yank: Option<YankState>,
    /// Output of `!command` runs, sent to the model with the next message and then cleared.
    pending_command_output: Vec<String>,
    /// True while the input holds pasted text. Pasted text never opens the slash menu or runs a command.
    input_pasted: bool,
    /// Results of local slash commands (e.g. `/about`), shown in the chat until the next message.
    notices: Vec<String>,
    /// Falling-letter screen from `/matrix`. While set it replaces the whole UI; any key or paste ends it.
    matrix: Option<MatrixRain>,
    /// Tetris game from `/tetris`. While set it replaces the whole UI; Esc or q leaves it.
    tetris: Option<Tetris>,
    /// Pac-Man game from `/pacman`. While set it replaces the whole UI; Esc or q leaves it.
    pacman: Option<Pacman>,
    /// Model picker from `/model`. While set it replaces the whole UI; Esc closes it.
    model_picker: Option<ModelPicker>,
    /// Least recently used models, shown at the top of the picker.
    recent_models: RecentModels,
    /// Input box as last drawn, and how many wrapped rows it was scrolled. Maps mouse clicks to the cursor.
    input_area: Rect,
    input_scroll_row: u16,
    /// Chat as last drawn. Hovering or clicking it gives it focus.
    chat_area: Rect,
    /// True when the chat has focus: Up/Down scroll it and typing gives focus back to the input.
    chat_focused: bool,
}

/// Where the last yank landed, so Alt+Y can swap it for the previous kill.
#[derive(Clone, Copy)]
struct YankState {
    /// Char index where the yanked text starts.
    start: usize,
    /// Length of the yanked text in chars.
    len: usize,
    /// How many entries back from the newest kill the yanked text comes from (0 = newest).
    ring_offset: usize,
}

/// Maximum number of killed texts kept for yanking.
const KILL_RING_MAX: usize = 60;

/// Characters the falling streams are made of (same set as the Emacs `matrix-mode` it follows).
const MATRIX_GLYPHS: &str = "123456789abcdefghijklmnopqrstuvwxyzαβγδεζηθικλμνξοπρστυφχψωΑΒΓΔΕΖΗΘΙΚΛΜΝΞΟΠΡΣΤΥΦΧΨΩ";

/// True when the terminal cell at (column, row) is inside `area`.
fn contains_cell(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x && column < area.right() && row >= area.y && row < area.bottom()
}

/// Xorshift64 step, returning a float in [0, 1). Good enough for falling letters; avoids a crate.
fn next_unit(state: &mut u64) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 40) as f32 / (1u64 << 24) as f32
}

/// One falling column of letters.
#[derive(Clone, Copy)]
struct MatrixStream {
    /// Row of the leading (brightest) letter. Negative while the stream is still above the screen.
    head: f32,
    /// Rows per second.
    speed: f32,
    /// Trail length in rows, head included.
    length: u16,
}

impl MatrixStream {
    /// A new stream. `scattered` streams start somewhere on screen; the others start above it,
    /// so the screen fills in gradually.
    fn random(rng: &mut u64, height: u16, scattered: bool) -> Self {
        let height = height.max(1) as f32;
        let head = if scattered {
            next_unit(rng) * height
        } else {
            -next_unit(rng) * height
        };
        Self {
            head,
            speed: 8.0 + next_unit(rng) * 20.0,
            length: 4 + (next_unit(rng) * height * 0.6) as u16,
        }
    }
}

/// Length of the fade-in when `/matrix` starts and the fade-out when it is stopped.
const MATRIX_FADE: Duration = Duration::from_millis(500);

/// Where the `/matrix` screen is in its life cycle.
#[derive(Clone, Copy, PartialEq)]
enum MatrixPhase {
    /// Letters start falling from the top edge while the screen brightens.
    FadingIn,
    /// Full rain.
    Running,
    /// No new streams spawn; the existing ones fall off the bottom while the screen dims.
    FadingOut,
}

/// State of the `/matrix` screen: one stream per column, advanced in real time.
struct MatrixRain {
    width: u16,
    height: u16,
    streams: Vec<MatrixStream>,
    glyphs: Vec<char>,
    rng: u64,
    last_tick: Instant,
    phase: MatrixPhase,
    phase_started: Instant,
    /// Brightness when `stop` was called, so a stop during the fade-in continues from there.
    stop_level: f32,
}

impl MatrixRain {
    fn new(width: u16, height: u16) -> Self {
        let rng = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1)
            | 1;
        let mut rain = Self {
            width: 0,
            height: 0,
            streams: Vec::new(),
            glyphs: MATRIX_GLYPHS.chars().collect(),
            rng,
            last_tick: Instant::now(),
            phase: MatrixPhase::FadingIn,
            phase_started: Instant::now(),
            stop_level: 1.0,
        };
        rain.resize(width, height);
        rain
    }

    /// Rebuilds the streams for a new size. Every stream starts above the top edge, so the
    /// letters fall in from the top.
    fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.streams = (0..width)
            .map(|_| MatrixStream::random(&mut self.rng, height, false))
            .collect();
    }

    /// Starts the fade-out. Ignored once it has begun.
    fn stop(&mut self) {
        if self.phase != MatrixPhase::FadingOut {
            self.stop_level = self.level();
            self.phase = MatrixPhase::FadingOut;
            self.phase_started = Instant::now();
        }
    }

    /// Brightness in [0, 1] that scales every colour.
    fn level(&self) -> f32 {
        let t = (self.phase_started.elapsed().as_secs_f32() / MATRIX_FADE.as_secs_f32()).min(1.0);
        match self.phase {
            MatrixPhase::FadingIn => t,
            MatrixPhase::Running => 1.0,
            MatrixPhase::FadingOut => self.stop_level * (1.0 - t),
        }
    }

    /// Moves the phase on when its fade has finished.
    fn update_phase(&mut self) {
        if self.phase == MatrixPhase::FadingIn && self.phase_started.elapsed() >= MATRIX_FADE {
            self.phase = MatrixPhase::Running;
        }
    }

    /// Moves every stream down by the time since the last draw. A stream that has left the
    /// bottom starts again above the top, unless the screen is fading out.
    fn advance(&mut self) {
        let now = Instant::now();
        // Capped so a long pause (e.g. a suspended terminal) does not jump the streams.
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.2);
        self.last_tick = now;
        let height = self.height as f32;
        let stopping = self.phase == MatrixPhase::FadingOut;
        for stream in &mut self.streams {
            stream.head += stream.speed * dt;
            if stream.head - stream.length as f32 > height {
                if stopping {
                    // Length 0 draws nothing, so the stream is gone for good.
                    stream.length = 0;
                } else {
                    *stream = MatrixStream::random(&mut self.rng, self.height, false);
                }
            }
        }
    }

    /// Paints the rain over `area`. Returns false once the fade-out is over and the normal
    /// UI should come back.
    fn draw(&mut self, frame: &mut Frame, area: Rect) -> bool {
        self.update_phase();
        if self.phase == MatrixPhase::FadingOut && self.level() <= 0.0 {
            return false;
        }
        if self.width != area.width || self.height != area.height {
            self.resize(area.width, area.height);
        }
        self.advance();

        let level = self.level();
        let scale = |value: f32| (value * level) as u8;
        let buf = frame.buffer_mut();
        for (col, stream) in self.streams.iter().enumerate() {
            let x = area.x + col as u16;
            for row in 0..area.height {
                // Rows above the leading letter; the trail is the first `length` of them.
                let distance = stream.head.floor() - row as f32;
                let lit = distance >= 0.0 && distance < stream.length as f32;
                let cell = &mut buf[(x, area.y + row)];
                cell.set_bg(Color::Black);
                if !lit {
                    cell.set_char(' ');
                    continue;
                }
                // Letters flicker: each drawn cell picks a new glyph.
                let index = (next_unit(&mut self.rng) * self.glyphs.len() as f32) as usize;
                let color = if distance < 1.0 {
                    let white = scale(255.0);
                    Color::Rgb(white, white, white)
                } else {
                    let fade = 1.0 - distance / stream.length as f32;
                    Color::Rgb(0, scale(60.0 + 195.0 * fade), 0)
                };
                cell.set_char(self.glyphs[index % self.glyphs.len()]).set_fg(color);
            }
        }
        true
    }
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
    minerve.fetch_context_window_in_background();
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
        kill_ring: Vec::new(),
        last_yank: None,
        pending_command_output: Vec::new(),
        input_pasted: false,
        notices: Vec::new(),
        matrix: None,
        tetris: None,
        pacman: None,
        model_picker: None,
        recent_models: RecentModels::load(),
        input_area: Rect::default(),
        input_scroll_row: 0,
        chat_area: Rect::default(),
        chat_focused: false,
    };

    let mut terminal = ratatui::init();
    // Pasted text arrives as one `Event::Paste` instead of many key presses,
    // so newlines in a paste do not submit the prompt.
    let _ = execute!(io::stdout(), EnableBracketedPaste);
    // Clicks move the cursor in the input box and the wheel scrolls the chat.
    // Terminals then select text only with a modifier held (Shift in most, Option in iTerm2).
    let _ = execute!(io::stdout(), EnableMouseCapture);
    // Mode 1003 reports moves without a button held, so hovering the chat gives it focus.
    // Crossterm's EnableMouseCapture does not turn it on.
    let _ = io::stdout().write_all(b"\x1b[?1003h");
    // Lets Shift+Enter arrive as its own key where the terminal supports it.
    let keyboard_enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if keyboard_enhanced {
        let _ = execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }
    let result = app.run_loop(&mut terminal);
    // Restore even when the loop failed, so the shell is usable again.
    if keyboard_enhanced {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = io::stdout().write_all(b"\x1b[?1003l");
    let _ = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
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

            if let Some(game) = self.tetris.as_mut() {
                game.update(Instant::now());
            }
            if let Some(game) = self.pacman.as_mut() {
                game.update(Instant::now());
            }

            terminal.draw(|frame| self.draw(frame))?;

            // While the boot clip plays, wake up at the next frame boundary.
            let timeout = match &self.boot {
                Some(boot) => boot.time_to_next_frame().min(Duration::from_millis(50)),
                // The rain moves every frame, so it redraws about 30 times a second.
                None if self.matrix.is_some() => Duration::from_millis(33),
                None => Duration::from_millis(50),
            };
            if event::poll(timeout)? {
                match event::read()? {
                    // Windows also reports key releases; only presses count.
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        // Any key starts the fade-out and is not typed into the prompt. Ctrl+C still quits.
                        let quit_key = key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
                        if !quit_key && self.matrix.is_some() {
                            self.stop_matrix();
                            continue;
                        }
                        if !quit_key {
                            if let Some(game) = self.tetris.as_mut() {
                                if !game.handle_key(key.code) {
                                    self.tetris = None;
                                }
                                continue;
                            }
                            if let Some(game) = self.pacman.as_mut() {
                                if !game.handle_key(key.code) {
                                    self.pacman = None;
                                }
                                continue;
                            }
                        }
                        // The model picker takes every key, Ctrl+C still quits.
                        if !quit_key && self.model_picker.is_some() {
                            self.handle_model_picker_key(key);
                            continue;
                        }
                        self.stop_boot();
                        self.handle_key(key);
                        // Clearing the input also clears the paste marker.
                        if self.input.is_empty() {
                            self.input_pasted = false;
                        }
                    }
                    Event::Paste(text) => {
                        if self.tetris.is_some() || self.pacman.is_some() {
                            continue;
                        }
                        // Pasted text goes into the picker filter; newlines would not match any model id.
                        if let Some(picker) = self.model_picker.as_mut() {
                            picker.filter.extend(text.chars().filter(|c| !c.is_control()));
                            picker.selected = 0;
                            continue;
                        }
                        if self.matrix.is_some() {
                            self.stop_matrix();
                            continue;
                        }
                        self.stop_boot();
                        self.handle_paste(&text);
                    }
                    Event::Mouse(mouse) if self.matrix.is_none() && self.tetris.is_none() && self.pacman.is_none() => self.handle_mouse(mouse),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Hovering or clicking the chat focuses it; hovering or clicking the input focuses the input.
    /// Left click inside the input box also moves the cursor there. The wheel scrolls the chat.
    fn handle_mouse(&mut self, mouse: MouseEvent) {
        let over_chat = contains_cell(self.chat_area, mouse.column, mouse.row);
        let over_input = contains_cell(self.input_area, mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Moved | MouseEventKind::Down(_) => {
                if over_chat {
                    self.chat_focused = true;
                } else if over_input {
                    self.chat_focused = false;
                }
            }
            _ => {}
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if over_input => {
                // Inside the borders only.
                let area = self.input_area;
                if !(mouse.column > area.x && mouse.column < area.right().saturating_sub(1)
                    && mouse.row > area.y && mouse.row < area.bottom().saturating_sub(1))
                {
                    return;
                }
                let layout = layout_input(&self.input, area.width.saturating_sub(2));
                let col = mouse.column - area.x - 1;
                let row = mouse.row - area.y - 1 + self.input_scroll_row;
                self.cursor = layout.cursor_at_cell(col, row);
                self.last_yank = None;
            }
            MouseEventKind::ScrollUp => self.scroll_up = self.scroll_up.saturating_add(WHEEL_STEP),
            MouseEventKind::ScrollDown => self.scroll_up = self.scroll_up.saturating_sub(WHEEL_STEP),
            _ => {}
        }
    }

    /// Inserts pasted text at the cursor without submitting, keeping its newlines.
    fn handle_paste(&mut self, text: &str) {
        self.chat_focused = false;
        for c in text.replace("\r\n", "\n").replace('\r', "\n").chars() {
            self.insert_char(c);
        }
        self.input_pasted = true;
    }

    /// Slash commands whose name starts with the typed input. Empty unless the input is a
    /// typed (not pasted) single `/word`.
    fn slash_matches(&self) -> Vec<(&'static str, &'static str)> {
        if self.input_pasted || !self.input.starts_with('/') || self.input.contains(char::is_whitespace) {
            return Vec::new();
        }
        SLASH_COMMANDS
            .iter()
            .filter(|(name, _)| name.starts_with(self.input.as_str()))
            .copied()
            .collect()
    }

    /// Tab: completes the input to the first matching slash command.
    fn complete_slash_command(&mut self) {
        if let Some((name, _)) = self.slash_matches().first().copied() {
            self.set_input(name.to_string());
        }
    }

    /// Runs a slash command. Returns false if the input is not one.
    fn run_slash_command(&mut self, name: &str) -> bool {
        match name {
            "/matrix" => self.matrix = Some(MatrixRain::new(0, 0)),
            "/tetris" => self.tetris = Some(Tetris::new()),
            "/pacman" => self.pacman = Some(Pacman::new()),
            "/model" => self.open_model_picker(),
            "/about" => {
                // Skip the `# Minerve` heading and blank lines: the first real sentence is the description.
                let first_line = include_str!("../README.md")
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty() && !line.starts_with('#'))
                    .unwrap_or_default();
                self.notices.push(first_line.to_string());
            }
            _ => return false,
        }
        self.input.clear();
        self.cursor = 0;
        self.scroll_up = 0;
        self.input_pasted = false;
        true
    }

    fn handle_ui_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Transcript(messages) => self.transcript = messages,
            UiEvent::Status(status) => self.status = status,
            UiEvent::ModelList(result) => {
                if let Some(picker) = self.model_picker.as_mut() {
                    picker.all_models = Some(result);
                }
            }
        }
    }

    /// Opens the `/model` picker and fetches the OpenRouter model list in the background.
    fn open_model_picker(&mut self) {
        let current_model = self.minerve.active_model().id;
        self.model_picker = Some(ModelPicker::new(current_model));
        let minerve = self.minerve.clone();
        let ui = self.ui_tx.clone();
        crate::get_global_runtime().spawn(async move {
            let result = crate::openrouter::fetch_model_ids(&minerve.client, &minerve.base_url).await;
            let _ = ui.send(UiEvent::ModelList(result));
        });
    }

    /// Keys while the model picker is open. Enter switches model, Esc closes.
    fn handle_model_picker_key(&mut self, key: KeyEvent) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        let rows = picker.visible_rows(&self.recent_models);
        match key.code {
            KeyCode::Esc => self.model_picker = None,
            KeyCode::Up => picker.move_selection(-1, rows.len()),
            KeyCode::Down => picker.move_selection(1, rows.len()),
            KeyCode::Enter => {
                let Some(row) = rows.get(picker.selected) else {
                    return;
                };
                let model_id = row.model_id.clone();
                self.minerve.set_model(model_id.clone());
                self.minerve.fetch_context_window_in_background();
                self.recent_models.mark_used(&model_id);
                self.status = format!("model: {}", model_id);
                self.model_picker = None;
            }
            KeyCode::Backspace => {
                picker.filter.pop();
                picker.clamp_selection(picker.visible_rows(&self.recent_models).len());
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                picker.filter.push(c);
                picker.selected = 0;
            }
            _ => {}
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

        // Only an immediately preceding yank lets Alt+Y cycle; any other key ends the yank.
        let previous_yank = self.last_yank.take();
        if ctrl && key.code == KeyCode::Char('y') {
            self.yank();
            return;
        }
        if alt && key.code == KeyCode::Char('y') {
            self.yank_pop(previous_yank);
            return;
        }

        // Editing keys hand focus back to the input, so they are not swallowed by a focused chat.
        if self.chat_focused && matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete | KeyCode::Enter) {
            self.chat_focused = false;
        }

        match key.code {
            KeyCode::Enter if shift => self.insert_char('\n'),
            KeyCode::Enter => self.submit(),
            KeyCode::Char('j') if ctrl => self.insert_char('\n'),
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.input_len(),
            KeyCode::Char('k') if ctrl => self.kill_input(),
            KeyCode::Char('q') if ctrl => self.should_quit = true,
            KeyCode::Char('o') if ctrl => self.tools_expanded = !self.tools_expanded,
            KeyCode::Tab => self.complete_slash_command(),
            KeyCode::Char(c) if !ctrl && !alt => self.insert_char(c),
            KeyCode::Backspace => self.delete_before_cursor(),
            KeyCode::Delete => self.delete_at_cursor(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input_len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input_len(),
            KeyCode::Up if self.chat_focused => self.scroll_up = self.scroll_up.saturating_add(WHEEL_STEP),
            KeyCode::Down if self.chat_focused => self.scroll_up = self.scroll_up.saturating_sub(WHEEL_STEP),
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
        // A typed `/command` (or the first menu match for a partial one) runs locally.
        if let Some((name, _)) = self.slash_matches().first().copied() {
            if self.run_slash_command(name) {
                return;
            }
        }
        let content = self.input.clone();
        self.history.add_prompt(content.clone());

        // `!command` runs locally and is only kept for the next message; it is not sent yet.
        if let Some(command) = content.trim().strip_prefix('!') {
            let command = command.trim();
            if !command.is_empty() {
                let output = crate::tools::run_shell_command_tool::RunShellCommandTool::execute_command(command, None);
                self.pending_command_output
                    .push(format!("```\n$ {}\n{}\n```", command, output.trim_end()));
                self.input.clear();
                self.cursor = 0;
                self.scroll_up = 0;
                self.status = format!("queued output of `{}` for the next message", command);
            }
            return;
        }

        let content = if self.pending_command_output.is_empty() {
            content
        } else {
            let context = self.pending_command_output.join("\n\n");
            self.pending_command_output.clear();
            format!("Output of commands I ran in the shell:\n```\n{}\n```\n\n{}", context, content)
        };
        self.minerve
            .clone()
            .chat_with_arc(content, self.ui_tx.clone(), false);

        self.input.clear();
        self.cursor = 0;
        self.scroll_up = 0;
        self.input_pasted = false;
        self.notices.clear();
    }

    fn set_input(&mut self, text: String) {
        self.cursor = text.chars().count();
        self.input = text;
        // History and completion text is typed, not pasted.
        self.input_pasted = false;
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

    /// Ctrl+K: removes the whole input and keeps it in the kill ring.
    fn kill_input(&mut self) {
        if self.input.is_empty() {
            return;
        }
        let killed = std::mem::take(&mut self.input);
        self.cursor = 0;
        self.kill_ring.push(killed);
        if self.kill_ring.len() > KILL_RING_MAX {
            self.kill_ring.remove(0);
        }
    }

    /// Ctrl+Y: inserts the newest kill at the cursor.
    fn yank(&mut self) {
        let Some(text) = self.kill_ring.last().cloned() else {
            return;
        };
        let start = self.cursor;
        self.insert_str_at_cursor(&text);
        self.last_yank = Some(YankState {
            start,
            len: text.chars().count(),
            ring_offset: 0,
        });
    }

    /// Alt+Y: right after a yank, replaces the yanked text with the next older kill (wrapping).
    /// Does nothing if the previous key was not a yank.
    fn yank_pop(&mut self, previous: Option<YankState>) {
        let Some(state) = previous else {
            return;
        };
        let ring_len = self.kill_ring.len();
        if ring_len == 0 {
            return;
        }
        let ring_offset = (state.ring_offset + 1) % ring_len;
        let text = self.kill_ring[ring_len - 1 - ring_offset].clone();

        // Remove the previously yanked text, then insert the replacement at the same spot.
        let start_byte = self.byte_index(state.start);
        let end_byte = self.byte_index(state.start + state.len);
        self.input.replace_range(start_byte..end_byte, &text);
        self.cursor = state.start + text.chars().count();
        self.last_yank = Some(YankState {
            start: state.start,
            len: text.chars().count(),
            ring_offset,
        });
    }

    /// Inserts `text` at the cursor and moves the cursor past it.
    fn insert_str_at_cursor(&mut self, text: &str) {
        let idx = self.byte_index(self.cursor);
        self.input.insert_str(idx, text);
        self.cursor += text.chars().count();
    }

    /// Stops the video now (key press). The dotted ring then erodes away; the boot state is
    /// dropped and the screen cleared once the erosion finishes (see `draw_boot_overlay`).
    fn stop_boot(&mut self) {
        self.boot = None;
    }

    /// Starts the `/matrix` fade-out. The screen returns to the chat once it has dimmed.
    fn stop_matrix(&mut self) {
        if let Some(matrix) = self.matrix.as_mut() {
            matrix.stop();
        }
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
        if let Some(game) = self.tetris.as_ref() {
            game.draw(frame, frame.area());
            return;
        }

        if let Some(game) = self.pacman.as_ref() {
            game.draw(frame, frame.area());
            return;
        }

        if let Some(picker) = self.model_picker.as_ref() {
            let rows = picker.visible_rows(&self.recent_models);
            picker.draw(frame, frame.area(), &rows);
            return;
        }

        // The rain replaces the whole UI until a key or paste ends it.
        if let Some(matrix) = self.matrix.as_mut() {
            if matrix.draw(frame, frame.area()) {
                return;
            }
            // The fade-out finished: fall through to the normal UI.
            self.matrix = None;
        }

        // Input grows with its wrapped text: 1 line minimum, at most 8 lines of text.
        let inner_width = frame.area().width.saturating_sub(2);
        let row_count = layout_input(&self.input, inner_width).rows.len() as u16;
        let input_height = (row_count + 2).clamp(3, 10);

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
        self.draw_slash_menu(frame, input_area);

        frame.render_widget(
            Paragraph::new(
                "Enter send | Esc cancel turn | Shift+Enter newline | Up/Down history | PgUp/PgDn scroll | Ctrl+O tool output | Ctrl+C quit",
            )
            .style(Style::default().fg(Color::DarkGray)),
            hint_area,
        );

        if self.boot.is_some() {
            self.draw_boot_overlay(frame, chat_area);
        }
    }

    fn draw_chat(&mut self, frame: &mut Frame, area: Rect) {
        self.chat_area = area;
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
            lines.extend(crate::markdown::render(&text));
            lines.push(Line::default());
        }

        // Results of local slash commands. Not sent to the model.
        for notice in &self.notices {
            lines.push(Line::styled(
                "Command:",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::styled(notice.clone(), Style::default().fg(Color::Gray)));
            lines.push(Line::default());
        }

        // Shell output queued with `!command`: shown here, sent to the model with the next message.
        for output in &self.pending_command_output {
            lines.push(Line::styled(
                "Shell (queued, sent with your next message):",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
            lines.extend(crate::markdown::render(output));
            lines.push(Line::default());
        }
        lines
    }

    /// Popup listing the slash commands that match the input, just above the input box.
    fn draw_slash_menu(&self, frame: &mut Frame, input_area: Rect) {
        let matches = self.slash_matches();
        if matches.is_empty() {
            return;
        }
        let height = (matches.len() as u16 + 2).min(input_area.y);
        let area = Rect {
            x: input_area.x,
            y: input_area.y.saturating_sub(height),
            width: input_area.width,
            height,
        };
        let lines: Vec<Line> = matches
            .iter()
            .map(|(name, description)| {
                Line::from(vec![
                    Span::styled(
                        format!("{:<12}", name),
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(description.to_string(), Style::default().fg(Color::Gray)),
                ])
            })
            .collect();
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Commands (Tab complete, Enter run) ")
                    .border_style(Style::default().fg(Color::Cyan)),
            ),
            area,
        );
    }

    fn draw_input(&mut self, frame: &mut Frame, area: Rect) {
        let title = " Message ";
        let border_color = Color::Cyan;

        // Inside the borders.
        let inner_width = area.width.saturating_sub(2);
        let inner_height = area.height.saturating_sub(2);
        let layout = layout_input(&self.input, inner_width);
        let (col, row) = layout.cursor_cell(self.cursor);
        // Keep the cursor row visible once the wrapped input is taller than the box.
        let scroll_row = row.saturating_add(1).saturating_sub(inner_height);
        self.input_area = area;
        self.input_scroll_row = scroll_row;

        // Rows are pre-wrapped by display width (no Paragraph wrap) so they match the cursor position.
        let input = Paragraph::new(layout.rows.join("\n"))
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
}

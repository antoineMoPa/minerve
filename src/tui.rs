//! Terminal UI built with ratatui. Runs on the main thread and owns the terminal.
//!
//! Background work reports through `UiEvent`s (see `ui.rs`); key presses are handled here.

use std::io::{self, Write};
use std::sync::mpsc::{self, Receiver};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::chat_selection::{self, ChatSelection};
use crate::control_socket::{ControlSocket, ParentCommand, ScrollTarget};
use crate::input_layout::layout_input;
use crate::model_picker::ModelPicker;
use crate::provider::{Provider, PROVIDERS_BY_DEFAULT_PRIORITY};
use crate::setup::{self, SetupModal, SetupStep};
use crate::recent_models::RecentModels;
use crate::mcp_manager::{McpManager, McpManagerStep};
use crate::message_apps::{MessageApps, MessageAppsStep};
use crate::session_picker::SessionPicker;

use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, DisableFocusChange, EnableBracketedPaste, EnableFocusChange, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
    KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::boot::BootAnimation;
use crate::history::HistoryTracker;
use crate::minerve::{Minerve, GOAL_EVALUATOR_UI_ROLE};
use crate::openrouter::OpenRouterConnection;
use crate::session::SessionStart;
use crate::tetris::Tetris;
use crate::ui::{UiEvent, UiSender};

mod draw;
mod editing;
mod matrix;
mod pickers;

use matrix::MatrixRain;

/// Something the user sent while a turn was running. It waits until the turn ends.
enum QueuedPrompt {
    /// A normal message, with the image files pasted into it.
    Message { content: String, images: Vec<std::path::PathBuf> },
    /// A `/goal <goal>` to pursue.
    Goal(String),
}

/// Tool results longer than this are cut short in the chat view (the model still gets them all).
const MAX_OUTPUT_LEN: usize = 500;
/// Lines scrolled per PageUp/PageDown.
const SCROLL_STEP: usize = 10;
/// Chat lines scrolled per mouse wheel notch.
const WHEEL_STEP: usize = 3;
/// Slash commands offered by the `/` menu: (name, one-line description).
const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/about", "describe Minerve"),
    ("/model", "switch models and providers"),
    ("/goal", "keep working until an evaluator model judges the goal reached: /goal <goal>"),
    ("/resume", "continue a saved session"),
    ("/clear", "start a new conversation from fresh"),
    ("/apikeys", "set the OpenRouter, OpenAI and Anthropic keys"),
    ("/mcp", "manage MCP servers: add, connect, disconnect, remove"),
    ("/message-apps", "talk to minerve from Slack: set up, status, link your account"),
    ("/bootscreen", "replay the boot screen"),
    ("/matrix", "falling letters; any key to leave"),
    ("/tetris", "play a mini Tetris; esc to leave"),
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
    /// Prompts sent while a turn was running, with their images, oldest first. The run loop sends one
    /// each time a turn ends.
    queued_prompts: std::collections::VecDeque<QueuedPrompt>,
    /// Images pasted into the prompt being typed, shown as `[Image #N]` tokens.
    pasted_images: crate::images::PastedImages,
    /// True while the input holds pasted text. Pasted text never opens the slash menu or runs a command.
    input_pasted: bool,
    /// Results of local slash commands (e.g. `/about`), shown in the chat until the next message.
    notices: Vec<String>,
    /// Falling-letter screen from `/matrix`. While set it replaces the whole UI; any key or paste ends it.
    matrix: Option<MatrixRain>,
    /// Tetris game from `/tetris`. While set it replaces the whole UI; Esc or q leaves it.
    tetris: Option<Tetris>,
    /// Model picker from `/model`. While set it replaces the whole UI; Esc closes it.
    model_picker: Option<ModelPicker>,
    /// First-start setup modal, drawn over the chat. While set it takes every key and paste.
    setup_modal: Option<SetupModal>,
    /// Saved sessions from `/resume`. While set it replaces the whole UI; Esc closes it.
    session_picker: Option<SessionPicker>,
    /// MCP server manager from `/mcp`, drawn over the chat. While set it takes every key and paste.
    mcp_manager: Option<McpManager>,
    message_apps: Option<MessageApps>,
    /// Least recently used models, shown at the top of the picker.
    recent_models: RecentModels,
    /// Input box as last drawn, and how many wrapped rows it was scrolled. Maps mouse clicks to the cursor.
    input_area: Rect,
    input_scroll_row: u16,
    /// Dimmed hint shown while the message box is empty, typed out one character at a time.
    pro_tip: crate::pro_tips::ProTips,
    /// Chat as last drawn. Hovering or clicking it gives it focus.
    chat_area: Rect,
    /// True when the chat has focus: Up/Down scroll it and typing gives focus back to the input.
    chat_focused: bool,
    /// Chat row shown at the top of the chat area in the last draw. A new selection remembers it.
    chat_top: usize,
    /// Text dragged over in the chat, highlighted until Esc, a click, or the chat moving.
    chat_selection: Option<ChatSelection>,
    /// Set when the drag ends: the next draw copies the selected cells to the clipboard.
    copy_selection_on_draw: bool,
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

/// True when the terminal cell at (column, row) is inside `area`.
fn contains_cell(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x && column < area.right() && row >= area.y && row < area.bottom()
}

/// Turns mouse reporting on or off. Off, the terminal selects text with a plain drag.
/// Mode 1003 reports moves without a button held, so hovering the chat gives it focus;
/// crossterm's EnableMouseCapture does not turn it on.
pub(crate) fn set_mouse_capture(on: bool) {
    if on {
        let _ = execute!(io::stdout(), EnableMouseCapture);
        let _ = io::stdout().write_all(b"\x1b[?1003h");
    } else {
        let _ = io::stdout().write_all(b"\x1b[?1003l");
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
}

/// Runs the interactive UI until the user quits.
pub fn run(
    openrouter_connection: OpenRouterConnection,
    start: SessionStart,
    extra_system_prompts: &[String],
    force_setup: bool,
    control: Option<ControlSocket>,
) -> io::Result<()> {
    let (ui_tx, ui_rx) = mpsc::channel();
    let control = control.map(Arc::new);
    if let Some(control) = &control {
        let commands_tx = ui_tx.clone();
        control.read_commands_in_background(
            move |command| {
                let _ = commands_tx.send(UiEvent::ParentCommand(command));
            },
            || {},
        );
    }
    // The boot clip is for fresh sessions only; resuming one goes straight to the chat.
    let boot = match start {
        SessionStart::Resume(_) => None,
        SessionStart::New | SessionStart::NewWithId(_) => BootAnimation::start(),
    };
    let needs_setup = force_setup
        || setup::needs_setup(&openrouter_connection.api_keys);
    let stored_keys = openrouter_connection.api_keys.clone();
    let minerve = Arc::new(Minerve::new(
        openrouter_connection,
        start,
        extra_system_prompts,
        control,
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
        queued_prompts: std::collections::VecDeque::new(),
        input_pasted: false,
        pasted_images: Default::default(),
        notices: Vec::new(),
        matrix: None,
        tetris: None,
        model_picker: None,
        setup_modal: needs_setup.then(|| SetupModal::new(stored_keys.clone())),
        session_picker: None,
        mcp_manager: None,
        message_apps: None,
        recent_models: RecentModels::load(),
        input_area: Rect::default(),
        input_scroll_row: 0,
        pro_tip: crate::pro_tips::ProTips::new(Instant::now()),
        chat_area: Rect::default(),
        chat_focused: false,
        chat_top: 0,
        chat_selection: None,
        copy_selection_on_draw: false,
    };

    let mut terminal = ratatui::init();
    // Pasted text arrives as one `Event::Paste` instead of many key presses,
    // so newlines in a paste do not submit the prompt.
    let _ = execute!(io::stdout(), EnableBracketedPaste);
    // Focus changes repaint the UI in full: the terminal may have drawn over the screen while minerve was in the background.
    let _ = execute!(io::stdout(), EnableFocusChange);
    // Clicks move the cursor in the input box and the wheel scrolls the chat.
    // Terminals then select text only with a modifier held (Shift in most, Option in iTerm2).
    set_mouse_capture(true);
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
    set_mouse_capture(false);
    let _ = execute!(io::stdout(), DisableFocusChange, DisableBracketedPaste);
    ratatui::restore();
    // Printed after the restore so it lands on the normal screen, not the alternate one.
    println!("{}", app.minerve.current_session_id());
    result
}

impl App {
    fn run_loop(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.should_quit {
            while let Ok(event) = self.ui_rx.try_recv() {
                self.handle_ui_event(event);
            }
            self.send_next_queued_prompt();

            if let Some(game) = self.tetris.as_mut() {
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
                        }
                        if !quit_key {
                            if let Some(manager) = self.mcp_manager.as_mut() {
                                if let McpManagerStep::Close = manager.handle_key(key) {
                                    self.mcp_manager = None;
                                }
                                continue;
                            }
                            if let Some(apps) = self.message_apps.as_mut() {
                                if let MessageAppsStep::Close = apps.handle_key(key) {
                                    self.message_apps = None;
                                }
                                continue;
                            }
                        }
                        if !quit_key && self.session_picker.is_some() {
                            self.handle_session_picker_key(key);
                            continue;
                        }
                        // The model picker takes every key, Ctrl+C still quits.
                        if !quit_key && self.model_picker.is_some() {
                            self.handle_model_picker_key(key);
                            continue;
                        }
                        // The setup modal waits for the boot clip: the first key only ends the clip.
                        if !quit_key && self.boot.is_some() && self.setup_modal.is_some() {
                            self.stop_boot();
                            continue;
                        }
                        if !quit_key && self.setup_modal.is_some() {
                            self.handle_setup_key(key);
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
                        if self.tetris.is_some() {
                            continue;
                        }
                        if let Some(modal) = self.setup_modal.as_mut() {
                            modal.paste(&text);
                            continue;
                        }
                        if let Some(manager) = self.mcp_manager.as_mut() {
                            manager.paste(&text);
                            continue;
                        }
                        if let Some(apps) = self.message_apps.as_mut() {
                            apps.paste(&text);
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
                    Event::Mouse(mouse) if self.matrix.is_none() && self.tetris.is_none() && self.mcp_manager.is_none() && self.message_apps.is_none() => self.handle_mouse(mouse),
                    // Back in the terminal after another app had the screen, or resized: repaint every cell.
                    Event::FocusGained => terminal.clear()?,
                    // The chat rewraps, so a selection's cells no longer hold its text.
                    Event::Resize(_, _) => {
                        self.chat_selection = None;
                        terminal.clear()?
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Hovering or clicking the chat focuses it; hovering or clicking the input focuses the input.
    /// Left click inside the input box also moves the cursor there. The wheel scrolls the chat.
    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if let Some(picker) = self.session_picker.as_ref() {
            if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                if let Some(index) = picker.row_at(mouse.column, mouse.row) {
                    let id = picker.sessions[index].id.clone();
                    self.resume_session(&id);
                }
            }
            return;
        }
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
            MouseEventKind::Down(MouseButton::Left) => {
                self.chat_selection = over_chat
                    .then(|| ChatSelection::new((mouse.column, mouse.row), self.chat_top));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(selection) = self.chat_selection.as_mut() {
                    selection.cursor = chat_selection::clamp_to(self.chat_area, (mouse.column, mouse.row));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => match self.chat_selection {
                // A click without a drag selects nothing.
                Some(selection) if selection.is_empty() => self.chat_selection = None,
                Some(_) => self.copy_selection_on_draw = true,
                None => {}
            },
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

    /// Inserts pasted text at the cursor without submitting, keeping its newlines. A paste
    /// without text (Cmd+V with an image on the clipboard) attaches the clipboard image, and
    /// the path of an image file (a file dragged onto the terminal) attaches that file.
    fn handle_paste(&mut self, text: &str) {
        self.chat_focused = false;
        self.pro_tip.restart_idle_delay(Instant::now());
        if text.trim().is_empty() {
            self.paste_clipboard_image();
            return;
        }
        if let Some(path) = crate::images::pasted_image_path(text) {
            match crate::images::image_to_send(&path) {
                Ok(to_send) => self.attach_image(to_send),
                Err(err) => self.status = err,
            }
            return;
        }
        for c in text.replace("\r\n", "\n").replace('\r', "\n").chars() {
            self.insert_char(c);
        }
        self.input_pasted = true;
    }

    /// Ctrl+V: attaches the image on the clipboard. Terminals only paste text, so minerve reads the clipboard itself.
    fn paste_clipboard_image(&mut self) {
        match crate::images::paste_clipboard_image() {
            Ok(Some(path)) => self.attach_image(path),
            Ok(None) => self.status = String::from("no image on the clipboard"),
            Err(err) => self.status = err,
        }
    }

    fn attach_image(&mut self, path: std::path::PathBuf) {
        let token = self.pasted_images.attach(path);
        for c in token.chars() {
            self.insert_char(c);
        }
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
            "/model" => self.open_model_picker(),
            "/resume" => self.open_session_picker(),
            "/clear" => self.clear_conversation(),
            "/apikeys" => self.setup_modal = Some(SetupModal::new(self.minerve.api_keys_snapshot())),
            "/mcp" => self.mcp_manager = Some(McpManager::new()),
            "/message-apps" => self.message_apps = Some(MessageApps::new()),
            "/bootscreen" => self.boot = BootAnimation::start(),
            // Without a goal after it: `submit` handles `/goal <goal>`.
            "/goal" => self.notices.push(String::from(
                "Usage: /goal <goal>. Minerve keeps working, turn after turn, until an evaluator model judges the goal reached. Esc stops it.",
            )),
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
            UiEvent::ModelList(provider, result) => {
                if let Some(picker) = self.model_picker.as_mut() {
                    picker.lists.push((provider, result));
                }
            }
            UiEvent::Notice(notice) => self.notices.push(notice),
            UiEvent::ParentCommand(command) => self.handle_parent_command(command),
            UiEvent::ParentClosed => {}
        }
    }

    /// A command of the program that started minerve - see `control_socket`.
    fn handle_parent_command(&mut self, command: ParentCommand) {
        match command {
            ParentCommand::Scroll { to: ScrollTarget::Bottom } => self.scroll_up = 0,
            // `draw_chat` clamps this to the top of the chat.
            ParentCommand::Scroll { to: ScrollTarget::Top } => self.scroll_up = usize::MAX,
            ParentCommand::ScrollBy { lines } => {
                self.scroll_up = if lines < 0 {
                    self.scroll_up.saturating_add(lines.unsigned_abs() as usize)
                } else {
                    self.scroll_up.saturating_sub(lines as usize)
                };
            }
            ParentCommand::InsertText { text } => self.insert_str_at_cursor(&text),
            ParentCommand::Submit { text, .. } => {
                if self.minerve.request_in_flight.load(Ordering::SeqCst) {
                    self.status = String::from("a prompt from the parent program was refused: a turn is running");
                    return;
                }
                // Whatever was being typed in the box stays there, under the parent's prompt.
                let draft = std::mem::take(&mut self.input);
                let cursor = self.cursor;
                self.set_input(text);
                self.submit();
                self.input = draft;
                self.cursor = cursor;
            }
            ParentCommand::GetHistory {} => self.minerve.send_history_to_control(),
            // Only the GUI's `--serve` minerve acts on these.
            _ => {}
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        self.pro_tip.restart_idle_delay(Instant::now());

        if ctrl && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        // Esc clears the chat selection first, then stops the running turn.
        if key.code == KeyCode::Esc {
            if self.chat_selection.take().is_some() {
                return;
            }
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
            KeyCode::Char('v') if ctrl => self.paste_clipboard_image(),
            KeyCode::Tab => self.complete_slash_command(),
            KeyCode::Char(c) if !ctrl && !alt => self.insert_char(c),
            KeyCode::Backspace => self.delete_before_cursor(),
            KeyCode::Delete => self.delete_at_cursor(),
            KeyCode::Left if ctrl => self.word_left(),
            KeyCode::Right if ctrl => self.word_right(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input_len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input_len(),
            KeyCode::Up if self.chat_focused => self.scroll_up = self.scroll_up.saturating_add(WHEEL_STEP),
            KeyCode::Down if self.chat_focused => self.scroll_up = self.scroll_up.saturating_sub(WHEEL_STEP),
            KeyCode::Up => {
                if let Some(prompt) = self.history.get_previous_prompt(&self.input) {
                    self.set_input(prompt);
                }
            }
            KeyCode::Down => {
                if let Some(prompt) = self.history.get_next_prompt() {
                    self.set_input(prompt);
                }
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
        if let Some(goal) = self.input.strip_prefix("/goal ").map(str::trim) {
            if !goal.is_empty() && !self.input_pasted {
                self.start_goal(goal.to_string());
                return;
            }
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
        let images = self.pasted_images.take_referenced(&content);
        // One turn at a time: a second prompt would run beside the first in the same session.
        // While a turn runs, the prompt waits in the queue (see `send_next_queued_prompt`).
        if self.must_queue() {
            self.queued_prompts.push_back(QueuedPrompt::Message { content, images });
            self.status = format!("{} queued", self.queued_prompts.len());
        } else {
            self.minerve
                .clone()
                .chat_with_arc(content, images, self.ui_tx.clone(), false);
        }

        self.input.clear();
        self.cursor = 0;
        self.scroll_up = 0;
        self.input_pasted = false;
        self.notices.clear();
    }

    /// True when a new prompt must wait: a turn is running, or older prompts are still queued.
    fn must_queue(&self) -> bool {
        self.minerve.request_in_flight.load(Ordering::SeqCst) || !self.queued_prompts.is_empty()
    }

    /// Sends the oldest queued prompt once no turn is running. Called on every loop pass.
    fn send_next_queued_prompt(&mut self) {
        if self.minerve.request_in_flight.load(Ordering::SeqCst) {
            return;
        }
        let Some(queued) = self.queued_prompts.pop_front() else {
            return;
        };
        match queued {
            QueuedPrompt::Message { content, images } => {
                self.minerve.clone().chat_with_arc(content, images, self.ui_tx.clone(), false);
            }
            QueuedPrompt::Goal(goal) => self.minerve.clone().pursue_goal(goal, self.ui_tx.clone()),
        }
        self.status = String::new();
    }

    /// `/goal <goal>`: starts the goal loop, see `Minerve::pursue_goal`.
    fn start_goal(&mut self, goal: String) {
        self.history.add_prompt(self.input.clone());
        if self.must_queue() {
            self.queued_prompts.push_back(QueuedPrompt::Goal(goal));
            self.status = format!("{} queued", self.queued_prompts.len());
        } else {
            self.minerve.clone().pursue_goal(goal, self.ui_tx.clone());
        }
        self.input.clear();
        self.cursor = 0;
        self.scroll_up = 0;
        self.notices.clear();
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

}

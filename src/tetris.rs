//! `/tetris`: a small Tetris game that replaces the chat until the player leaves with Esc or q.
//!
//! Pieces are four cells around a pivot cell. Rotation turns the offsets 90 degrees around the
//! pivot, with a few horizontal kicks so a piece can rotate against a wall.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

const WIDTH: usize = 10;
const HEIGHT: usize = 20;
/// Points for clearing 0 to 4 lines at once, before multiplying by the level.
const LINE_POINTS: [u32; 5] = [0, 100, 300, 500, 800];
const LINES_PER_LEVEL: u32 = 10;
/// Horizontal offsets tried, in order, when a rotation does not fit where it is.
const ROTATION_KICKS: [i32; 5] = [0, -1, 1, -2, 2];

/// Offsets of the four cells from the pivot, for each piece: I, O, T, S, Z, J, L.
const SHAPES: [[(i32, i32); 4]; 7] = [
    [(-1, 0), (0, 0), (1, 0), (2, 0)],
    [(0, 0), (1, 0), (0, 1), (1, 1)],
    [(-1, 0), (0, 0), (1, 0), (0, -1)],
    [(0, 0), (1, 0), (-1, 1), (0, 1)],
    [(-1, 0), (0, 0), (0, 1), (1, 1)],
    [(-1, 0), (0, 0), (1, 0), (-1, 1)],
    [(-1, 0), (0, 0), (1, 0), (1, 1)],
];

/// Colour per piece, in the order of `SHAPES`.
const COLORS: [Color; 7] = [
    Color::Cyan,
    Color::Yellow,
    Color::Magenta,
    Color::Green,
    Color::Red,
    Color::Blue,
    Color::Rgb(255, 165, 0),
];

/// The falling piece: its pivot position on the board and its current cell offsets.
#[derive(Clone, Copy)]
struct Piece {
    kind: usize,
    x: i32,
    y: i32,
    offsets: [(i32, i32); 4],
}

impl Piece {
    fn spawn(kind: usize) -> Self {
        Self { kind, x: WIDTH as i32 / 2, y: 1, offsets: SHAPES[kind] }
    }

    fn cells(&self) -> [(i32, i32); 4] {
        self.offsets.map(|(dx, dy)| (self.x + dx, self.y + dy))
    }

    fn rotated(&self) -> Self {
        Self { offsets: self.offsets.map(|(dx, dy)| (-dy, dx)), ..*self }
    }
}

/// Xorshift64 step; returns a value in [0, n). Avoids a crate for the piece bag.
fn next_index(state: &mut u64, n: usize) -> usize {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state % n as u64) as usize
}

pub struct Tetris {
    /// Locked cells: the piece kind that filled each one.
    board: [[Option<usize>; WIDTH]; HEIGHT],
    piece: Piece,
    next: usize,
    /// Remaining kinds of the current shuffled bag. Every bag holds each piece once.
    bag: Vec<usize>,
    rng: u64,
    score: u32,
    lines: u32,
    last_fall: Instant,
    over: bool,
}

impl Tetris {
    pub fn new() -> Self {
        let rng = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1)
            | 1;
        let mut game = Self {
            board: [[None; WIDTH]; HEIGHT],
            piece: Piece::spawn(0),
            next: 0,
            bag: Vec::new(),
            rng,
            score: 0,
            lines: 0,
            last_fall: Instant::now(),
            over: false,
        };
        game.piece = Piece::spawn(game.take_from_bag());
        game.next = game.take_from_bag();
        game
    }

    fn take_from_bag(&mut self) -> usize {
        if self.bag.is_empty() {
            self.bag = (0..SHAPES.len()).collect();
            for i in (1..self.bag.len()).rev() {
                let j = next_index(&mut self.rng, i + 1);
                self.bag.swap(i, j);
            }
        }
        self.bag.pop().expect("bag was refilled above")
    }

    fn level(&self) -> u32 {
        1 + self.lines / LINES_PER_LEVEL
    }

    fn fall_interval(&self) -> Duration {
        let ms = 800u64.saturating_sub(70 * u64::from(self.level() - 1)).max(100);
        Duration::from_millis(ms)
    }

    fn fits(&self, piece: &Piece) -> bool {
        piece.cells().iter().all(|&(x, y)| {
            (0..WIDTH as i32).contains(&x)
                && (0..HEIGHT as i32).contains(&y)
                && self.board[y as usize][x as usize].is_none()
        })
    }

    fn try_move(&mut self, dx: i32, dy: i32) -> bool {
        let moved = Piece { x: self.piece.x + dx, y: self.piece.y + dy, ..self.piece };
        if self.fits(&moved) {
            self.piece = moved;
            true
        } else {
            false
        }
    }

    fn rotate(&mut self) {
        let turned = self.piece.rotated();
        for kick in ROTATION_KICKS {
            let candidate = Piece { x: turned.x + kick, ..turned };
            if self.fits(&candidate) {
                self.piece = candidate;
                return;
            }
        }
    }

    /// Writes the piece into the board, clears full rows, and spawns the next piece.
    fn lock(&mut self) {
        for (x, y) in self.piece.cells() {
            self.board[y as usize][x as usize] = Some(self.piece.kind);
        }

        let kept: Vec<[Option<usize>; WIDTH]> = self
            .board
            .iter()
            .copied()
            .filter(|row| row.iter().any(|cell| cell.is_none()))
            .collect();
        let cleared = HEIGHT - kept.len();
        let mut board = [[None; WIDTH]; HEIGHT];
        for (i, row) in kept.into_iter().enumerate() {
            board[cleared + i] = row;
        }
        self.board = board;

        self.score += LINE_POINTS[cleared] * self.level();
        self.lines += cleared as u32;

        self.piece = Piece::spawn(self.next);
        self.next = self.take_from_bag();
        if !self.fits(&self.piece) {
            self.over = true;
        }
    }

    fn hard_drop(&mut self) {
        while self.try_move(0, 1) {
            self.score += 2;
        }
        self.lock();
    }

    /// Gravity: moves the piece down once the fall interval has passed, locking it when it lands.
    pub fn update(&mut self, now: Instant) {
        if self.over || now.duration_since(self.last_fall) < self.fall_interval() {
            return;
        }
        self.last_fall = now;
        if !self.try_move(0, 1) {
            self.lock();
        }
    }

    /// Handles one key press. Returns false when the player leaves the game.
    pub fn handle_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => return false,
            KeyCode::Char('r') if self.over => *self = Tetris::new(),
            _ if self.over => {}
            KeyCode::Left => {
                self.try_move(-1, 0);
            }
            KeyCode::Right => {
                self.try_move(1, 0);
            }
            KeyCode::Down => {
                if self.try_move(0, 1) {
                    self.score += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('x') => self.rotate(),
            KeyCode::Char(' ') => self.hard_drop(),
            _ => {}
        }
        true
    }

    /// Paints the board and the side panel centred in `area`.
    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(Clear, area);
        let board_w = (WIDTH * 2 + 2) as u16;
        let board_h = (HEIGHT + 2) as u16;
        let panel_w = 16u16;
        let total_w = board_w + 2 + panel_w;
        if area.width < total_w || area.height < board_h {
            let notice = format!("terminal too small for tetris: need {total_w}x{board_h}");
            frame.render_widget(Paragraph::new(notice), area);
            return;
        }

        let [_, rows, _] = Layout::vertical([Constraint::Min(0), Constraint::Length(board_h), Constraint::Min(0)]).areas(area);
        let [_, centre, _] = Layout::horizontal([Constraint::Min(0), Constraint::Length(total_w), Constraint::Min(0)]).areas(rows);
        let [board_area, _, panel] =
            Layout::horizontal([Constraint::Length(board_w), Constraint::Length(2), Constraint::Length(panel_w)]).areas(centre);

        // Locked cells plus the falling piece, drawn as one grid.
        let mut grid = self.board;
        for (x, y) in self.piece.cells() {
            grid[y as usize][x as usize] = Some(self.piece.kind);
        }

        let block = Block::bordered().title(" tetris ");
        let inner = block.inner(board_area);
        frame.render_widget(block, board_area);
        let buf = frame.buffer_mut();
        for (row, cells) in grid.iter().enumerate() {
            for (col, cell) in cells.iter().enumerate() {
                let x = inner.x + (col * 2) as u16;
                let y = inner.y + row as u16;
                match cell {
                    Some(kind) => buf.set_string(x, y, "██", Style::default().fg(COLORS[*kind])),
                    None => buf.set_string(x, y, " .", Style::default().fg(Color::DarkGray)),
                };
            }
        }

        let mut lines = vec![
            Line::from("score"),
            Line::from(self.score.to_string()),
            Line::from(""),
            Line::from("lines"),
            Line::from(self.lines.to_string()),
            Line::from(""),
            Line::from("level"),
            Line::from(self.level().to_string()),
            Line::from(""),
            Line::from("next"),
        ];
        // The preview sits in rows 11..=13 of the panel, under the "next" label.
        lines.extend([
            Line::from(""),
            Line::from(""),
            Line::from(""),
            Line::from(""),
            Line::from("←→ move  ↑ turn"),
            Line::from("↓ soft  space drop"),
            Line::from("esc/q leave"),
        ]);
        if self.over {
            lines.push(Line::from(""));
            lines.push(Line::from("game over"));
            lines.push(Line::from("r to restart"));
        }
        frame.render_widget(Paragraph::new(lines), panel);

        let buf = frame.buffer_mut();
        for (dx, dy) in SHAPES[self.next] {
            let x = panel.x + 4 + ((dx + 1) as u16) * 2;
            let y = panel.y + 11 + (dy + 1) as u16;
            buf.set_string(x, y, "██", Style::default().fg(COLORS[self.next]));
        }
    }
}

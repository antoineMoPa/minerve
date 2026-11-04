//! `/pacman`: a small Pac-Man game that replaces the chat until the player leaves with Esc or q.
//!
//! The maze is a fixed grid of walls, dots, power pellets and empty cells. The player and the
//! ghosts move one cell per tick. Eating a power pellet scares the ghosts for a while, and a
//! scared ghost that is touched is sent back to its start.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

/// `#` wall, `.` dot, `o` power pellet, space empty. Every row must have the same width.
const MAZE: [&str; 11] = [
    "###################",
    "#........#........#",
    "#o##.###.#.###.##o#",
    "#.................#",
    "#.##.#.#####.#.##.#",
    "#....#.......#....#",
    "#.######.#.######.#",
    "#.........#.......#",
    "#.#.###.#.#.###.#.#",
    "#.................#",
    "###################",
];
const PLAYER_START: (i32, i32) = (9, 3);
/// Ghost start cells, paired with `GHOST_COLORS` by index.
const GHOST_STARTS: [(i32, i32); 3] = [(9, 5), (8, 9), (10, 9)];
const GHOST_COLORS: [Color; 3] = [Color::Red, Color::Magenta, Color::Cyan];
const TICK: Duration = Duration::from_millis(150);
/// Ticks a power pellet keeps ghosts scared: 40 ticks at 150 ms is 6 seconds.
const FRIGHTENED_TICKS: u32 = 40;
/// Chance, in percent, that a ghost that is not scared heads straight for the player.
const CHASE_PERCENT: usize = 75;
const LIVES: u32 = 3;
const DOT_POINTS: u32 = 10;
const POWER_POINTS: u32 = 50;
const GHOST_POINTS: u32 = 200;
const DIRS: [(i32, i32); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cell {
    Wall,
    Dot,
    Power,
    Empty,
}

#[derive(Clone, Copy)]
struct Ghost {
    x: i32,
    y: i32,
    start: (i32, i32),
    dir: (i32, i32),
    /// Ticks left as a scared ghost. Zero means it hunts.
    frightened: u32,
    color: Color,
}

impl Ghost {
    fn pos(&self) -> (i32, i32) {
        (self.x, self.y)
    }

    fn reset(&mut self) {
        (self.x, self.y) = self.start;
        self.dir = (0, -1);
        self.frightened = 0;
    }
}

/// Xorshift64 step; returns a value in [0, n). Same generator as the Tetris piece bag.
fn next_index(state: &mut u64, n: usize) -> usize {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state % n as u64) as usize
}

pub struct Pacman {
    grid: Vec<Vec<Cell>>,
    player: (i32, i32),
    /// Direction the player is currently moving. (0, 0) until the first arrow key.
    heading: (i32, i32),
    /// Direction the player asked for. Taken as soon as the maze allows it.
    wanted: (i32, i32),
    ghosts: Vec<Ghost>,
    dots_left: u32,
    score: u32,
    lives: u32,
    rng: u64,
    last_tick: Instant,
    won: bool,
    over: bool,
}

impl Pacman {
    pub fn new() -> Self {
        let rng = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1)
            | 1;
        let grid: Vec<Vec<Cell>> = MAZE
            .iter()
            .map(|row| {
                row.chars()
                    .map(|c| match c {
                        '#' => Cell::Wall,
                        '.' => Cell::Dot,
                        'o' => Cell::Power,
                        ' ' => Cell::Empty,
                        other => panic!("unknown pacman maze cell {other:?}"),
                    })
                    .collect()
            })
            .collect();
        let dots_left = grid.iter().flatten().filter(|c| matches!(c, Cell::Dot | Cell::Power)).count() as u32;
        let ghosts = GHOST_STARTS
            .iter()
            .zip(GHOST_COLORS)
            .map(|(&start, color)| Ghost { x: start.0, y: start.1, start, dir: (0, -1), frightened: 0, color })
            .collect();
        let mut game = Self {
            grid,
            player: PLAYER_START,
            heading: (0, 0),
            wanted: (0, 0),
            ghosts,
            dots_left,
            score: 0,
            lives: LIVES,
            rng,
            last_tick: Instant::now(),
            won: false,
            over: false,
        };
        game.eat();
        game
    }

    fn is_open(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 {
            return false;
        }
        self.grid.get(y as usize).and_then(|row| row.get(x as usize)).is_some_and(|cell| *cell != Cell::Wall)
    }

    /// Takes whatever lies under the player: dots, power pellets, or nothing.
    fn eat(&mut self) {
        let (x, y) = self.player;
        let cell = &mut self.grid[y as usize][x as usize];
        match *cell {
            Cell::Dot => {
                self.score += DOT_POINTS;
                self.dots_left -= 1;
            }
            Cell::Power => {
                self.score += POWER_POINTS;
                self.dots_left -= 1;
                for ghost in self.ghosts.iter_mut() {
                    ghost.frightened = FRIGHTENED_TICKS;
                }
            }
            Cell::Wall | Cell::Empty => return,
        }
        *cell = Cell::Empty;
        if self.dots_left == 0 {
            self.won = true;
        }
    }

    fn step_player(&mut self) {
        let (px, py) = self.player;
        let (wx, wy) = self.wanted;
        if self.is_open(px + wx, py + wy) {
            self.heading = self.wanted;
        }
        let (hx, hy) = self.heading;
        if self.is_open(px + hx, py + hy) {
            self.player = (px + hx, py + hy);
            self.eat();
        }
    }

    fn step_ghosts(&mut self) {
        let target = self.player;
        for i in 0..self.ghosts.len() {
            let ghost = self.ghosts[i];
            let (x, y) = ghost.pos();
            let open: Vec<(i32, i32)> = DIRS.into_iter().filter(|&(dx, dy)| self.is_open(x + dx, y + dy)).collect();
            // Ghosts only turn around at a dead end, so they do not jitter back and forth.
            let reverse = (-ghost.dir.0, -ghost.dir.1);
            let forward: Vec<(i32, i32)> = open.iter().copied().filter(|&d| d != reverse).collect();
            let choices = if forward.is_empty() { open } else { forward };
            if choices.is_empty() {
                continue;
            }

            let dir = if ghost.frightened == 0 && next_index(&mut self.rng, 100) < CHASE_PERCENT {
                let distance = |&(dx, dy): &(i32, i32)| (x + dx - target.0).abs() + (y + dy - target.1).abs();
                choices.iter().copied().min_by_key(distance).expect("choices is not empty")
            } else {
                choices[next_index(&mut self.rng, choices.len())]
            };
            self.ghosts[i].x = x + dir.0;
            self.ghosts[i].y = y + dir.1;
            self.ghosts[i].dir = dir;
        }
    }

    /// A ghost on the player's cell either gets eaten (when scared) or costs a life.
    fn check_collisions(&mut self) {
        let mut caught = false;
        for ghost in self.ghosts.iter_mut() {
            if ghost.pos() != self.player {
                continue;
            }
            if ghost.frightened > 0 {
                self.score += GHOST_POINTS;
                ghost.reset();
            } else {
                caught = true;
            }
        }
        if caught {
            self.lives -= 1;
            if self.lives == 0 {
                self.over = true;
                return;
            }
            self.player = PLAYER_START;
            self.heading = (0, 0);
            self.wanted = (0, 0);
            for ghost in self.ghosts.iter_mut() {
                ghost.reset();
            }
        }
    }

    /// Moves the player and the ghosts once the tick has passed.
    pub fn update(&mut self, now: Instant) {
        if self.over || self.won || now.duration_since(self.last_tick) < TICK {
            return;
        }
        self.last_tick = now;
        self.step_player();
        self.check_collisions();
        self.step_ghosts();
        self.check_collisions();
        for ghost in self.ghosts.iter_mut() {
            ghost.frightened = ghost.frightened.saturating_sub(1);
        }
    }

    /// Handles one key press. Returns false when the player leaves the game.
    pub fn handle_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => return false,
            KeyCode::Char('r') if self.over || self.won => *self = Pacman::new(),
            KeyCode::Left => self.wanted = (-1, 0),
            KeyCode::Right => self.wanted = (1, 0),
            KeyCode::Up => self.wanted = (0, -1),
            KeyCode::Down => self.wanted = (0, 1),
            _ => {}
        }
        true
    }

    fn cell_span(&self, x: i32, y: i32) -> Span<'static> {
        if self.player == (x, y) {
            return Span::styled("C ", Style::default().fg(Color::Yellow));
        }
        if let Some(ghost) = self.ghosts.iter().find(|g| g.pos() == (x, y)) {
            let color = if ghost.frightened > 0 { Color::Blue } else { ghost.color };
            return Span::styled("M ", Style::default().fg(color));
        }
        match self.grid[y as usize][x as usize] {
            Cell::Wall => Span::styled("██", Style::default().fg(Color::Blue)),
            Cell::Dot => Span::styled("· ", Style::default().fg(Color::White)),
            Cell::Power => Span::styled("o ", Style::default().fg(Color::White)),
            Cell::Empty => Span::raw("  "),
        }
    }

    /// Paints the maze and the side panel centred in `area`.
    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(Clear, area);
        let rows = self.grid.len();
        let cols = self.grid[0].len();
        let board_w = (cols * 2 + 2) as u16;
        let board_h = (rows + 2) as u16;
        let panel_w = 16u16;
        let total_w = board_w + 2 + panel_w;
        if area.width < total_w || area.height < board_h {
            let notice = format!("terminal too small for pacman: need {total_w}x{board_h}");
            frame.render_widget(Paragraph::new(notice), area);
            return;
        }

        let [_, band, _] = Layout::vertical([Constraint::Min(0), Constraint::Length(board_h), Constraint::Min(0)]).areas(area);
        let [_, centre, _] = Layout::horizontal([Constraint::Min(0), Constraint::Length(total_w), Constraint::Min(0)]).areas(band);
        let [board_area, _, panel] =
            Layout::horizontal([Constraint::Length(board_w), Constraint::Length(2), Constraint::Length(panel_w)]).areas(centre);

        let block = Block::bordered().title(" pacman ");
        let maze_lines: Vec<Line> = (0..rows)
            .map(|y| Line::from((0..cols).map(|x| self.cell_span(x as i32, y as i32)).collect::<Vec<_>>()))
            .collect();
        frame.render_widget(Paragraph::new(maze_lines).block(block), board_area);

        let mut lines = vec![
            Line::from("score"),
            Line::from(self.score.to_string()),
            Line::from(""),
            Line::from("lives"),
            Line::from(self.lives.to_string()),
            Line::from(""),
            Line::from("←→↑↓ move"),
            Line::from("esc/q leave"),
        ];
        if self.won {
            lines.push(Line::from(""));
            lines.push(Line::from("you win!"));
            lines.push(Line::from("r to restart"));
        } else if self.over {
            lines.push(Line::from(""));
            lines.push(Line::from("game over"));
            lines.push(Line::from("r to restart"));
        }
        frame.render_widget(Paragraph::new(lines), panel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maze_rows_have_equal_width() {
        let width = MAZE[0].len();
        assert!(MAZE.iter().all(|row| row.len() == width));
    }

    #[test]
    fn start_cells_are_open() {
        let game = Pacman::new();
        assert!(game.is_open(PLAYER_START.0, PLAYER_START.1));
        assert!(GHOST_STARTS.iter().all(|&(x, y)| game.is_open(x, y)));
    }

    #[test]
    fn new_game_eats_the_dot_under_the_player_start() {
        let game = Pacman::new();
        assert_eq!(game.grid[PLAYER_START.1 as usize][PLAYER_START.0 as usize], Cell::Empty);
        assert_eq!(game.score, DOT_POINTS);
    }

    #[test]
    fn wall_blocks_player_movement() {
        let mut game = Pacman::new();
        // The cell above the start, (9,2), is a wall, so pressing Up does not move the player.
        game.wanted = (0, -1);
        game.step_player();
        assert_eq!(game.player, PLAYER_START);
    }
}

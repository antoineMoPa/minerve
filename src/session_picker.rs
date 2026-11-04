//! State and drawing of the `/resume` picker: the saved sessions, most recent first, each with
//! the start of its first prompt. Enter or a click opens the session in place of the current one.

use std::time::SystemTime;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::session::SessionSummary;

pub struct SessionPicker {
    pub sessions: Vec<SessionSummary>,
    /// Index into `sessions`.
    pub selected: usize,
    /// List area as last drawn. Maps mouse clicks to rows.
    pub list_area: Rect,
    /// Index of the first visible row, as last drawn.
    pub list_offset: usize,
}

impl SessionPicker {
    pub fn new(sessions: Vec<SessionSummary>) -> Self {
        Self {
            sessions,
            selected: 0,
            list_area: Rect::default(),
            list_offset: 0,
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.sessions.is_empty() {
            self.selected = 0;
            return;
        }
        let last = self.sessions.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    /// Index of the row under a click at (column, row), if the click is on a session.
    pub fn row_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.list_area;
        // Inside the borders only.
        let inside = column > area.x
            && column < area.right().saturating_sub(1)
            && row > area.y
            && row < area.bottom().saturating_sub(1);
        if !inside {
            return None;
        }
        let index = self.list_offset + (row - area.y - 1) as usize;
        (index < self.sessions.len()).then_some(index)
    }

    /// Draws the picker over the whole screen: list of sessions, then a hint line.
    pub fn draw(&mut self, frame: &mut Frame, area: Rect, now: SystemTime) {
        frame.render_widget(Clear, area);
        let [title_area, list_area, hint_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(area);

        frame.render_widget(
            Paragraph::new(format!(" Resume a session ({} saved)", self.sessions.len()))
                .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            title_area,
        );

        let items: Vec<ListItem> = if self.sessions.is_empty() {
            vec![ListItem::new(Line::styled(
                "no saved session yet: a session is saved after each answer.",
                Style::default().fg(Color::Gray),
            ))]
        } else {
            self.sessions
                .iter()
                .map(|session| session_item(session, now))
                .collect()
        };
        let mut state = ListState::default().with_selected(if self.sessions.is_empty() {
            None
        } else {
            Some(self.selected)
        });
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        frame.render_stateful_widget(list, list_area, &mut state);
        self.list_area = list_area;
        self.list_offset = state.offset();

        frame.render_widget(
            Paragraph::new("Enter or click resume | Up/Down move | Esc close")
                .style(Style::default().fg(Color::DarkGray)),
            hint_area,
        );
    }
}

/// One session row: age, id prefix, then the first prompt.
fn session_item(session: &SessionSummary, now: SystemTime) -> ListItem<'static> {
    let age = format_age(now.duration_since(session.modified).unwrap_or_default().as_secs());
    let prompt = if session.first_prompt.is_empty() {
        String::from("(no prompt)")
    } else {
        session.first_prompt.clone()
    };
    ListItem::new(Line::from(vec![
        Span::styled(format!("{:>8} ", age), Style::default().fg(Color::Gray)),
        Span::styled(
            format!("{} ", &session.id[..8]),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(prompt),
    ]))
}

/// Seconds as a short age: `45s`, `12m`, `3h`, `5d`.
fn format_age(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{}s ago", seconds),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str) -> SessionSummary {
        SessionSummary {
            id: id.to_string(),
            modified: SystemTime::UNIX_EPOCH,
            first_prompt: String::from("hello"),
        }
    }

    #[test]
    fn formats_ages() {
        assert_eq!(format_age(45), "45s ago");
        assert_eq!(format_age(720), "12m ago");
        assert_eq!(format_age(3 * 3600), "3h ago");
        assert_eq!(format_age(5 * 86_400), "5d ago");
    }

    #[test]
    fn selection_stays_inside_the_list() {
        let mut picker = SessionPicker::new(vec![summary("a"), summary("b")]);
        picker.move_selection(5);
        assert_eq!(picker.selected, 1);
        picker.move_selection(-9);
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn click_maps_to_the_row_under_it() {
        let mut picker = SessionPicker::new(vec![summary("a"), summary("b")]);
        picker.list_area = Rect::new(0, 1, 40, 5);
        assert_eq!(picker.row_at(5, 2), Some(0));
        assert_eq!(picker.row_at(5, 3), Some(1));
        assert_eq!(picker.row_at(5, 4), None, "past the last session");
        assert_eq!(picker.row_at(0, 2), None, "on the left border");
        assert_eq!(picker.row_at(5, 1), None, "on the top border");
    }
}

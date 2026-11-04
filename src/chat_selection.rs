//! Mouse selection of the chat transcript. Mouse capture hides the terminal's own selection, so a
//! drag over the chat highlights the cells here, and releasing copies them with an OSC 52 sequence.
//! Selections are in terminal cells: a scroll or a resize clears them, because the text moves.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChatSelection {
    /// Cell (column, row) where the drag started.
    pub anchor: (u16, u16),
    /// Cell (column, row) under the pointer now.
    pub cursor: (u16, u16),
    /// Chat row shown at the top of the chat area when the drag started. Once the chat shows
    /// another row there (scroll, new message, resize), the cells no longer hold the selected text.
    pub chat_top: usize,
}

impl ChatSelection {
    pub fn new(cell: (u16, u16), chat_top: usize) -> Self {
        Self {
            anchor: cell,
            cursor: cell,
            chat_top,
        }
    }

    /// True when the drag never left the cell it started on.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    /// First and last cell in reading order (top to bottom, left to right).
    fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        let (a, b) = (self.anchor, self.cursor);
        if (a.1, a.0) <= (b.1, b.0) {
            (a, b)
        } else {
            (b, a)
        }
    }

    /// True when the cell (column, row) lies between the first and last cell, like a text selection.
    pub fn contains(&self, column: u16, row: u16) -> bool {
        let ((start_col, start_row), (end_col, end_row)) = self.ordered();
        if row < start_row || row > end_row {
            return false;
        }
        if start_row == end_row {
            return column >= start_col && column <= end_col;
        }
        if row == start_row {
            column >= start_col
        } else if row == end_row {
            column <= end_col
        } else {
            true
        }
    }
}

/// Keeps a cell inside `area`, so a drag past the chat edge selects up to the edge.
pub fn clamp_to(area: Rect, cell: (u16, u16)) -> (u16, u16) {
    let column = cell.0.clamp(area.x, area.right().saturating_sub(1));
    let row = cell.1.clamp(area.y, area.bottom().saturating_sub(1));
    (column, row)
}

/// Reverses the colours of the selected cells inside `area`, so the selection is visible.
pub fn highlight(buffer: &mut Buffer, area: Rect, selection: &ChatSelection) {
    for row in area.y..area.bottom() {
        for column in area.x..area.right() {
            if selection.contains(column, row) {
                if let Some(cell) = buffer.cell_mut((column, row)) {
                    cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
                }
            }
        }
    }
}

/// Text of the selected cells in `buffer`, one line per row, trailing spaces removed.
/// `buffer` is the last frame drawn, so it holds exactly what the user sees.
pub fn selected_text(buffer: &Buffer, area: Rect, selection: &ChatSelection) -> String {
    let mut lines = Vec::new();
    for row in area.y..area.bottom() {
        let mut line = String::new();
        for column in area.x..area.right() {
            if selection.contains(column, row) {
                if let Some(cell) = buffer.cell((column, row)) {
                    // Cells after a wide character hold no symbol of their own.
                    line.push_str(cell.symbol());
                }
            }
        }
        if selection.contains(area.x, row) || line.len() > 0 {
            lines.push(line.trim_end().to_string());
        }
    }
    lines.join("\n")
}

/// Escape sequence that puts `text` on the system clipboard (OSC 52). Supported by most modern terminals.
pub fn osc52_copy(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 63] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode("héllo".as_bytes()), "aMOpbGxv");
    }

    #[test]
    fn selection_follows_reading_order_even_when_dragged_backwards() {
        let forward = ChatSelection { anchor: (2, 1), cursor: (4, 3), chat_top: 0 };
        let backward = ChatSelection { anchor: (4, 3), cursor: (2, 1), chat_top: 0 };
        for selection in [forward, backward] {
            assert!(selection.contains(2, 1));
            assert!(!selection.contains(1, 1), "before the start column on the first row");
            assert!(selection.contains(0, 2), "middle rows are whole");
            assert!(selection.contains(4, 3));
            assert!(!selection.contains(5, 3), "after the end column on the last row");
        }
    }

    #[test]
    fn single_row_selection_is_bounded_on_both_sides() {
        let selection = ChatSelection { anchor: (3, 0), cursor: (6, 0), chat_top: 0 };
        assert!(selection.contains(3, 0) && selection.contains(6, 0));
        assert!(!selection.contains(2, 0) && !selection.contains(7, 0));
    }

    #[test]
    fn drag_past_the_chat_edge_stops_at_the_edge() {
        let area = Rect::new(10, 5, 20, 4);
        assert_eq!(clamp_to(area, (0, 0)), (10, 5));
        assert_eq!(clamp_to(area, (99, 99)), (29, 8));
    }

    #[test]
    fn copies_the_selected_text_from_the_buffer() {
        let area = Rect::new(0, 0, 10, 2);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "hello", Style::default());
        buffer.set_string(0, 1, "world", Style::default());
        let selection = ChatSelection { anchor: (1, 0), cursor: (2, 1), chat_top: 0 };
        assert_eq!(selected_text(&buffer, area, &selection), "ello\nwor");
    }

    #[test]
    fn osc52_wraps_the_base64_text() {
        assert_eq!(osc52_copy("hi"), "\x1b]52;c;aGk=\x07");
    }
}

//! Wraps the message input into terminal rows and finds where the cursor lands.
//!
//! Wrapping and cursor placement share one walk over the text so they can never disagree.
//! Columns are terminal cells (display width), not chars: an emoji or CJK char takes 2 cells,
//! a combining accent 0. Control chars other than '\n' (e.g. a pasted tab) take 0 cells,
//! because ratatui does not draw them.

use unicode_width::UnicodeWidthChar;

pub struct InputLayout {
    /// The input split into the rows the input box shows.
    pub rows: Vec<String>,
    /// (cell column, row) of each cursor position: index `i` is the cursor before char `i`,
    /// the last entry is the cursor after the last char.
    cursor_cells: Vec<(u16, u16)>,
}

impl InputLayout {
    /// (cell column, row) of the cursor before char index `cursor`.
    pub fn cursor_cell(&self, cursor: usize) -> (u16, u16) {
        self.cursor_cells[cursor]
    }

    /// Cursor (char index) for a click at (cell column, row): the rightmost cursor position on
    /// that row at or left of the click. A click below the last row lands on the last row.
    pub fn cursor_at_cell(&self, col: u16, row: u16) -> usize {
        let last_row = self.cursor_cells.last().expect("one cursor position per char, plus the end").1;
        let row = row.min(last_row);
        self.cursor_cells
            .iter()
            .rposition(|&(c, r)| r == row && c <= col)
            .expect("every row starts with a cursor position at column 0")
    }
}

/// Lays out `input` in a box `width` cells wide.
pub fn layout_input(input: &str, width: u16) -> InputLayout {
    let width = width.max(1) as usize;
    let mut rows = vec![String::new()];
    let mut col = 0usize;
    let mut cursor_cells = Vec::with_capacity(input.len() + 1);
    let cell = |col: usize, row: usize| (col as u16, row.min(u16::MAX as usize) as u16);

    for c in input.chars() {
        if c == '\n' {
            cursor_cells.push(cell(col, rows.len() - 1));
            rows.push(String::new());
            col = 0;
            continue;
        }
        let cell_width = c.width().unwrap_or(0);
        // A char that does not fit in what is left of the row starts the next one.
        if col > 0 && col + cell_width > width {
            rows.push(String::new());
            col = 0;
        }
        cursor_cells.push(cell(col, rows.len() - 1));
        rows.last_mut().expect("rows starts non-empty").push(c);
        col += cell_width;
        // A full row: the next char (and a cursor after this one) goes on a new row,
        // so the cursor never sits on the right border.
        if col >= width {
            rows.push(String::new());
            col = 0;
        }
    }
    cursor_cells.push(cell(col, rows.len() - 1));

    InputLayout { rows, cursor_cells }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::widgets::Paragraph;
    use ratatui::Terminal;

    /// Renders the rows like the input box does and returns the symbol left of the cursor.
    fn symbol_before_cursor(input: &str, width: u16) -> String {
        let layout = layout_input(input, width);
        let (cursor_col, cursor_row) = layout.cursor_cell(input.chars().count());
        let height = layout.rows.len() as u16;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new(layout.rows.join("\n")), frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let (end_x, y) = match (cursor_col, cursor_row) {
            (0, row) => (width, row - 1),
            (col, row) => (col, row),
        };
        // Read the row's symbols left to right up to the cursor, skipping the cells a wide char covers.
        let mut x = 0;
        let mut last = String::new();
        while x < end_x {
            last = buffer[(x, y)].symbol().to_string();
            x += last.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>().max(1) as u16;
        }
        assert_eq!(x, end_x, "cursor lands inside a wide char");
        last
    }

    #[test]
    fn ascii_wraps_at_width() {
        let layout = layout_input("abcdefgh", 5);
        assert_eq!(layout.rows, vec!["abcde", "fgh"]);
        assert_eq!(layout.cursor_cell(8), (3, 1));
    }

    #[test]
    fn exactly_full_row_puts_cursor_on_next_row() {
        let layout = layout_input("abcde", 5);
        assert_eq!(layout.rows, vec!["abcde", ""]);
        assert_eq!(layout.cursor_cell(5), (0, 1));
    }

    #[test]
    fn wide_chars_count_two_cells() {
        // Each emoji is 2 cells: 2 fit in a 5-cell row, the third wraps.
        let layout = layout_input("😀😀😀", 5);
        assert_eq!(layout.rows, vec!["😀😀", "😀"]);
        assert_eq!(layout.cursor_cell(3), (2, 1));
    }

    #[test]
    fn cursor_in_middle_and_after_newline() {
        let layout = layout_input("ab\ncd", 10);
        assert_eq!(layout.cursor_cell(3), (0, 1));
        let layout = layout_input("ab\ncd", 10);
        assert_eq!(layout.cursor_cell(2), (2, 0));
    }

    #[test]
    fn cursor_follows_typing_in_rendered_long_line() {
        let line = "un long message avec des emoji 😀 et du 日本語 qui dépasse la largeur 🎉 fin";
        for width in [7, 10, 13, 20] {
            for end in 1..=line.chars().count() {
                let typed: String = line.chars().take(end).collect();
                let last = typed.chars().last().unwrap().to_string();
                if last == " " {
                    continue;
                }
                assert_eq!(symbol_before_cursor(&typed, width), last, "width {width}, typed {typed:?}");
            }
        }
    }

    #[test]
    fn click_maps_back_to_cursor() {
        // Rows: "abcde" / "fg" then a newline / "😀x".
        let layout = layout_input("abcdefg\n😀x", 5);
        assert_eq!(layout.rows, vec!["abcde", "fg", "😀x"]);
        assert_eq!(layout.cursor_at_cell(2, 0), 2);
        // Past the end of a line ending in a newline: before the newline.
        assert_eq!(layout.cursor_at_cell(4, 1), 7);
        // On the second cell of the emoji: before the emoji.
        assert_eq!(layout.cursor_at_cell(1, 2), 8);
        assert_eq!(layout.cursor_at_cell(2, 2), 9);
        // Past the end of the text, and below the last row: after the last char.
        assert_eq!(layout.cursor_at_cell(4, 2), 10);
        assert_eq!(layout.cursor_at_cell(0, 9), 8);
    }

    #[test]
    fn every_cursor_position_round_trips_through_a_click() {
        let text = "un long message 😀 et du 日本語\nqui dépasse 🎉 fin";
        for width in [5, 7, 13] {
            let layout = layout_input(text, width);
            for cursor in 0..=text.chars().count() {
                let (col, row) = layout.cursor_cell(cursor);
                assert_eq!(layout.cursor_at_cell(col, row), cursor, "width {width}, cursor {cursor}");
            }
        }
    }
}

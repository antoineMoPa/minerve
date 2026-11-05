//! Editing the input line: cursor moves, kills and Emacs-style yanks.

use super::*;

impl App {
    pub(super) fn set_input(&mut self, text: String) {
        self.cursor = text.chars().count();
        self.input = text;
        // History and completion text is typed, not pasted.
        self.input_pasted = false;
    }

    pub(super) fn input_len(&self) -> usize {
        self.input.chars().count()
    }

    /// Byte index of the char at `char_idx`, or the end of the input.
    pub(super) fn byte_index(&self, char_idx: usize) -> usize {
        self.input
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.input.len())
    }

    pub(super) fn insert_char(&mut self, c: char) {
        let idx = self.byte_index(self.cursor);
        self.input.insert(idx, c);
        self.cursor += 1;
    }

    pub(super) fn delete_before_cursor(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let idx = self.byte_index(self.cursor - 1);
        self.input.remove(idx);
        self.cursor -= 1;
    }

    pub(super) fn delete_at_cursor(&mut self) {
        if self.cursor >= self.input_len() {
            return;
        }
        let idx = self.byte_index(self.cursor);
        self.input.remove(idx);
    }

    /// Ctrl+K: removes the whole input and keeps it in the kill ring.
    pub(super) fn kill_input(&mut self) {
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
    pub(super) fn yank(&mut self) {
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
    pub(super) fn yank_pop(&mut self, previous: Option<YankState>) {
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
    pub(super) fn insert_str_at_cursor(&mut self, text: &str) {
        let idx = self.byte_index(self.cursor);
        self.input.insert_str(idx, text);
        self.cursor += text.chars().count();
    }
}

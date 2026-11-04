//! State and drawing of the `/model` picker: a filter box over the OpenRouter model list,
//! with the recently used models pinned at the top.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::recent_models::RecentModels;

pub struct ModelPicker {
    /// Text typed in the filter box.
    pub filter: String,
    /// Every model id on OpenRouter. None while the list is still being fetched.
    pub all_models: Option<Result<Vec<String>, String>>,
    /// Index into `visible_rows`.
    pub selected: usize,
    /// Model id that is currently active, shown with a marker.
    pub current_model: String,
}

/// One line of the picker: a model id, and whether it comes from the recent list.
#[derive(Clone, PartialEq, Debug)]
pub struct PickerRow {
    pub model_id: String,
    pub recent: bool,
}

impl ModelPicker {
    pub fn new(current_model: String) -> Self {
        Self {
            filter: String::new(),
            all_models: None,
            selected: 0,
            current_model,
        }
    }

    /// Recent models that match the filter, then every other matching model in OpenRouter's order.
    /// Empty while the list is loading or failed.
    pub fn visible_rows(&self, recent: &RecentModels) -> Vec<PickerRow> {
        let Some(Ok(all_models)) = &self.all_models else {
            return Vec::new();
        };
        let needle = self.filter.to_lowercase();
        let matches = |id: &str| id.to_lowercase().contains(&needle);

        let recent_rows = recent
            .ids()
            .iter()
            .filter(|id| all_models.contains(id) && matches(id))
            .map(|id| PickerRow {
                model_id: id.clone(),
                recent: true,
            });
        let other_rows = all_models
            .iter()
            .filter(|id| !recent.ids().contains(id) && matches(id))
            .map(|id| PickerRow {
                model_id: id.clone(),
                recent: false,
            });
        recent_rows.chain(other_rows).collect()
    }

    /// Keeps the selection inside the rows after the filter changed.
    pub fn clamp_selection(&mut self, row_count: usize) {
        self.selected = self.selected.min(row_count.saturating_sub(1));
    }

    pub fn move_selection(&mut self, delta: isize, row_count: usize) {
        if row_count == 0 {
            self.selected = 0;
            return;
        }
        let last = row_count as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    /// Draws the picker over the whole screen: filter box on top, then the list.
    pub fn draw(&self, frame: &mut Frame, area: Rect, rows: &[PickerRow]) {
        frame.render_widget(Clear, area);
        let [filter_area, list_area, hint_area] = ratatui::layout::Layout::vertical([
            ratatui::layout::Constraint::Length(3),
            ratatui::layout::Constraint::Min(3),
            ratatui::layout::Constraint::Length(1),
        ])
        .areas(area);

        frame.render_widget(
            Paragraph::new(self.filter.as_str()).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Model: type to filter ")
                    .border_style(Style::default().fg(Color::Cyan)),
            ),
            filter_area,
        );
        // The filter box cursor sits after the typed text.
        let cursor_x = filter_area.x + 1 + self.filter.chars().count() as u16;
        frame.set_cursor_position((cursor_x.min(filter_area.right().saturating_sub(2)), filter_area.y + 1));

        let body = self.body_message(rows);
        let items: Vec<ListItem> = match body {
            Some(message) => vec![ListItem::new(Line::styled(message, Style::default().fg(Color::Gray)))],
            None => rows.iter().map(|row| self.row_item(row)).collect(),
        };
        let mut state = ListState::default().with_selected(if rows.is_empty() {
            None
        } else {
            Some(self.selected)
        });
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Cyan)))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        frame.render_stateful_widget(list, list_area, &mut state);

        frame.render_widget(
            Paragraph::new("Enter select | Up/Down move | Esc close")
                .style(Style::default().fg(Color::DarkGray)),
            hint_area,
        );
    }

    /// Text shown instead of the rows while loading, on failure, or when nothing matches.
    fn body_message(&self, rows: &[PickerRow]) -> Option<String> {
        match &self.all_models {
            None => Some(String::from("loading models from OpenRouter...")),
            Some(Err(err)) => Some(format!("cannot load models: {}", err)),
            Some(Ok(_)) if rows.is_empty() => Some(String::from("no model matches the filter")),
            Some(Ok(_)) => None,
        }
    }

    fn row_item(&self, row: &PickerRow) -> ListItem<'static> {
        let marker = if row.model_id == self.current_model { "* " } else { "  " };
        let mut spans = vec![Span::raw(marker), Span::raw(row.model_id.clone())];
        if row.recent {
            spans.push(Span::styled("  recent", Style::default().fg(Color::DarkGray)));
        }
        ListItem::new(Line::from(spans))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker_with(models: &[&str], filter: &str) -> ModelPicker {
        let mut picker = ModelPicker::new(String::from("current"));
        picker.all_models = Some(Ok(models.iter().map(|m| m.to_string()).collect()));
        picker.filter = filter.to_string();
        picker
    }

    fn recent_with(ids: &[&str]) -> RecentModels {
        RecentModels::from_ids(ids.iter().map(|id| id.to_string()).collect())
    }

    fn ids(rows: &[PickerRow]) -> Vec<&str> {
        rows.iter().map(|row| row.model_id.as_str()).collect()
    }

    #[test]
    fn recent_models_come_first_then_the_rest() {
        let picker = picker_with(&["x/one", "y/two", "z/three"], "");
        let rows = picker.visible_rows(&recent_with(&["z/three"]));
        assert_eq!(ids(&rows), ["z/three", "x/one", "y/two"]);
        assert!(rows[0].recent);
        assert!(!rows[1].recent);
    }

    #[test]
    fn filter_is_case_insensitive_substring_on_the_id() {
        let picker = picker_with(&["anthropic/Claude-3", "openai/gpt-4", "meta/llama"], "CLAUDE");
        let rows = picker.visible_rows(&recent_with(&[]));
        assert_eq!(ids(&rows), ["anthropic/Claude-3"]);
    }

    #[test]
    fn filter_also_applies_to_recent_models() {
        let picker = picker_with(&["x/one", "y/two"], "two");
        let rows = picker.visible_rows(&recent_with(&["x/one", "y/two"]));
        assert_eq!(ids(&rows), ["y/two"]);
    }

    #[test]
    fn recent_model_missing_from_openrouter_is_hidden() {
        let picker = picker_with(&["x/one"], "");
        let rows = picker.visible_rows(&recent_with(&["gone/model"]));
        assert_eq!(ids(&rows), ["x/one"]);
    }

    #[test]
    fn no_rows_while_loading() {
        let picker = ModelPicker::new(String::from("current"));
        assert!(picker.visible_rows(&recent_with(&["x/one"])).is_empty());
    }

    #[test]
    fn selection_stays_inside_the_rows() {
        let mut picker = ModelPicker::new(String::from("current"));
        picker.move_selection(1, 3);
        picker.move_selection(1, 3);
        picker.move_selection(1, 3);
        assert_eq!(picker.selected, 2);
        picker.move_selection(-5, 3);
        assert_eq!(picker.selected, 0);
        picker.clamp_selection(0);
        assert_eq!(picker.selected, 0);
    }
}

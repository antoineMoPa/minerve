//! State and drawing of the `/model` picker: a filter box over the model lists of every provider
//! that has a key, with the recently used models pinned at the top.
//! Ids carry their provider prefix (`openrouter:`, `openai:`, `anthropic:`), so typing a provider
//! name in the filter finds its models.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::provider::{route_model_id, Provider};
use crate::recent_models::RecentModels;

pub struct ModelPicker {
    /// Text typed in the filter box.
    pub filter: String,
    /// Only rows from this provider are listed; None lists every provider. Tab cycles it.
    pub provider_filter: Option<Provider>,
    /// Providers that have a key, in the order their lists are shown. Only these are fetched.
    pub keyed_providers: Vec<Provider>,
    /// Fetch result of each provider, in the order they arrived.
    pub lists: Vec<(Provider, Result<Vec<String>, String>)>,
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
    pub fn new(current_model: String, keyed_providers: Vec<Provider>) -> Self {
        Self {
            filter: String::new(),
            provider_filter: None,
            keyed_providers,
            lists: Vec::new(),
            selected: 0,
            current_model,
        }
    }

    /// All providers, then each keyed provider alone, then back to all.
    pub fn cycle_provider_filter(&mut self) {
        let next_index = match self.provider_filter {
            None => 0,
            Some(current) => {
                self.keyed_providers
                    .iter()
                    .position(|provider| *provider == current)
                    .expect("provider filter is always a keyed provider")
                    + 1
            }
        };
        self.provider_filter = self.keyed_providers.get(next_index).copied();
        self.selected = 0;
    }

    fn provider_filter_label(&self) -> &'static str {
        match self.provider_filter {
            None => "all providers",
            Some(provider) => provider.label(),
        }
    }

    /// Ids of every list fetched so far, in `keyed_providers` order.
    fn loaded_ids(&self) -> Vec<&String> {
        self.keyed_providers
            .iter()
            .filter_map(|provider| self.lists.iter().find(|(listed, _)| listed == provider))
            .filter_map(|(_, result)| result.as_ref().ok())
            .flatten()
            .collect()
    }

    fn load_errors(&self) -> Vec<String> {
        self.lists
            .iter()
            .filter_map(|(provider, result)| {
                result
                    .as_ref()
                    .err()
                    .map(|err| format!("cannot load {} models: {}", provider.label(), err))
            })
            .collect()
    }

    /// Recent models that match the filter, then every other matching model in provider order.
    pub fn visible_rows(&self, recent: &RecentModels) -> Vec<PickerRow> {
        let all_models = self.loaded_ids();
        let needle = self.filter.to_lowercase();
        let matches = |id: &str| {
            self.provider_filter
                .is_none_or(|provider| route_model_id(id).0 == provider)
                && id.to_lowercase().contains(&needle)
        };

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
                model_id: (*id).clone(),
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
                    .title(format!(" Model: type to filter | Tab: {} ", self.provider_filter_label()))
                    .border_style(Style::default().fg(Color::Cyan)),
            ),
            filter_area,
        );
        // The filter box cursor sits after the typed text.
        let cursor_x = filter_area.x + 1 + self.filter.chars().count() as u16;
        frame.set_cursor_position((cursor_x.min(filter_area.right().saturating_sub(2)), filter_area.y + 1));

        let items: Vec<ListItem> = match self.body_message(rows) {
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

        // A provider whose list failed while others loaded is reported in the hint line.
        let (hint, hint_color) = match self.load_errors().first() {
            Some(err) if !rows.is_empty() => (err.clone(), Color::Red),
            _ => (
                String::from("Enter select | Up/Down move | Tab provider | Esc close"),
                Color::DarkGray,
            ),
        };
        frame.render_widget(
            Paragraph::new(hint).style(Style::default().fg(hint_color)),
            hint_area,
        );
    }

    /// Text shown instead of the rows when there are none.
    fn body_message(&self, rows: &[PickerRow]) -> Option<String> {
        if !rows.is_empty() {
            return None;
        }
        if self.keyed_providers.is_empty() {
            return Some(String::from("no API key set: run /apikeys to add one."));
        }
        let errors = self.load_errors();
        if self.lists.len() < self.keyed_providers.len() {
            return Some(String::from("loading models..."));
        }
        if !errors.is_empty() && self.loaded_ids().is_empty() {
            return Some(errors.join(" | "));
        }
        Some(String::from("no model matches the filter"))
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

    const ALL: [Provider; 3] = [Provider::OpenRouter, Provider::Anthropic, Provider::OpenAi];

    fn picker_with(models: &[&str], filter: &str) -> ModelPicker {
        let mut picker = ModelPicker::new(String::from("current"), ALL.to_vec());
        for provider in ALL {
            let ids = models
                .iter()
                .filter(|id| route_model_id(id).0 == provider)
                .map(|id| id.to_string())
                .collect();
            picker.lists.push((provider, Ok(ids)));
        }
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
        let picker = picker_with(&["openrouter:x/one", "openrouter:y/two", "openrouter:z/three"], "");
        let rows = picker.visible_rows(&recent_with(&["openrouter:z/three"]));
        assert_eq!(ids(&rows), ["openrouter:z/three", "openrouter:x/one", "openrouter:y/two"]);
        assert!(rows[0].recent);
        assert!(!rows[1].recent);
    }

    #[test]
    fn lists_follow_provider_order_whatever_order_they_arrive_in() {
        let mut picker = ModelPicker::new(String::from("current"), ALL.to_vec());
        picker.lists.push((Provider::Anthropic, Ok(vec![String::from("anthropic:claude")])));
        picker.lists.push((Provider::OpenRouter, Ok(vec![String::from("openrouter:a/b")])));
        let rows = picker.visible_rows(&recent_with(&[]));
        assert_eq!(ids(&rows), ["openrouter:a/b", "anthropic:claude"]);
    }

    #[test]
    fn typing_a_provider_name_finds_its_models() {
        let models = ["openai:gpt-4o", "openrouter:meta/llama", "anthropic:claude-haiku-4-5"];
        let picker = picker_with(&models, "anthropic");
        assert_eq!(ids(&picker.visible_rows(&recent_with(&[]))), ["anthropic:claude-haiku-4-5"]);
        let picker = picker_with(&models, "OPENROUTER");
        assert_eq!(ids(&picker.visible_rows(&recent_with(&[]))), ["openrouter:meta/llama"]);
    }

    #[test]
    fn tab_cycles_through_keyed_providers_only() {
        let mut picker = ModelPicker::new(String::from("current"), vec![Provider::Anthropic]);
        assert_eq!(picker.provider_filter, None);
        picker.cycle_provider_filter();
        assert_eq!(picker.provider_filter, Some(Provider::Anthropic));
        picker.cycle_provider_filter();
        assert_eq!(picker.provider_filter, None);
    }

    #[test]
    fn provider_filter_combines_with_the_text_filter() {
        let mut picker = picker_with(
            &["openai:gpt-4o", "openai:gpt-3", "openrouter:openai/gpt-4o", "anthropic:claude"],
            "gpt",
        );
        picker.provider_filter = Some(Provider::OpenAi);
        assert_eq!(ids(&picker.visible_rows(&recent_with(&[]))), ["openai:gpt-4o", "openai:gpt-3"]);
        picker.provider_filter = Some(Provider::Anthropic);
        assert!(picker.visible_rows(&recent_with(&[])).is_empty());
    }

    #[test]
    fn recent_model_missing_from_the_lists_is_hidden() {
        let picker = picker_with(&["openrouter:x/one"], "");
        let rows = picker.visible_rows(&recent_with(&["openrouter:gone/model", "x/one"]));
        assert_eq!(ids(&rows), ["openrouter:x/one"]);
    }

    #[test]
    fn failed_list_does_not_hide_the_others() {
        let mut picker = ModelPicker::new(String::from("current"), ALL.to_vec());
        picker.lists.push((Provider::OpenRouter, Err(String::from("401"))));
        picker.lists.push((Provider::Anthropic, Ok(vec![String::from("anthropic:claude")])));
        assert_eq!(ids(&picker.visible_rows(&recent_with(&[]))), ["anthropic:claude"]);
        assert_eq!(picker.load_errors(), ["cannot load OpenRouter models: 401"]);
    }

    #[test]
    fn selection_stays_inside_the_rows() {
        let mut picker = ModelPicker::new(String::from("current"), ALL.to_vec());
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

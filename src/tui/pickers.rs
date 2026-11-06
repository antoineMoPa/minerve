//! The model and session pickers and the API key setup modal: opening them and their keys.

use super::*;

impl App {
    /// Opens the `/model` picker and fetches, in the background, the model list of every provider
    /// that has a key. Providers without a key are not listed.
    pub(super) fn open_model_picker(&mut self) {
        let current_model = self.minerve.active_model().id;
        let keys = self.minerve.api_keys_snapshot();
        let keyed_providers: Vec<Provider> = PROVIDERS_BY_DEFAULT_PRIORITY
            .into_iter()
            .filter(|provider| keys.get(*provider).is_some())
            .collect();
        for provider in keyed_providers.iter().copied() {
            let api_key = keys.get(provider).expect("filtered on key presence").to_string();
            let minerve = self.minerve.clone();
            let ui = self.ui_tx.clone();
            crate::get_global_runtime().spawn(async move {
                let result = crate::model_lists::fetch_model_ids(
                    &minerve.client,
                    provider,
                    &api_key,
                    &minerve.base_url,
                )
                .await;
                let _ = ui.send(UiEvent::ModelList(provider, result));
            });
        }
        self.model_picker = Some(ModelPicker::new(current_model, keyed_providers));
    }

    /// Opens the `/resume` picker over the saved sessions, most recent first.
    pub(super) fn open_session_picker(&mut self) {
        if self.minerve.request_in_flight.load(Ordering::SeqCst) {
            self.status = String::from("wait for the running answer before resuming another session");
            return;
        }
        self.session_picker = Some(SessionPicker::new(crate::session::list_sessions()));
    }

    /// Keys while the `/resume` picker is open. Enter continues the selected session.
    pub(super) fn handle_session_picker_key(&mut self, key: KeyEvent) {
        let Some(picker) = self.session_picker.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.session_picker = None,
            KeyCode::Up => picker.move_selection(-1),
            KeyCode::Down => picker.move_selection(1),
            KeyCode::Enter => {
                let Some(session) = picker.sessions.get(picker.selected) else {
                    return;
                };
                let id = session.id.clone();
                self.resume_session(&id);
            }
            _ => {}
        }
    }

    /// Loads the saved session `id` and shows its chat. A file that cannot be read keeps the
    /// current session and reports why on the status line.
    /// `/clear`: a fresh conversation in a new session. Refused while a turn runs, like `/resume`.
    pub(super) fn clear_conversation(&mut self) {
        if self.minerve.request_in_flight.load(Ordering::SeqCst) {
            self.status = String::from("wait for the running answer before clearing the conversation");
            return;
        }
        self.minerve.start_new_session();
        self.transcript = self.minerve.ui_messages();
        self.scroll_up = 0;
        self.notices.clear();
        self.pending_command_output.clear();
        self.queued_prompts.clear();
        self.chat_selection = None;
        self.status = String::from("started a new conversation");
    }

    pub(super) fn resume_session(&mut self, id: &str) {
        match crate::session::load_for_resume(id) {
            Ok(file) => {
                self.minerve.replace_session(file);
                self.transcript = self.minerve.ui_messages();
                self.scroll_up = 0;
                self.notices.clear();
                self.pending_command_output.clear();
                self.queued_prompts.clear();
                self.status = format!("resumed session {}", id);
                self.session_picker = None;
            }
            Err(err) => self.status = err,
        }
    }

    /// Keys while the setup modal is open. Enter on the last field saves the keys and applies them.
    pub(super) fn handle_setup_key(&mut self, key: KeyEvent) {
        let Some(modal) = self.setup_modal.as_mut() else {
            return;
        };
        match modal.handle_key(key) {
            SetupStep::Stay => {}
            SetupStep::Cancelled => self.setup_modal = None,
            SetupStep::Completed(settings) => match setup::save(&settings) {
                Ok(path) => {
                    self.minerve.set_api_keys(
                        settings.openrouter_key,
                        settings.openai_key,
                        settings.anthropic_key,
                    );
                    // Keep the active model if its provider has a key, else move to the most
                    // recent model with a key, else a keyed provider's default.
                    let keys = self.minerve.api_keys_snapshot();
                    let active = self.minerve.active_model();
                    let candidates = std::iter::once(active.id.clone())
                        .chain(self.recent_models.ids().iter().cloned());
                    let picked = crate::openrouter::pick_model_with_key(candidates, &keys);
                    let model = (picked != active.id).then_some(picked);
                    if let Some(model) = model {
                        self.minerve.set_model(model);
                        self.minerve.fetch_context_window_in_background();
                    }
                    self.status = format!("saved to {}", path.display());
                    self.setup_modal = None;
                }
                Err(err) => modal.set_error(format!("could not save ~/.minerve/keys.json: {}", err)),
            },
        }
    }

    /// Keys while the model picker is open. Enter switches model, Esc closes.
    pub(super) fn handle_model_picker_key(&mut self, key: KeyEvent) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        let rows = picker.visible_rows(&self.recent_models);
        match key.code {
            KeyCode::Esc => self.model_picker = None,
            KeyCode::Up => picker.move_selection(-1, rows.len()),
            KeyCode::Down => picker.move_selection(1, rows.len()),
            KeyCode::Tab => {
                picker.cycle_provider_filter();
                picker.selected = 0;
            }
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
}

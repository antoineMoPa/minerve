//! The `--serve` commands that open the TUI's pickers and modals: `/goal`, `/model`, `/resume`, `/apikeys`.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::control_socket::{ControlSocket, MinerveEvent, ModelListOut, ParentCommand, SessionOut};
use crate::minerve::Minerve;
use crate::provider::{Provider, PROVIDERS_BY_DEFAULT_PRIORITY};
use crate::recent_models::RecentModels;
use crate::setup;
use crate::ui::{UiEvent, UiSender};

/// A `list_models` waiting for the providers it asked.
struct PendingModels {
    expected: usize,
    lists: Vec<ModelListOut>,
}

pub struct Extras {
    recent: RecentModels,
    pending: Option<PendingModels>,
}

impl Extras {
    pub fn new() -> Self {
        Self { recent: RecentModels::load(), pending: None }
    }

    /// True when `command` was one of ours.
    pub fn handle(&mut self, command: ParentCommand, minerve: &Arc<Minerve>, control: &ControlSocket, ui: &UiSender) -> bool {
        match command {
            ParentCommand::Goal { goal } => {
                if busy(minerve, control) {
                    return true;
                }
                minerve.clone().pursue_goal(goal, ui.clone());
            }
            ParentCommand::ListModels {} => self.list_models(minerve, control, ui),
            ParentCommand::SetModel { model } => {
                minerve.set_model(model.clone());
                minerve.fetch_context_window_in_background();
                self.recent.mark_used(&model);
                control.send(&MinerveEvent::Notice { notice: &format!("model: {model}") });
                self.send_models(minerve, control, Vec::new());
            }
            ParentCommand::ListSessions {} => {
                let sessions: Vec<SessionOut> = crate::session::list_sessions()
                    .into_iter()
                    .map(|summary| SessionOut {
                        modified: summary.modified.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
                        id: summary.id,
                        first_prompt: summary.first_prompt,
                    })
                    .collect();
                control.send(&MinerveEvent::Sessions { sessions: &sessions });
            }
            ParentCommand::ResumeSession { id } => {
                if busy(minerve, control) {
                    return true;
                }
                match crate::session::load_for_resume(&id) {
                    Ok(file) => {
                        minerve.replace_session(file);
                        control.send(&MinerveEvent::Transcript { messages: &minerve.ui_messages() });
                        control.send(&MinerveEvent::Usage { summary: &minerve.token_counter.format_usage_summary() });
                    }
                    Err(err) => control.send(&MinerveEvent::Notice { notice: &err }),
                }
            }
            ParentCommand::GetApiKeys {} => send_api_keys(minerve, control),
            ParentCommand::SetApiKeys { openrouter, openai, anthropic } => {
                let non_empty = |key: Option<String>| key.filter(|key| !key.trim().is_empty());
                let settings = setup::Settings {
                    openrouter_key: non_empty(openrouter),
                    openai_key: non_empty(openai),
                    anthropic_key: non_empty(anthropic),
                };
                match setup::save(&settings) {
                    Ok(path) => {
                        minerve.set_api_keys(settings.openrouter_key, settings.openai_key, settings.anthropic_key);
                        self.keep_a_model_with_a_key(minerve);
                        control.send(&MinerveEvent::Notice { notice: &format!("saved to {}", path.display()) });
                    }
                    Err(err) => control.send(&MinerveEvent::Notice { notice: &format!("could not save the keys: {err}") }),
                }
                send_api_keys(minerve, control);
            }
            other => return crate::serve_mcp::handle(&other, control),
        }
        true
    }

    /// Keeps the active model if its provider has a key, else the most recent model with a key.
    fn keep_a_model_with_a_key(&self, minerve: &Arc<Minerve>) {
        let keys = minerve.api_keys_snapshot();
        let active = minerve.active_model();
        let candidates = std::iter::once(active.id.clone()).chain(self.recent.ids().iter().cloned());
        let picked = crate::openrouter::pick_model_with_key(candidates, &keys);
        if picked != active.id {
            minerve.set_model(picked);
            minerve.fetch_context_window_in_background();
        }
    }

    fn list_models(&mut self, minerve: &Arc<Minerve>, control: &ControlSocket, ui: &UiSender) {
        if self.pending.is_some() {
            return;
        }
        let keys = minerve.api_keys_snapshot();
        let keyed: Vec<Provider> =
            PROVIDERS_BY_DEFAULT_PRIORITY.into_iter().filter(|provider| keys.get(*provider).is_some()).collect();
        if keyed.is_empty() {
            self.send_models(minerve, control, Vec::new());
            return;
        }
        self.pending = Some(PendingModels { expected: keyed.len(), lists: Vec::new() });
        for provider in keyed {
            let api_key = keys.get(provider).expect("filtered on key presence").to_string();
            let minerve = minerve.clone();
            let ui = ui.clone();
            crate::get_global_runtime().spawn(async move {
                let result =
                    crate::model_lists::fetch_model_ids(&minerve.client, provider, &api_key, &minerve.base_url).await;
                let _ = ui.send(UiEvent::ModelList(provider, result));
            });
        }
    }

    /// One provider answered; once all did, the parent gets the whole list.
    pub fn on_model_list(&mut self, provider: Provider, result: Result<Vec<String>, String>, minerve: &Minerve, control: &ControlSocket) {
        let Some(pending) = self.pending.as_mut() else { return };
        let (models, error) = match result {
            Ok(models) => (models, None),
            Err(err) => (Vec::new(), Some(err)),
        };
        pending.lists.push(ModelListOut { provider: provider.label(), models, error });
        if pending.lists.len() >= pending.expected {
            let lists = self.pending.take().map(|pending| pending.lists).unwrap_or_default();
            self.send_models(minerve, control, lists);
        }
    }

    fn send_models(&self, minerve: &Minerve, control: &ControlSocket, lists: Vec<ModelListOut>) {
        control.send(&MinerveEvent::Models { current: &minerve.active_model().id, recent: self.recent.ids(), lists: &lists });
    }
}

fn busy(minerve: &Minerve, control: &ControlSocket) -> bool {
    let busy = minerve.request_in_flight.load(Ordering::SeqCst);
    if busy {
        control.send(&MinerveEvent::Notice { notice: "busy: a turn is running" });
    }
    busy
}

fn send_api_keys(minerve: &Minerve, control: &ControlSocket) {
    let keys = minerve.api_keys_snapshot();
    control.send(&MinerveEvent::ApiKeys {
        openrouter: keys.get(Provider::OpenRouter).is_some(),
        openai: keys.get(Provider::OpenAi).is_some(),
        anthropic: keys.get(Provider::Anthropic).is_some(),
    });
}

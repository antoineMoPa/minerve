//! The tasks a turn runs in: a prompt, a `/goal`, and the guard that ends a turn whose task panicked.

use super::*;

impl Minerve {
    /// Ends the turn once no queued message is left: those `run_turn` did not take are answered first.
    async fn end_turn_after_answering_queued_prompts(&self, ui: &UiSender, is_headless: bool) {
        while let Some(prompt) = self.end_turn_or_take_queued_prompts(ui) {
            self.add_prompt(prompt, ui);
            let _ = self.run_turn(ui, is_headless).await;
        }
    }

    /// Ends a turn whose task panicked, so minerve takes prompts again: without this the turn
    /// stayed "working" for good and every new prompt waited in the queue. Locks the panic left
    /// poisoned are cleared first.
    fn end_turn_after_panic(&self, ui: &UiSender) {
        self.messages.clear_poison();
        self.queued_prompts.clear_poison();
        self.session_id.clear_poison();
        self.model.clear_poison();
        send_notice(
            ui,
            format!(
                "The turn stopped on a bug in minerve (details in ~/{}). Your next message starts a new turn.",
                crate::PANIC_LOG_PATH
            ),
        );
        self.end_turn(ui);
    }

    /// Sends a prompt typed by the user, with the image files pasted into it.
    pub fn chat_with_arc(
        self: Arc<Self>,
        user_input: String,
        images: Vec<PathBuf>,
        ui: UiSender,
        is_headless: bool,
    ) {
        self.start_turn(
            ChatCompletionMessage::user_with_images(user_input, images),
            &ui,
        );
        crate::get_global_runtime().spawn(async move {
            let _turn_guard = TurnEndsOnPanic::new(&self, &ui);
            // A failed request is already shown in the chat.
            let _ = self.run_turn(&ui, is_headless).await;
            self.end_turn_after_answering_queued_prompts(&ui, is_headless)
                .await;
        });
    }

    /// `/goal`: takes turns until the goal evaluator judges the goal reached, the user cancels
    /// with Esc, the evaluator fails, or `MAX_GOAL_TURNS` turns ran. How it ended is sent as a notice.
    pub fn pursue_goal(self: Arc<Self>, goal: String, ui: UiSender) {
        let goal_start = self.messages.lock().unwrap().len();
        self.start_turn(ChatCompletionMessage::user(goal::start_prompt(&goal)), &ui);
        crate::get_global_runtime().spawn(async move {
            let _turn_guard = TurnEndsOnPanic::new(&self, &ui);
            let provider = self.active_model().provider;
            let mut outcome = goal::GoalOutcome::NotReached;
            // Copy of the goal's messages for the evaluator. A new prompt cleans tool results out of
            // `messages`; this copy keeps them, so every turn's results can be checked.
            let mut goal_messages: Vec<ChatCompletionMessage> = Vec::new();
            let mut copied_until = goal_start;
            for turn in 1..=goal::MAX_GOAL_TURNS {
                if let Err(error) = self.run_turn(&ui, false).await {
                    outcome = goal::GoalOutcome::RequestFailed { error };
                    break;
                }
                if self.over_budget().is_some() {
                    outcome = goal::GoalOutcome::OverBudget;
                    break;
                }
                // Registered before checking the flag, so a cancel arriving in between is not missed.
                let cancelled = self.cancel_notify.notified();
                tokio::pin!(cancelled);
                cancelled.as_mut().enable();
                if self.cancel_requested.load(Ordering::SeqCst) {
                    outcome = goal::GoalOutcome::Cancelled;
                    break;
                }
                send_status(
                    &ui,
                    format!("goal: evaluating turn {}/{}...", turn, goal::MAX_GOAL_TURNS),
                );
                {
                    let messages = self.messages.lock().unwrap();
                    goal_messages.extend_from_slice(&messages[copied_until..]);
                    copied_until = messages.len();
                }
                let verdict = tokio::select! {
                    verdict = goal::evaluate(&self, provider, &goal, &goal_messages) => verdict,
                    _ = &mut cancelled => {
                        outcome = goal::GoalOutcome::Cancelled;
                        break;
                    }
                };
                let (verdict, evaluator_model_id) = match verdict {
                    Ok(verdict) => verdict,
                    Err(errors) => {
                        outcome = goal::GoalOutcome::EvaluatorFailed { errors };
                        break;
                    }
                };
                match verdict.status {
                    goal::GoalStatus::Reached => {
                        outcome = goal::GoalOutcome::Reached {
                            turns: turn,
                            evaluator_model_id,
                            reason: verdict.reason,
                        };
                        break;
                    }
                    goal::GoalStatus::Stalled => {
                        outcome = goal::GoalOutcome::Stalled {
                            turns: turn,
                            evaluator_model_id,
                            reason: verdict.reason,
                        };
                        break;
                    }
                    goal::GoalStatus::NotReached => self.add_prompt(
                        ChatCompletionMessage::authored_prompt(
                            PromptAuthor::GoalEvaluator,
                            goal::continue_prompt(&goal, &verdict.reason),
                        ),
                        &ui,
                    ),
                }
            }
            send_notice(&ui, outcome.notice());
            self.end_turn_after_answering_queued_prompts(&ui, false)
                .await;
        });
    }
}

/// Held by a turn's task: when the task panics, the turn is ended from a new task (see
/// `Minerve::end_turn_after_panic`). Nothing runs on the panicking thread, where a second
/// panic would abort the process.
pub(super) struct TurnEndsOnPanic {
    minerve: Arc<Minerve>,
    ui: UiSender,
}

impl TurnEndsOnPanic {
    pub(super) fn new(minerve: &Arc<Minerve>, ui: &UiSender) -> Self {
        Self {
            minerve: minerve.clone(),
            ui: ui.clone(),
        }
    }
}

impl Drop for TurnEndsOnPanic {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let minerve = self.minerve.clone();
        let ui = self.ui.clone();
        crate::get_global_runtime().spawn(async move { minerve.end_turn_after_panic(&ui) });
    }
}

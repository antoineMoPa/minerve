//! Messages the user sends while a turn runs. They do not wait for the turn to end: the turn
//! adds them, as one user message, right after the tool calls it is running are answered, so the
//! model reads them with its next request. A turn about to end takes them and goes on instead.
//!
//! The queue is only filled while a turn runs, and a turn only ends once it is empty.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use crate::control_socket::MinerveEvent;
use crate::minerve::Minerve;
use crate::ui::UiSender;
use crate::ChatCompletionMessage;

/// A message as the user sent it, with the image files pasted into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedPrompt {
    pub text: String,
    pub images: Vec<PathBuf>,
}

/// Every waiting message as one, oldest first, so the model reads them all before it answers.
fn merge_queued_prompts(prompts: Vec<QueuedPrompt>) -> Option<QueuedPrompt> {
    let mut prompts = prompts.into_iter();
    let mut merged = prompts.next()?;
    for prompt in prompts {
        merged.text.push_str("\n\n");
        merged.text.push_str(&prompt.text);
        merged.images.extend(prompt.images);
    }
    Some(merged)
}

impl Minerve {
    /// Keeps `prompt` for the running turn and says how many now wait.
    /// Gives it back when no turn runs: it is then a turn to start.
    pub fn queue_prompt_for_running_turn(
        &self,
        prompt: QueuedPrompt,
    ) -> Result<usize, QueuedPrompt> {
        // The lock is taken before the flag is read, see `end_turn_or_take_queued_prompts`.
        let mut queue = self.queued_prompts.lock().unwrap();
        if !self.request_in_flight.load(Ordering::SeqCst) {
            return Err(prompt);
        }
        queue.push(prompt);
        Ok(queue.len())
    }

    /// Takes back the newest waiting message, e.g. to edit it. `None` once the turn took it.
    pub fn unqueue_newest_prompt(&self) -> Option<QueuedPrompt> {
        self.queued_prompts.lock().unwrap().pop()
    }

    /// The text of the messages still waiting, oldest first.
    pub fn list_queued_prompt_texts(&self) -> Vec<String> {
        let queue = self.queued_prompts.lock().unwrap();
        queue.iter().map(|prompt| prompt.text.clone()).collect()
    }

    /// The waiting messages as the one user message to add to the conversation now.
    pub(crate) fn take_queued_prompts_as_one_message(&self) -> Option<ChatCompletionMessage> {
        let prompts = std::mem::take(&mut *self.queued_prompts.lock().unwrap());
        self.build_message_from_taken_prompts(prompts)
    }

    /// Ends the turn, unless messages came too late for its last request (or it was stopped, or
    /// its request failed): these are given back, and the caller runs the turn again with them.
    pub(crate) fn end_turn_or_take_queued_prompts(
        &self,
        ui: &UiSender,
    ) -> Option<ChatCompletionMessage> {
        // Held until the turn is marked as ended, so no message is queued for a turn that just ended.
        let mut queue = self.queued_prompts.lock().unwrap();
        if queue.is_empty() {
            self.end_turn(ui);
            return None;
        }
        let prompts = std::mem::take(&mut *queue);
        drop(queue);
        // A stop ended the requests of before, not the answer to these messages.
        self.cancel_requested.store(false, Ordering::SeqCst);
        self.build_message_from_taken_prompts(prompts)
    }

    fn build_message_from_taken_prompts(
        &self,
        prompts: Vec<QueuedPrompt>,
    ) -> Option<ChatCompletionMessage> {
        let count = prompts.len();
        let merged = merge_queued_prompts(prompts)?;
        self.send_control_event(&MinerveEvent::QueuedSent { count });
        Some(ChatCompletionMessage::user_with_images(
            merged.text,
            merged.images,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_prompt(text: &str, image: Option<&str>) -> QueuedPrompt {
        QueuedPrompt {
            text: text.to_string(),
            images: image.into_iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn waiting_messages_merge_into_one_and_keep_their_order() {
        let merged = merge_queued_prompts(vec![
            build_prompt("one", Some("a.png")),
            build_prompt("two", None),
            build_prompt("three", Some("b.png")),
        ]);
        assert_eq!(
            merged,
            Some(QueuedPrompt {
                text: String::from("one\n\ntwo\n\nthree"),
                images: vec![PathBuf::from("a.png"), PathBuf::from("b.png")],
            })
        );
        assert_eq!(merge_queued_prompts(Vec::new()), None);
    }
}

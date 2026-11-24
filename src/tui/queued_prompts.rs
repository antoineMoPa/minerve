//! What the user sent while a turn was running. It waits until the turn ends.

use std::collections::VecDeque;
use std::path::PathBuf;

pub enum QueuedPrompt {
    /// A normal message, with the image files pasted into it.
    Message {
        content: String,
        images: Vec<PathBuf>,
    },
    /// A `/goal <goal>` to pursue.
    Goal(String),
}

/// The prompt to send now that no turn runs. Messages waiting one after the other go together,
/// as one message, so the model reads them all before it answers. A goal goes alone.
pub fn take_next_queued_prompt(queue: &mut VecDeque<QueuedPrompt>) -> Option<QueuedPrompt> {
    let (mut content, mut images) = match queue.pop_front()? {
        QueuedPrompt::Message { content, images } => (content, images),
        goal @ QueuedPrompt::Goal(_) => return Some(goal),
    };
    while matches!(queue.front(), Some(QueuedPrompt::Message { .. })) {
        if let Some(QueuedPrompt::Message {
            content: next_content,
            images: next_images,
        }) = queue.pop_front()
        {
            content.push_str("\n\n");
            content.push_str(&next_content);
            images.extend(next_images);
        }
    }
    Some(QueuedPrompt::Message { content, images })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(content: &str, image: Option<&str>) -> QueuedPrompt {
        QueuedPrompt::Message {
            content: content.to_string(),
            images: image.into_iter().map(PathBuf::from).collect(),
        }
    }

    fn describe(prompt: Option<QueuedPrompt>) -> String {
        match prompt {
            Some(QueuedPrompt::Message { content, images }) => format!("{content:?} {images:?}"),
            Some(QueuedPrompt::Goal(goal)) => format!("goal {goal}"),
            None => String::from("nothing"),
        }
    }

    #[test]
    fn waiting_messages_go_as_one_and_a_goal_goes_alone() {
        let mut queue = VecDeque::from([
            message("one", Some("a.png")),
            message("two", None),
            QueuedPrompt::Goal(String::from("ship it")),
            message("three", Some("b.png")),
        ]);
        assert_eq!(
            describe(take_next_queued_prompt(&mut queue)),
            r#""one\n\ntwo" ["a.png"]"#
        );
        assert_eq!(
            describe(take_next_queued_prompt(&mut queue)),
            "goal ship it"
        );
        assert_eq!(
            describe(take_next_queued_prompt(&mut queue)),
            r#""three" ["b.png"]"#
        );
        assert_eq!(describe(take_next_queued_prompt(&mut queue)), "nothing");
    }
}

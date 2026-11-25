//! Models that take no image input (e.g. a text-only model on OpenRouter) reject a request with
//! an image. Instead of ending the turn on that error, the model is remembered and its requests
//! carry a text note in place of each image.

use crate::chat::ChatCompletionMessage;

/// Whether an API error says the model cannot take images.
/// OpenRouter: "No endpoints found that support image input".
/// OpenAI: "image_url is only supported by certain models".
pub fn is_image_input_unsupported_error(error: &str) -> bool {
    let error = error.to_lowercase();
    error.contains("support image input")
        || error.contains("image input is not supported")
        || error.contains("image_url is only supported")
        || error.contains("does not support image")
}

pub fn has_any_image(history: &[ChatCompletionMessage]) -> bool {
    history.iter().any(|message| !message.images.is_empty())
}

/// The history with every image replaced by a note naming its file, for text-only models.
pub fn replace_images_with_text_notes(
    history: Vec<ChatCompletionMessage>,
) -> Vec<ChatCompletionMessage> {
    history
        .into_iter()
        .map(|mut message| {
            if message.images.is_empty() {
                return message;
            }
            let notes: Vec<String> = std::mem::take(&mut message.images)
                .iter()
                .map(|path| {
                    format!(
                        "[Image {} not shown: this model takes no image input]",
                        path.display()
                    )
                })
                .collect();
            let text = match message.content.take() {
                Some(content) if !content.is_empty() => {
                    format!("{}\n{}", content, notes.join("\n"))
                }
                _ => notes.join("\n"),
            };
            message.content = Some(text);
            message
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn recognizes_the_openrouter_error() {
        assert!(is_image_input_unsupported_error(
            r#"404 Not Found: {"error":{"message":"No endpoints found that support image input","code":404}}"#
        ));
        assert!(!is_image_input_unsupported_error("429 Too Many Requests"));
    }

    #[test]
    fn replaces_images_with_notes() {
        let message = ChatCompletionMessage::user_with_images(
            "look".to_string(),
            vec![PathBuf::from("/a.png")],
        );
        let out = replace_images_with_text_notes(vec![message]);
        assert!(out[0].images.is_empty());
        assert_eq!(
            out[0].content.as_deref(),
            Some("look\n[Image /a.png not shown: this model takes no image input]")
        );
    }
}

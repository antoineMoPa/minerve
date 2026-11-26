//! Posting a chat request to a model provider.

use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;

use crate::anthropic;
use crate::provider::Provider;
use crate::token_counter::TokenCounter;
use crate::{ChatCompletionRequest, ChatCompletionResponse};

/// Only connecting is bounded: an answer is not streamed and a long one is legitimate, so the
/// wait for it has no limit (the user stops a turn that stalls).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// The HTTP client of a session.
pub fn build_model_client() -> Client {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .expect("HTTP client")
}

/// Sends `request` to the API of `provider` and records its usage in `token_counter`.
/// `url` is the endpoint of that provider; `api_key` is the key of that provider.
pub async fn post_request_with_token_count(
    client: &Client,
    provider: Provider,
    url: &str,
    api_key: &str,
    request: ChatCompletionRequest,
    token_counter: Arc<TokenCounter>,
) -> Result<ChatCompletionResponse, String> {
    let chat_response = match provider {
        Provider::Anthropic => anthropic::post_messages(client, url, api_key, &request).await?,
        Provider::OpenRouter | Provider::OpenAi => {
            post_chat_completions(client, url, api_key, &request).await?
        }
    };

    if let Some(ref usage) = chat_response.usage {
        // Correctly use the increment with the provided token_counter
        token_counter.increment_prompt(usage.prompt_tokens as usize);
        token_counter.increment_completion(usage.completion_tokens as usize);
        token_counter.add_request_cost(usage.cost);
        token_counter.set_last_prompt_tokens(usage.prompt_tokens as usize);
        if let Some(details) = &usage.prompt_tokens_details {
            token_counter.increment_cached_prompt(details.cached_tokens as usize);
        }
    }

    Ok(chat_response)
}

/// OpenRouter and OpenAI share the chat completions request and answer shape.
async fn post_chat_completions(
    client: &Client,
    url: &str,
    api_key: &str,
    request: &ChatCompletionRequest,
) -> Result<ChatCompletionResponse, String> {
    let response = client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(request)
        .send()
        .await
        .map_err(|err| err.to_string())?;

    let status = response.status();
    let body = response.text().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("{}: {}", status, body));
    }

    serde_json::from_str(&body).map_err(|err| format!("Unexpected response ({}): {}", err, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::RequestMessage;
    use crate::ChatCompletionMessage;
    use std::io::{Read, Write};

    fn build_small_request() -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: String::from("x/one"),
            messages: vec![RequestMessage::from_message(&ChatCompletionMessage::user(
                String::from("hi"),
            ))],
            tools: None,
            usage: None,
            session_id: None,
        }
    }

    /// Serves one connection: reads the request, then answers `body`.
    fn serve_one_request(body: &'static str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        });
        url
    }

    #[tokio::test]
    async fn an_answer_without_choices_is_an_error_not_a_panic() {
        let url = serve_one_request(r#"{"choices":[]}"#);
        let response = post_request_with_token_count(
            &build_model_client(),
            Provider::OpenRouter,
            &url,
            "sk-test",
            build_small_request(),
            Arc::new(TokenCounter::new()),
        )
        .await
        .unwrap();
        let err = response.take_first_message().unwrap_err();
        assert!(err.contains("no choices"), "{err}");
    }
}

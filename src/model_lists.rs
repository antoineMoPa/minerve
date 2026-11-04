//! Model lists of each provider, fetched for the `/model` picker.
//! Every id comes back with its provider prefix (`openrouter:`, `openai:`, `anthropic:`),
//! the same id `ActiveModel::new` routes, so typing the provider name in the picker finds its models.

use reqwest::RequestBuilder;

use crate::provider::Provider;

const OPENAI_MODELS_URL: &str = "https://api.openai.com/v1/models";
/// The Anthropic list is paginated; 1000 is its maximum page size and covers every model.
const ANTHROPIC_MODELS_URL: &str = "https://api.anthropic.com/v1/models?limit=1000";

/// The list request of `provider`, authenticated with `api_key`.
fn models_request(
    client: &reqwest::Client,
    provider: Provider,
    api_key: &str,
    openrouter_base_url: &str,
) -> RequestBuilder {
    match provider {
        Provider::OpenRouter => client
            .get(format!("{}/models", openrouter_base_url))
            .bearer_auth(api_key),
        Provider::OpenAi => client.get(OPENAI_MODELS_URL).bearer_auth(api_key),
        Provider::Anthropic => client
            .get(ANTHROPIC_MODELS_URL)
            .header("x-api-key", api_key)
            .header("anthropic-version", crate::anthropic::ANTHROPIC_VERSION),
    }
}

/// Prefixed ids of every model `provider` lists, in the order it returns them.
/// All three APIs answer `{"data": [{"id": ...}, ...]}`.
pub async fn fetch_model_ids(
    client: &reqwest::Client,
    provider: Provider,
    api_key: &str,
    openrouter_base_url: &str,
) -> Result<Vec<String>, String> {
    let response = models_request(client, provider, api_key, openrouter_base_url)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("{}: {}", status, text));
    }
    let body: serde_json::Value = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    model_ids_from_list(provider, &body)
}

fn model_ids_from_list(provider: Provider, body: &serde_json::Value) -> Result<Vec<String>, String> {
    let entries = body["data"]
        .as_array()
        .ok_or_else(|| format!("unexpected model list: {}", body))?;
    entries
        .iter()
        .map(|entry| {
            entry["id"]
                .as_str()
                .map(|id| format!("{}{}", provider.id_prefix(), id))
                .ok_or_else(|| format!("model list entry without id: {}", entry))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_get_the_provider_prefix() {
        let body = serde_json::json!({"data": [{"id": "claude-haiku-4-5"}, {"id": "claude-opus-4-1"}]});
        assert_eq!(
            model_ids_from_list(Provider::Anthropic, &body).unwrap(),
            ["anthropic:claude-haiku-4-5", "anthropic:claude-opus-4-1"]
        );
        let body = serde_json::json!({"data": [{"id": "anthropic/claude-3"}]});
        assert_eq!(
            model_ids_from_list(Provider::OpenRouter, &body).unwrap(),
            ["openrouter:anthropic/claude-3"]
        );
    }

    #[test]
    fn entry_without_id_is_an_error() {
        let body = serde_json::json!({"data": [{"name": "x"}]});
        assert!(model_ids_from_list(Provider::OpenAi, &body).is_err());
    }
}

use std::env;

use crate::provider::{route_model_id, ApiKeys, Provider};
use crate::recent_models::RecentModels;

pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const MODEL_ENV_VAR: &str = "MINERVE_MODEL";

/// Model id prefixes whose providers only cache up to explicit cache_control breakpoints.
/// The other providers on OpenRouter cache automatically.
const MODEL_PREFIXES_WITH_CACHE_BREAKPOINTS: [&str; 1] = ["anthropic/"];

/// Whether an OpenRouter model id needs explicit cache breakpoints in its requests.
pub fn model_uses_cache_breakpoints(model: &str) -> bool {
    // "~anthropic/claude-haiku-latest" style ids are OpenRouter aliases for the latest model.
    let model_id = model.strip_prefix('~').unwrap_or(model);
    MODEL_PREFIXES_WITH_CACHE_BREAKPOINTS
        .iter()
        .any(|prefix| model_id.starts_with(prefix))
}

/// Where Minerve sends requests and which keys it has. The model is the one the next turn uses.
pub struct OpenRouterConnection {
    pub base_url: String,
    pub api_keys: ApiKeys,
    pub model: String,
}

/// First of `candidates` whose provider has a key, else the default model of a provider with a key.
pub fn pick_model_with_key(candidates: impl IntoIterator<Item = String>, api_keys: &ApiKeys) -> String {
    candidates
        .into_iter()
        .find(|id| api_keys.get(route_model_id(id).0).is_some())
        .unwrap_or_else(|| {
            String::from(
                api_keys
                    .default_model_id()
                    .unwrap_or(Provider::OpenRouter.default_model_id()),
            )
        })
}

/// Context window size in tokens of `model`, from OpenRouter's public model list.
/// Returns None if the list cannot be fetched or does not contain the model.
pub async fn fetch_context_window(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
) -> Option<usize> {
    let body: serde_json::Value = client
        .get(format!("{}/models", base_url))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let entry = body["data"]
        .as_array()?
        .iter()
        .find(|entry| entry["id"].as_str() == Some(model))?;
    entry["context_length"].as_u64().map(|tokens| tokens as usize)
}

/// Reads the API keys of every provider; a key missing only fails requests to its provider.
/// The model is the first of MINERVE_MODEL, then the models picked with `/model` (most recent first),
/// whose provider has a key. Otherwise it is the default model of the first provider with a key
/// (see `ApiKeys::default_model_id`). With no key at all it is OpenRouter's default; the setup modal
/// then asks for a key.
pub fn read_openrouter_connection_from_env() -> OpenRouterConnection {
    let api_keys = ApiKeys::from_env();
    let recent_models = RecentModels::load();
    let candidates = env::var(MODEL_ENV_VAR)
        .ok()
        .into_iter()
        .chain(recent_models.ids().iter().cloned());
    let model = pick_model_with_key(candidates, &api_keys);

    OpenRouterConnection {
        // The override is for tests, which point minerve at a server of their own.
        base_url: std::env::var("MINERVE_OPENROUTER_BASE_URL").unwrap_or_else(|_| String::from(OPENROUTER_BASE_URL)),
        api_keys,
        model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anthropic_only() -> ApiKeys {
        ApiKeys {
            anthropic: Some(String::from("sk-ant")),
            ..ApiKeys::default()
        }
    }

    #[test]
    fn skips_recent_models_without_a_key() {
        let recent = ["openrouter:x/one", "anthropic:claude-opus-4-1"].map(String::from);
        assert_eq!(pick_model_with_key(recent, &anthropic_only()), "anthropic:claude-opus-4-1");
    }

    #[test]
    fn falls_back_to_the_default_of_a_keyed_provider() {
        let recent = ["~anthropic/claude-haiku-latest"].map(String::from);
        assert_eq!(pick_model_with_key(recent, &anthropic_only()), "anthropic:claude-haiku-5-5");
    }
}

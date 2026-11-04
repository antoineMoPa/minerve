use std::env;

const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const API_KEY_ENV_VAR: &str = "OPENROUTER_API_KEY";
const MODEL_ENV_VAR: &str = "MINERVE_MODEL";
const DEFAULT_MODEL: &str = "~anthropic/claude-haiku-latest";

/// Model id prefixes whose providers only cache up to explicit cache_control breakpoints.
/// The other providers on OpenRouter cache automatically.
const MODEL_PREFIXES_WITH_CACHE_BREAKPOINTS: [&str; 1] = ["anthropic/"];

pub struct OpenRouterConnection {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub uses_cache_breakpoints: bool,
}

fn model_uses_cache_breakpoints(model: &str) -> bool {
    // "~anthropic/claude-haiku-latest" style ids are OpenRouter aliases for the latest model.
    let model_id = model.strip_prefix('~').unwrap_or(model);
    MODEL_PREFIXES_WITH_CACHE_BREAKPOINTS
        .iter()
        .any(|prefix| model_id.starts_with(prefix))
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

/// Reads OPENROUTER_API_KEY (required) and MINERVE_MODEL (default: ~anthropic/claude-haiku-latest).
pub fn read_openrouter_connection_from_env() -> OpenRouterConnection {
    let api_key = env::var(API_KEY_ENV_VAR).unwrap_or_else(|_| {
        panic!(
            "{} is not set. Add it to ~/.env or export it. Create a key at https://openrouter.ai/keys",
            API_KEY_ENV_VAR
        )
    });
    let model = env::var(MODEL_ENV_VAR).unwrap_or_else(|_| String::from(DEFAULT_MODEL));

    OpenRouterConnection {
        base_url: String::from(OPENROUTER_BASE_URL),
        api_key,
        uses_cache_breakpoints: model_uses_cache_breakpoints(&model),
        model,
    }
}

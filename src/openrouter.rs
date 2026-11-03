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

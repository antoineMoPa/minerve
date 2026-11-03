use std::env;

const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const API_KEY_ENV_VAR: &str = "OPENROUTER_API_KEY";
const MODEL_ENV_VAR: &str = "MINERVE_MODEL";
const DEFAULT_MODEL: &str = "~anthropic/claude-haiku-latest";

pub struct OpenRouterConnection {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
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
        model,
    }
}

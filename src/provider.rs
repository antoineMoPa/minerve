//! Which API a model id is sent to.
//! - `openai:<model>` goes to the OpenAI chat completions API.
//! - `anthropic:<model>` goes to the Anthropic (Claude) Messages API.
//! - `openrouter:<model>`, such as `openrouter:~anthropic/claude-haiku-latest`, goes to OpenRouter.
//! - An id without prefix also goes to OpenRouter: ids saved before the prefixes existed have none.

use crate::openrouter::model_uses_cache_breakpoints;

pub const OPENROUTER_API_KEY_ENV_VAR: &str = "OPENROUTER_API_KEY";
pub const OPENAI_API_KEY_ENV_VAR: &str = "OPENAI_API_KEY";
pub const ANTHROPIC_API_KEY_ENV_VAR: &str = "ANTHROPIC_API_KEY";

const OPENAI_CHAT_COMPLETIONS_URL: &str = "https://api.openai.com/v1/chat/completions";
const ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const OPENROUTER_ID_PREFIX: &str = "openrouter:";
const OPENAI_ID_PREFIX: &str = "openai:";
const ANTHROPIC_ID_PREFIX: &str = "anthropic:";

/// Providers in the order their key is looked for when no model was chosen yet.
pub const PROVIDERS_BY_DEFAULT_PRIORITY: [Provider; 3] =
    [Provider::OpenRouter, Provider::Anthropic, Provider::OpenAi];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenRouter,
    OpenAi,
    Anthropic,
}

impl Provider {
    /// Name shown next to a model in the picker.
    pub fn label(self) -> &'static str {
        match self {
            Provider::OpenRouter => "OpenRouter",
            Provider::OpenAi => "OpenAI",
            Provider::Anthropic => "Anthropic",
        }
    }

    /// Prefix of the model ids routed to this provider.
    pub fn id_prefix(self) -> &'static str {
        match self {
            Provider::OpenRouter => OPENROUTER_ID_PREFIX,
            Provider::OpenAi => OPENAI_ID_PREFIX,
            Provider::Anthropic => ANTHROPIC_ID_PREFIX,
        }
    }

    /// Model used when none was chosen with MINERVE_MODEL or `/model`, and this provider has a key.
    pub fn default_model_id(self) -> &'static str {
        match self {
            Provider::OpenRouter => "openrouter:~anthropic/claude-haiku-latest",
            Provider::OpenAi => "openai:gpt-4o-mini",
            Provider::Anthropic => "anthropic:claude-haiku-5-5",
        }
    }

    /// URL requests of this provider are posted to. `openrouter_base_url` is only used for OpenRouter.
    pub fn request_url(self, openrouter_base_url: &str) -> String {
        match self {
            Provider::OpenRouter => format!("{}/chat/completions", openrouter_base_url),
            Provider::OpenAi => String::from(OPENAI_CHAT_COMPLETIONS_URL),
            Provider::Anthropic => String::from(ANTHROPIC_MESSAGES_URL),
        }
    }

    fn api_key_env_var(self) -> &'static str {
        match self {
            Provider::OpenRouter => OPENROUTER_API_KEY_ENV_VAR,
            Provider::OpenAi => OPENAI_API_KEY_ENV_VAR,
            Provider::Anthropic => ANTHROPIC_API_KEY_ENV_VAR,
        }
    }
}

/// API keys read from the environment. A key is None when its env var is not set;
/// a request to a provider without a key fails with an error saying which var to set.
#[derive(Clone, Default)]
pub struct ApiKeys {
    pub openrouter: Option<String>,
    pub openai: Option<String>,
    pub anthropic: Option<String>,
}

impl ApiKeys {
    /// Env vars win (`.env` is loaded into them first). `~/.minerve/keys.json` fills only what is unset.
    pub fn from_env() -> Self {
        let saved = crate::keys_file::load();
        Self {
            openrouter: std::env::var(OPENROUTER_API_KEY_ENV_VAR)
                .ok()
                .or(saved.openrouter_api_key),
            openai: std::env::var(OPENAI_API_KEY_ENV_VAR).ok().or(saved.openai_api_key),
            anthropic: std::env::var(ANTHROPIC_API_KEY_ENV_VAR)
                .ok()
                .or(saved.anthropic_api_key),
        }
    }

    pub fn get(&self, provider: Provider) -> Option<&str> {
        match provider {
            Provider::OpenRouter => self.openrouter.as_deref(),
            Provider::OpenAi => self.openai.as_deref(),
            Provider::Anthropic => self.anthropic.as_deref(),
        }
    }

    pub fn for_provider(&self, provider: Provider) -> Result<&str, String> {
        self.get(provider).ok_or_else(|| {
            format!(
                "no {} key ({}): add one with /apikeys, or pick a model of a provider you have a key for with /model.",
                provider.label(),
                provider.api_key_env_var()
            )
        })
    }

    /// Default model of the first provider that has a key. None when no key is set.
    pub fn default_model_id(&self) -> Option<&'static str> {
        PROVIDERS_BY_DEFAULT_PRIORITY
            .into_iter()
            .find(|provider| self.get(*provider).is_some())
            .map(Provider::default_model_id)
    }
}

/// The model requests go to. `Minerve` holds one and replaces it when the user switches models with `/model`.
#[derive(Clone)]
pub struct ActiveModel {
    /// Id as the user sees and saves it, e.g. `openai:gpt-4o`.
    pub id: String,
    pub provider: Provider,
    /// Model name put in the request body, without the provider prefix.
    pub api_model: String,
    pub uses_cache_breakpoints: bool,
}

/// Which provider a model id is sent to, and the id without its prefix. `openai:` and `anthropic:`
/// go to those APIs directly; every other id goes through OpenRouter.
pub fn route_model_id(id: &str) -> (Provider, &str) {
    if let Some(name) = id.strip_prefix(OPENROUTER_ID_PREFIX) {
        (Provider::OpenRouter, name)
    } else if let Some(name) = id.strip_prefix(OPENAI_ID_PREFIX) {
        (Provider::OpenAi, name)
    } else if let Some(name) = id.strip_prefix(ANTHROPIC_ID_PREFIX) {
        (Provider::Anthropic, name)
    } else {
        (Provider::OpenRouter, id)
    }
}

impl ActiveModel {
    pub fn new(id: String) -> Self {
        let (provider, api_model) = {
            let (provider, name) = route_model_id(&id);
            (provider, name.to_string())
        };
        let uses_cache_breakpoints = match provider {
            // OpenRouter only caches Anthropic models up to explicit breakpoints.
            Provider::OpenRouter => model_uses_cache_breakpoints(&api_model),
            // Anthropic needs breakpoints; OpenAI caches automatically.
            Provider::Anthropic => true,
            Provider::OpenAi => false,
        };
        Self {
            id,
            provider,
            api_model,
            uses_cache_breakpoints,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_without_prefix_go_to_openrouter() {
        let model = ActiveModel::new(String::from("~anthropic/claude-haiku-latest"));
        assert_eq!(model.provider, Provider::OpenRouter);
        assert_eq!(model.api_model, "~anthropic/claude-haiku-latest");
        assert!(model.uses_cache_breakpoints);
    }

    #[test]
    fn openrouter_prefix_is_stripped() {
        let model = ActiveModel::new(String::from("openrouter:~anthropic/claude-haiku-latest"));
        assert_eq!(model.provider, Provider::OpenRouter);
        assert_eq!(model.api_model, "~anthropic/claude-haiku-latest");
        assert!(model.uses_cache_breakpoints);
    }

    #[test]
    fn default_model_follows_the_first_key_set() {
        let keys = ApiKeys {
            anthropic: Some(String::from("sk-ant")),
            ..ApiKeys::default()
        };
        assert_eq!(keys.default_model_id(), Some("anthropic:claude-haiku-5-5"));
        assert_eq!(ApiKeys::default().default_model_id(), None);
    }

    #[test]
    fn openai_prefix_is_stripped_and_caches_automatically() {
        let model = ActiveModel::new(String::from("openai:gpt-4o"));
        assert_eq!(model.provider, Provider::OpenAi);
        assert_eq!(model.api_model, "gpt-4o");
        assert_eq!(model.id, "openai:gpt-4o");
        assert!(!model.uses_cache_breakpoints);
    }

    #[test]
    fn anthropic_prefix_is_stripped_and_uses_breakpoints() {
        let model = ActiveModel::new(String::from("anthropic:claude-3-5-haiku-latest"));
        assert_eq!(model.provider, Provider::Anthropic);
        assert_eq!(model.api_model, "claude-3-5-haiku-latest");
        assert!(model.uses_cache_breakpoints);
    }

    #[test]
    fn missing_key_names_the_env_var() {
        let keys = ApiKeys::default();
        let err = keys.for_provider(Provider::Anthropic).unwrap_err();
        assert!(err.contains("ANTHROPIC_API_KEY"));
    }
}

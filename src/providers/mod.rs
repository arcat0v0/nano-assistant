mod glm;
mod hub;

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use rig::agent::model::ModelHandle;
use rig::client::CompletionClient;
use rig::providers::{anthropic, gemini, openai};

use crate::config::Config;
use crate::hub::{model_routes_via_hub, resolve_hub_config, HubClient};

fn resolve_api_key(config: &Config, env_vars: &[&str]) -> Option<String> {
    env_vars
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .or_else(|| config.provider.api_key.clone())
}

pub fn build_model(config: &Config, config_path: &Path) -> anyhow::Result<ModelHandle> {
    let model = config.provider.model.as_deref().unwrap_or("gpt-4o-mini");
    if model_routes_via_hub(config) {
        let resolved = resolve_hub_config(config);
        let client = HubClient::new(config_path.to_path_buf(), config.clone())?;
        let transport = hub::AuthenticatedTransport::hub(client);
        let openai = openai::CompletionsClient::builder()
            .api_key("hub-signed")
            .base_url(format!("{}/v1", resolved.url))
            .http_client(transport)
            .build()?;
        return Ok(ModelHandle::named(
            "hub",
            openai.completion_model(model.strip_prefix("free/").unwrap_or(model)),
        ));
    }
    if model.starts_with("free/") {
        anyhow::bail!("free/* models require [hub].enabled = true or NANA_HUB_DISABLED unset");
    }

    let provider = config.provider.provider.as_deref().unwrap_or("openai");
    let base_url = config.provider.api_url.as_deref();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(config.provider.timeout_secs))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .context("building provider HTTP client")?;

    match provider {
        "openai" => {
            let key = resolve_api_key(config, &["NA_API_KEY", "OPENAI_API_KEY"])
                .context("OpenAI API key not set. Set OPENAI_API_KEY or edit config.toml.")?;
            let openai = openai::CompletionsClient::builder()
                .api_key(key)
                .base_url(
                    base_url
                        .unwrap_or("https://api.openai.com/v1")
                        .trim_end_matches('/'),
                )
                .http_client(client)
                .build()?;
            Ok(ModelHandle::named(provider, openai.completion_model(model)))
        }
        "anthropic" => {
            let key = resolve_api_key(config, &["NA_API_KEY", "ANTHROPIC_API_KEY"])
                .context("Anthropic API key not set. Set ANTHROPIC_API_KEY or edit config.toml.")?;
            let base = base_url.unwrap_or("https://api.anthropic.com");
            if key.starts_with("sk-ant-oat01-") {
                let transport = hub::AuthenticatedTransport::anthropic_oauth(key.clone(), client);
                let anthropic = anthropic::Client::builder()
                    .api_key(key)
                    .anthropic_beta("claude-code-20250219")
                    .anthropic_beta("oauth-2025-04-20")
                    .base_url(base)
                    .http_client(transport)
                    .build()?;
                let mut model = anthropic.completion_model(model);
                model.default_max_tokens = Some(4096);
                Ok(ModelHandle::named(provider, model))
            } else {
                let anthropic = anthropic::Client::builder()
                    .api_key(key)
                    .base_url(base)
                    .http_client(client)
                    .build()?;
                let mut model = anthropic.completion_model(model);
                model.default_max_tokens = Some(4096);
                Ok(ModelHandle::named(provider, model))
            }
        }
        "gemini" => {
            let key = resolve_api_key(config, &["NA_API_KEY", "GEMINI_API_KEY"])
                .context("Gemini API key not set. Set GEMINI_API_KEY or edit config.toml.")?;
            let base = base_url
                .unwrap_or("https://generativelanguage.googleapis.com")
                .trim_end_matches('/')
                .trim_end_matches("/v1beta");
            let gemini = gemini::Client::builder()
                .api_key(key)
                .base_url(base)
                .http_client(client)
                .build()?;
            Ok(ModelHandle::named(
                provider,
                gemini.completion_model(model.strip_prefix("models/").unwrap_or(model)),
            ))
        }
        "glm" => {
            let key = resolve_api_key(config, &["NA_API_KEY", "GLM_API_KEY"])
                .context("GLM API key not set. Set GLM_API_KEY or edit config.toml.")?;
            let transport = hub::AuthenticatedTransport::glm(&key, client)?;
            let openai = openai::CompletionsClient::builder()
                .api_key("glm-signed")
                .base_url(
                    base_url
                        .unwrap_or("https://api.z.ai/api/paas/v4")
                        .trim_end_matches('/'),
                )
                .http_client(transport)
                .build()?;
            Ok(ModelHandle::named(provider, openai.completion_model(model)))
        }
        "ollama" | "compatible" => {
            let key = resolve_api_key(config, &["NA_API_KEY"])
                .unwrap_or_else(|| "not-required".to_string());
            let default_url = if provider == "ollama" {
                "http://localhost:11434/v1"
            } else {
                "http://localhost:8080/v1"
            };
            let openai = openai::CompletionsClient::builder()
                .api_key(key)
                .base_url(base_url.unwrap_or(default_url).trim_end_matches('/'))
                .http_client(client)
                .build()?;
            Ok(ModelHandle::named(provider, openai.completion_model(model)))
        }
        other => anyhow::bail!(
            "unknown provider: '{other}'. Valid: openai, anthropic, gemini, glm, ollama, compatible"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    struct SavedKeys {
        previous: Vec<(&'static str, Option<OsString>)>,
    }

    impl SavedKeys {
        fn new(names: &[&'static str]) -> Self {
            Self {
                previous: names
                    .iter()
                    .map(|name| (*name, std::env::var_os(name)))
                    .collect(),
            }
        }
    }

    impl Drop for SavedKeys {
        fn drop(&mut self) {
            for (name, value) in &self.previous {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn api_key_precedence_prefers_global_env_then_provider_env_then_config() {
        let _saved = SavedKeys::new(&["NA_API_KEY", "OPENAI_API_KEY"]);
        let mut config = Config::default();
        config.provider.api_key = Some("config-key".into());
        std::env::set_var("OPENAI_API_KEY", "provider-key");
        std::env::set_var("NA_API_KEY", "global-key");
        assert_eq!(
            resolve_api_key(&config, &["NA_API_KEY", "OPENAI_API_KEY"]).as_deref(),
            Some("global-key")
        );

        std::env::remove_var("NA_API_KEY");
        assert_eq!(
            resolve_api_key(&config, &["NA_API_KEY", "OPENAI_API_KEY"]).as_deref(),
            Some("provider-key")
        );

        std::env::remove_var("OPENAI_API_KEY");
        assert_eq!(
            resolve_api_key(&config, &["NA_API_KEY", "OPENAI_API_KEY"]).as_deref(),
            Some("config-key")
        );
    }
}

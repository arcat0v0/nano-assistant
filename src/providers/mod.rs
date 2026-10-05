mod catalog;
mod hub;

pub use catalog::{builtin_providers, discover_models, preset, ProviderPreset};

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use rig::agent::model::ModelHandle;
use rig::client::CompletionClient;
use rig::providers::{anthropic, gemini, openai};

use crate::config::credentials::load_deepseek_key;
use crate::config::{Config, ResolvedModel};
use crate::hub::{model_routes_via_hub, resolve_hub_config, HubClient};

fn provider_key_env(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" => Some("OPENAI_API_KEY"),
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "gemini" => Some("GEMINI_API_KEY"),
        "deepseek" | "kimi" | "glm" | "mimo" | "qwen" => {
            preset(provider).map(|entry| entry.api_key_env)
        }
        _ => None,
    }
}

fn env_key(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn resolve_api_key(
    config: &Config,
    selected: &ResolvedModel,
    config_path: &Path,
) -> anyhow::Result<Option<String>> {
    if let Some(env_name) = selected.api_key_env.as_deref() {
        if let Some(key) = env_key(env_name) {
            return Ok(Some(key));
        }
        if selected.provider == "deepseek" {
            if let Some(key) = load_deepseek_key(config_path)? {
                return Ok(Some(key));
            }
        }
        return Ok(None);
    }

    if let Some(key) = env_key("NA_API_KEY") {
        return Ok(Some(key));
    }
    if let Some(env_name) = provider_key_env(&selected.provider) {
        if let Some(key) = env_key(env_name) {
            return Ok(Some(key));
        }
    }
    if selected.provider == "deepseek" {
        if let Some(key) = load_deepseek_key(config_path)? {
            return Ok(Some(key));
        }
    }
    if selected.allows_legacy_key {
        if let Some(key) = config
            .provider
            .api_key
            .as_ref()
            .filter(|key| !key.trim().is_empty())
        {
            return Ok(Some(key.clone()));
        }
    }

    if matches!(selected.provider.as_str(), "ollama" | "compatible") {
        return Ok(Some("not-required".into()));
    }

    Ok(None)
}

fn require_api_key(
    config: &Config,
    selected: &ResolvedModel,
    config_path: &Path,
) -> anyhow::Result<String> {
    resolve_api_key(config, selected, config_path)?.ok_or_else(|| {
        if let Some(env_name) = selected.api_key_env.as_deref() {
            anyhow::anyhow!(
                "model profile '{}' requires environment variable {env_name} or its provider credential",
                selected.profile_name.as_deref().unwrap_or("<unnamed>")
            )
        } else {
            let hint = provider_key_env(&selected.provider).unwrap_or("NA_API_KEY");
            anyhow::anyhow!(
                "{} API key not set for model '{}'. Set {hint} or configure this profile.",
                selected.provider,
                selected.model
            )
        }
    })
}

pub fn credentials_available(
    config: &Config,
    selected: &ResolvedModel,
    config_path: &Path,
) -> anyhow::Result<bool> {
    Ok(resolve_api_key(config, selected, config_path)?.is_some())
}

pub fn build_model(
    selected: &ResolvedModel,
    config: &Config,
    config_path: &Path,
) -> anyhow::Result<ModelHandle> {
    let model = selected.model.as_str();
    if model_routes_via_hub(&selected.apply_to_config(config)) {
        let resolved = resolve_hub_config(config);
        let effective_config = selected.apply_to_config(config);
        let client = HubClient::new(config_path.to_path_buf(), effective_config)?;
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

    let provider = selected.provider.as_str();
    let base_url = selected.api_url.as_deref();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(selected.timeout_secs))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .context("building provider HTTP client")?;

    match provider {
        "openai" => {
            let key = require_api_key(config, selected, config_path)?;
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
            let key = require_api_key(config, selected, config_path)?;
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
            let key = require_api_key(config, selected, config_path)?;
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
        "deepseek" | "kimi" | "glm" | "mimo" | "qwen" => {
            let preset = preset(provider).expect("built-in provider has a preset");
            let key = require_api_key(config, selected, config_path)?;
            let url = base_url.unwrap_or(preset.base_url).trim_end_matches('/');
            if provider == "mimo" {
                let transport = hub::AuthenticatedTransport::mimo(key.clone(), client);
                let openai = openai::CompletionsClient::builder()
                    .api_key(key)
                    .base_url(url)
                    .http_client(transport)
                    .build()?;
                Ok(ModelHandle::named(provider, openai.completion_model(model)))
            } else {
                let openai = openai::CompletionsClient::builder()
                    .api_key(key)
                    .base_url(url)
                    .http_client(client)
                    .build()?;
                Ok(ModelHandle::named(provider, openai.completion_model(model)))
            }
        }
        "ollama" | "compatible" => {
            let key = require_api_key(config, selected, config_path)?;
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
            "unknown provider: '{other}'. Valid: openai, anthropic, gemini, deepseek, kimi, glm, mimo, qwen, ollama, compatible"
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
        let selected = crate::config::models::resolve_selection(&config, None, None, None).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::env::set_var("OPENAI_API_KEY", "provider-key");
        std::env::set_var("NA_API_KEY", "global-key");
        assert_eq!(
            resolve_api_key(&config, &selected, &path).unwrap(),
            Some("global-key".into())
        );

        std::env::remove_var("NA_API_KEY");
        assert_eq!(
            resolve_api_key(&config, &selected, &path).unwrap(),
            Some("provider-key".into())
        );

        std::env::remove_var("OPENAI_API_KEY");
        assert_eq!(
            resolve_api_key(&config, &selected, &path).unwrap(),
            Some("config-key".into())
        );
    }

    #[test]
    fn cli_and_tui_credential_checks_share_deepseek_key_file_resolution() {
        let _saved = SavedKeys::new(&["NA_API_KEY", "DEEPSEEK_API_KEY"]);
        std::env::remove_var("NA_API_KEY");
        std::env::remove_var("DEEPSEEK_API_KEY");
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.toml");
        crate::config::credentials::save_deepseek_key(&config_path, "sk-fixture-only").unwrap();
        let mut config = Config::default();
        config.models.profiles.insert(
            "saved".into(),
            crate::config::ModelProfile {
                provider: "deepseek".into(),
                model: "deepseek-flash".into(),
                api_url: None,
                api_key_env: None,
                temperature: None,
                timeout_secs: None,
                reasoning_effort: None,
            },
        );
        let selected = crate::config::models::resolve_profile(
            &config,
            "saved",
            crate::config::SelectionSource::Session,
        )
        .unwrap();
        assert_eq!(
            resolve_api_key(&config, &selected, &config_path).unwrap(),
            Some("sk-fixture-only".into())
        );
        assert!(credentials_available(&config, &selected, &config_path).unwrap());
        assert!(build_model(&selected, &config, &config_path).is_ok());
    }

    #[test]
    #[serial_test::serial]
    fn profile_custom_key_missing_rejects_legacy_and_generic_fallback() {
        let _saved = SavedKeys::new(&["NA_API_KEY", "OPENAI_API_KEY", "MISSING_PROFILE_KEY"]);
        std::env::set_var("NA_API_KEY", "generic-key");
        std::env::set_var("OPENAI_API_KEY", "provider-key");
        std::env::remove_var("MISSING_PROFILE_KEY");
        let mut config = Config::default();
        config.provider.api_key = Some("legacy-secret".into());
        config.models.profiles.insert(
            "private".into(),
            crate::config::ModelProfile {
                provider: "openai".into(),
                model: "gpt-4o-mini".into(),
                api_url: None,
                api_key_env: Some("MISSING_PROFILE_KEY".into()),
                temperature: None,
                timeout_secs: None,
                reasoning_effort: None,
            },
        );
        let selected =
            crate::config::models::resolve_selection(&config, Some("private"), None, None).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.toml");
        let error = build_model(&selected, &config, &config_path)
            .expect_err("missing profile key should reject model");
        assert!(error.to_string().contains("MISSING_PROFILE_KEY"));

        std::env::set_var("MISSING_PROFILE_KEY", "");
        let error = build_model(&selected, &config, &config_path)
            .expect_err("empty profile key should reject model");
        assert!(error.to_string().contains("MISSING_PROFILE_KEY"));

        std::env::set_var("MISSING_PROFILE_KEY", "profile-key");
        assert!(build_model(&selected, &config, &config_path).is_ok());
    }
}

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context};
use serde::Deserialize;

use crate::config::credentials::load_deepseek_key;
use crate::config::schema::Config;
use crate::config::schema::RuntimeApiKey;

pub const MODEL: &str = "deepseek-flash";
pub const API_BASE: &str = "https://api.deepseek.com";

pub fn api_base(config: &Config, catalog: &Config) -> String {
    let selected = config
        .active_profile
        .as_deref()
        .and_then(|name| catalog.models.profiles.get(name));
    if let Some(profile) = selected.filter(|profile| profile.provider == "deepseek") {
        if let Some(url) = profile.api_url.as_ref() {
            return url.trim_end_matches('/').to_owned();
        }
    }
    if config.provider.provider.as_deref() == Some("deepseek") {
        if let Some(url) = config.provider.api_url.as_ref() {
            return url.trim_end_matches('/').to_owned();
        }
    }
    if catalog.provider.provider.as_deref() == Some("deepseek") {
        if let Some(url) = catalog.provider.api_url.as_ref() {
            return url.trim_end_matches('/').to_owned();
        }
    }
    if let Some(profile) = catalog
        .models
        .default
        .as_deref()
        .and_then(|name| catalog.models.profiles.get(name))
        .filter(|profile| profile.provider == "deepseek")
    {
        if let Some(url) = profile.api_url.as_ref() {
            return url.trim_end_matches('/').to_owned();
        }
    }
    API_BASE.to_owned()
}

pub fn has_configured_key(
    current: &Config,
    catalog: &Config,
    config_path: &Path,
) -> anyhow::Result<bool> {
    if nonempty_env("DEEPSEEK_API_KEY").is_some() {
        return Ok(true);
    }

    for profile in catalog
        .models
        .profiles
        .values()
        .filter(|profile| profile.provider == "deepseek")
    {
        if profile
            .api_key_env
            .as_deref()
            .and_then(nonempty_env)
            .is_some()
        {
            return Ok(true);
        }
    }

    if current.provider.provider.as_deref() == Some("deepseek")
        && (current
            .provider
            .api_key
            .as_deref()
            .is_some_and(|key| !key.is_empty())
            || nonempty_env("NA_API_KEY").is_some())
    {
        return Ok(true);
    }

    Ok(load_deepseek_key(config_path)?.is_some())
}

pub fn attach_saved_key(current: &mut Config, config_path: &Path) -> anyhow::Result<()> {
    if current.provider.provider.as_deref() != Some("deepseek") {
        return Ok(());
    }
    let explicit_key = nonempty_env("DEEPSEEK_API_KEY").or_else(|| nonempty_env("NA_API_KEY"));
    if explicit_key.is_some()
        || current
            .provider
            .api_key
            .as_deref()
            .is_some_and(|key| !key.is_empty())
    {
        return Ok(());
    }
    if let Some(key) = load_deepseek_key(config_path)? {
        current.runtime_api_key = Some(RuntimeApiKey::new("deepseek", key));
    }
    Ok(())
}

pub async fn validate_key(api_base: &str, key: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .context("building DeepSeek connection check")?;
    let url = format!("{}/models", api_base.trim_end_matches('/'));
    let response = client.get(url).bearer_auth(key).send().await?;
    if !response.status().is_success() {
        bail!("DeepSeek returned HTTP {}", response.status().as_u16());
    }
    let catalog: ModelCatalog = response
        .json()
        .await
        .context("DeepSeek returned an unreadable model list")?;
    if !catalog.data.iter().any(|model| model.id == MODEL) {
        bail!("DeepSeek did not list the default model");
    }
    Ok(())
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

#[derive(Deserialize)]
struct ModelCatalog {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn mock(status: u16, body: &'static str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let count = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..count]).into_owned();
            write!(
                stream,
                "HTTP/1.1 {status} fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            request
        });
        (format!("http://{address}"), worker)
    }

    #[tokio::test]
    async fn checks_auth_with_a_mock_get_models_request() {
        let (base, worker) = mock(200, r#"{"data":[{"id":"deepseek-flash"}]}"#);
        validate_key(&base, "sk-test-only").await.unwrap();
        let request = worker.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /models http/1.1"));
        assert!(request.contains("authorization: bearer sk-test-only"));
    }

    #[tokio::test]
    async fn rejects_mock_auth_failure_without_including_key_in_error() {
        let (base, worker) = mock(401, r#"{"error":"unauthorized"}"#);
        let error = validate_key(&base, "sk-invalid-test").await.unwrap_err();
        assert!(error.to_string().contains("401"));
        assert!(!error.to_string().contains("sk-invalid-test"));
        assert!(worker.join().unwrap().contains("Bearer sk-invalid-test"));
    }
}

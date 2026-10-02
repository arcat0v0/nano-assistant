use std::time::Duration;

use anyhow::{bail, Context};
use serde::Deserialize;

use crate::config::ResolvedModel;

pub const MODEL: &str = "deepseek-flash";
pub const API_BASE: &str = "https://api.deepseek.com";

pub fn api_base(model: &ResolvedModel) -> String {
    model
        .api_url
        .as_deref()
        .unwrap_or(API_BASE)
        .trim_end_matches('/')
        .to_owned()
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

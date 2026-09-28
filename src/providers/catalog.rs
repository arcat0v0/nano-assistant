use std::collections::HashSet;
use std::time::Duration;

use anyhow::{bail, Context};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderPreset {
    pub id: &'static str,
    pub display_name: &'static str,
    pub base_url: &'static str,
    pub api_key_env: &'static str,
}

const PRESETS: [ProviderPreset; 5] = [
    ProviderPreset {
        id: "deepseek",
        display_name: "DeepSeek",
        base_url: "https://api.deepseek.com",
        api_key_env: "DEEPSEEK_API_KEY",
    },
    ProviderPreset {
        id: "kimi",
        display_name: "Kimi",
        base_url: "https://api.moonshot.cn/v1",
        api_key_env: "MOONSHOT_API_KEY",
    },
    ProviderPreset {
        id: "glm",
        display_name: "GLM (Z.AI)",
        base_url: "https://api.z.ai/api/paas/v4",
        api_key_env: "GLM_API_KEY",
    },
    ProviderPreset {
        id: "mimo",
        display_name: "MiMo",
        base_url: "https://api.xiaomimimo.com/v1",
        api_key_env: "MIMO_API_KEY",
    },
    ProviderPreset {
        id: "qwen",
        display_name: "Qwen",
        base_url: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        api_key_env: "DASHSCOPE_API_KEY",
    },
];

const GLM_PRICING_URL: &str = "https://docs.z.ai/guides/overview/pricing.md";

pub fn builtin_providers() -> &'static [ProviderPreset] {
    &PRESETS
}

pub fn preset(id: &str) -> Option<&'static ProviderPreset> {
    PRESETS.iter().find(|item| item.id == id)
}

pub async fn discover_models(
    provider: &str,
    api_url: Option<&str>,
    key_env: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    let entry = preset(provider).with_context(|| {
        format!("unknown built-in provider '{provider}'; choose deepseek, kimi, glm, mimo, or qwen")
    })?;
    let name = key_env.unwrap_or(entry.api_key_env);
    let key = std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{provider} model discovery requires {name}; set this environment variable to your API key"))?;
    let base_url = api_url.unwrap_or(entry.base_url).trim_end_matches('/');
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .context("building model discovery HTTP client")?;
    let models = match provider {
        "qwen" => fetch_qwen(&client, base_url, &key).await?,
        "glm" => {
            let pricing =
                (api_url.is_none() || api_url == Some(entry.base_url)).then_some(GLM_PRICING_URL);
            fetch_glm(&client, base_url, &key, pricing).await?
        }
        _ => fetch_openai_models(&client, provider, base_url, &key).await?,
    };
    if models.is_empty() {
        bail!("{provider} returned no models; check your account's model access and endpoint");
    }
    Ok(unique(models))
}

#[derive(Deserialize)]
struct OpenAiCatalog {
    data: Vec<OpenAiModel>,
}

#[derive(Deserialize)]
struct OpenAiModel {
    id: String,
}

#[derive(Deserialize)]
struct QwenCatalog {
    output: QwenPage,
}

#[derive(Deserialize)]
struct QwenPage {
    total: usize,
    models: Vec<QwenModel>,
}

#[derive(Deserialize)]
struct QwenModel {
    model: String,
}

fn unique(models: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    models
        .into_iter()
        .filter(|name| !name.trim().is_empty() && seen.insert(name.clone()))
        .collect()
}

async fn response_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    provider: &str,
) -> anyhow::Result<T> {
    if !response.status().is_success() {
        bail!("{provider} model discovery failed with HTTP {}; check your API key, model access, and endpoint", response.status());
    }
    response
        .json()
        .await
        .with_context(|| format!("{provider} model discovery returned an invalid response"))
}

async fn fetch_openai_models(
    client: &reqwest::Client,
    provider: &str,
    base_url: &str,
    key: &str,
) -> anyhow::Result<Vec<String>> {
    let url = format!("{base_url}/models");
    let request = client.get(url);
    let request = if provider == "mimo" {
        request.header("api-key", key)
    } else {
        request.bearer_auth(key)
    };
    let response = request
        .send()
        .await
        .with_context(|| format!("connecting to {provider} model catalog"))?;
    let catalog: OpenAiCatalog = response_json(response, provider).await?;
    Ok(catalog.data.into_iter().map(|model| model.id).collect())
}

fn qwen_catalog_url(base_url: &str) -> anyhow::Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(base_url).context("invalid Qwen API URL")?;
    let prefix = url.path().trim_end_matches('/');
    let prefix = prefix.strip_suffix("/compatible-mode/v1").unwrap_or(prefix);
    url.set_path(&format!("{prefix}/api/v1/models"));
    url.set_query(None);
    Ok(url)
}

async fn fetch_qwen(
    client: &reqwest::Client,
    base_url: &str,
    key: &str,
) -> anyhow::Result<Vec<String>> {
    let url = qwen_catalog_url(base_url)?;
    let mut models = Vec::new();
    let mut page_no = 1usize;
    loop {
        let response = client
            .get(url.clone())
            .bearer_auth(key)
            .query(&[
                ("providers", "qwen"),
                ("features", "function-calling"),
                ("page_no", &page_no.to_string()),
                ("page_size", "100"),
            ])
            .send()
            .await
            .context("connecting to Qwen model catalog")?;
        let page: QwenCatalog = response_json(response, "qwen").await?;
        if page.output.models.is_empty() && models.len() < page.output.total {
            bail!("Qwen model catalog returned an empty page before all models were listed");
        }
        models.extend(page.output.models.into_iter().map(|model| model.model));
        if models.len() >= page.output.total {
            break;
        }
        page_no += 1;
    }
    Ok(models)
}

async fn fetch_glm(
    client: &reqwest::Client,
    base_url: &str,
    key: &str,
    pricing_url: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    let response = client
        .get(format!("{base_url}/models"))
        .bearer_auth(key)
        .send()
        .await
        .context("connecting to glm model catalog")?;
    if matches!(
        response.status(),
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::METHOD_NOT_ALLOWED
    ) {
        if let Some(pricing_url) = pricing_url {
            let catalog = client
                .get(pricing_url)
                .send()
                .await
                .context("retrieving Z.AI's published model catalog from its pricing page")?
                .error_for_status()
                .context("Z.AI's published model catalog is unavailable")?
                .text()
                .await
                .context("reading Z.AI's published model catalog")?;
            let models = parse_glm_pricing(&catalog);
            if models.is_empty() {
                bail!("Z.AI's published model catalog has no text models; published listings do not guarantee account entitlement");
            }
            return Ok(models);
        }
    }
    let catalog: OpenAiCatalog = response_json(response, "glm").await?;
    Ok(catalog.data.into_iter().map(|model| model.id).collect())
}

fn parse_glm_pricing(markdown: &str) -> Vec<String> {
    let mut text_models = false;
    let mut models = Vec::new();
    for line in markdown.lines() {
        if line.starts_with("### ") {
            text_models = matches!(line.trim(), "### Latest Models" | "### Text Models");
        } else if line.starts_with("## ") {
            text_models = false;
        } else if text_models {
            if let Some(name) = line
                .trim()
                .strip_prefix('|')
                .and_then(|row| row.split('|').next())
            {
                let name = name.trim();
                if name.starts_with("GLM-")
                    && name
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '.')
                {
                    models.push(name.to_ascii_lowercase());
                }
            }
        }
    }
    unique(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, Instant};

    fn server(responses: Vec<(u16, &'static str)>) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "request never reached server");
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("accept failed: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let size = stream.read(&mut buffer).unwrap();
                    assert!(size > 0, "request ended without headers");
                    bytes.extend_from_slice(&buffer[..size]);
                    if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                        break;
                    }
                }
                requests.push(String::from_utf8(bytes).unwrap());
                let label = match status {
                    200 => "OK",
                    401 => "Unauthorized",
                    404 => "Not Found",
                    _ => "Error",
                };
                write!(stream, "HTTP/1.1 {status} {label}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            requests
        });
        (url, handle)
    }

    fn saved_key(name: &'static str, value: &'static str) -> impl Drop {
        struct Restore(&'static str, Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                if let Some(value) = self.1.take() {
                    std::env::set_var(self.0, value);
                } else {
                    std::env::remove_var(self.0);
                }
            }
        }
        let previous = std::env::var_os(name);
        std::env::set_var(name, value);
        Restore(name, previous)
    }

    #[test]
    fn published_glm_catalog_only_lists_text_models() {
        let markdown = "## Models\n### Latest Models\n| Model | Input |\n| :--- | :--- |\n| GLM-5.3-FlashX | $1 |\n### Text Models\n| Model | Input |\n| :--- | :--- |\n| GLM-5.3-FlashX | $1 |\n| GLM-4.7 | $1 |\n### Vision Models\n| Model | Input |\n| :--- | :--- |\n| GLM-4.6V | $1 |\n";
        assert_eq!(
            parse_glm_pricing(markdown),
            vec!["glm-5.3-flashx", "glm-4.7"]
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn vendor_catalog_requests_use_vendor_auth_and_configured_base() {
        let _key = saved_key("NA_CATALOG_TEST_KEY", "test-secret");
        for provider in ["deepseek", "kimi", "mimo", "glm"] {
            let (base, received) = server(vec![(200, r#"{"data":[{"id":"live-model"}]}"#)]);
            let models = discover_models(
                provider,
                Some(&format!("{base}/v1")),
                Some("NA_CATALOG_TEST_KEY"),
            )
            .await
            .unwrap();
            assert_eq!(models, ["live-model"]);
            let request = received.join().unwrap().pop().unwrap();
            assert!(request.starts_with("GET /v1/models HTTP/1.1"), "{request}");
            if provider == "mimo" {
                assert!(request.contains("api-key: test-secret\r\n"), "{request}");
                assert!(
                    !request.to_lowercase().contains("authorization:"),
                    "{request}"
                );
            } else {
                assert!(
                    request.contains("authorization: Bearer test-secret\r\n"),
                    "{request}"
                );
            }
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn qwen_fetches_every_page_with_vendor_and_tool_filters() {
        let _key = saved_key("NA_CATALOG_TEST_KEY", "test-secret");
        let (base, received) = server(vec![
            (
                200,
                r#"{"output":{"total":3,"models":[{"model":"qwen-a"},{"model":"qwen-b"}]}}"#,
            ),
            (
                200,
                r#"{"output":{"total":3,"models":[{"model":"qwen-c"}]}}"#,
            ),
        ]);
        let models = discover_models(
            "qwen",
            Some(&format!("{base}/compatible-mode/v1")),
            Some("NA_CATALOG_TEST_KEY"),
        )
        .await
        .unwrap();
        assert_eq!(models, ["qwen-a", "qwen-b", "qwen-c"]);
        let requests = received.join().unwrap();
        assert_eq!(requests.len(), 2);
        for (page, request) in requests.iter().enumerate() {
            assert!(request.starts_with("GET /api/v1/models?"), "{request}");
            assert!(
                request.contains(&format!("page_no={}", page + 1)),
                "{request}"
            );
            assert!(request.contains("providers=qwen"), "{request}");
            assert!(request.contains("features=function-calling"), "{request}");
            assert!(
                request.contains("authorization: Bearer test-secret\r\n"),
                "{request}"
            );
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn rejected_and_malformed_responses_never_succeed_or_leak_keys() {
        let _key = saved_key("NA_CATALOG_TEST_KEY", "test-secret");
        for (status, body) in [
            (401, r#"{"message":"rejected"}"#),
            (200, r#"{"unexpected":true}"#),
        ] {
            let (base, received) = server(vec![(status, body)]);
            let error = discover_models("glm", Some(&base), Some("NA_CATALOG_TEST_KEY"))
                .await
                .unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains("glm"), "{message}");
            assert!(!message.contains("test-secret"), "{message}");
            assert!(
                message.contains(if status == 401 { "401" } else { "response" }),
                "{message}"
            );
            assert_eq!(received.join().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn custom_glm_endpoint_never_uses_published_catalog_on_404() {
        let _key = saved_key("NA_CATALOG_TEST_KEY", "test-secret");
        let (base, received) = server(vec![(404, "not found")]);
        let error = discover_models("glm", Some(&base), Some("NA_CATALOG_TEST_KEY"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("404"));
        assert_eq!(received.join().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn mimo_chat_transport_replaces_bearer_with_vendor_api_key() {
        use bytes::Bytes;
        use rig::http_client::{HttpClientExt, Request};

        let (base, received) = server(vec![(200, "{}")]);
        let transport = super::super::hub::AuthenticatedTransport::mimo(
            "test-secret".to_owned(),
            reqwest::Client::new(),
        );
        let request = Request::builder()
            .method("POST")
            .uri(format!("{base}/v1/chat/completions"))
            .header("authorization", "Bearer rig-supplied")
            .body(Bytes::from_static(b"{}"))
            .unwrap();
        let response = transport.send::<Bytes, Bytes>(request).await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let request = received.join().unwrap().pop().unwrap();
        assert!(
            request.starts_with("POST /v1/chat/completions HTTP/1.1"),
            "{request}"
        );
        assert!(request.contains("api-key: test-secret\r\n"), "{request}");
        assert!(
            !request.to_ascii_lowercase().contains("authorization:"),
            "{request}"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn glm_uses_published_catalog_only_on_unsupported_default_endpoint() {
        let _key = saved_key("NA_CATALOG_TEST_KEY", "test-secret");
        let (catalog, catalog_requests) = server(vec![(
            200,
            "## Models\n### Latest Models\n| Model | Input |\n| :--- | :--- |\n| GLM-new | $1 |\n",
        )]);
        let (base, received) = server(vec![(404, "not found")]);
        let client = reqwest::Client::new();
        let result = fetch_glm(&client, &base, "test-secret", Some(&catalog))
            .await
            .unwrap();
        assert_eq!(result, ["glm-new"]);
        assert_eq!(received.join().unwrap().len(), 1);
        assert_eq!(catalog_requests.join().unwrap().len(), 1);
    }
}

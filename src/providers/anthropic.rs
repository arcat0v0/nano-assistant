use super::{ChatMessage, Provider};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub struct AnthropicProvider {
    api_key: Option<String>,
    base_url: String,
    max_tokens: u32,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    temperature: f64,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

impl AnthropicProvider {
    pub fn new(api_key: Option<&str>) -> Self {
        Self {
            api_key: api_key.map(str::trim).filter(|k| !k.is_empty()).map(ToString::to_string),
            base_url: "https://api.anthropic.com".to_string(),
            max_tokens: 4096,
        }
    }

    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = base_url.trim_end_matches('/').to_string();
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }

    fn apply_auth(&self, req: reqwest::RequestBuilder, key: &str) -> reqwest::RequestBuilder {
        if key.starts_with("sk-ant-oat01-") {
            req.header("Authorization", format!("Bearer {key}"))
                .header("anthropic-beta", "claude-code-20250219,oauth-2025-04-20")
        } else {
            req.header("x-api-key", key)
        }
    }

    async fn post(&self, request: &ChatRequest) -> anyhow::Result<String> {
        let key = self.api_key.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Anthropic API key not set. Set ANTHROPIC_API_KEY or edit config.toml.")
        })?;

        let response = self
            .apply_auth(
                self.client()
                    .post(format!("{}/v1/messages", self.base_url))
                    .header("anthropic-version", "2023-06-01")
                    .header("content-type", "application/json")
                    .json(request),
                key,
            )
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic API error ({status}): {body}");
        }

        let anth_response: AnthropicResponse = response.json().await?;
        anth_response
            .content
            .into_iter()
            .find(|c| c.kind == "text")
            .and_then(|c| c.text)
            .ok_or_else(|| anyhow::anyhow!("No response from Anthropic"))
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let request = ChatRequest {
            model: model.to_string(),
            max_tokens: self.max_tokens,
            system: system_prompt.map(ToString::to_string),
            messages: vec![AnthropicMessage { role: "user".into(), content: message.to_string() }],
            temperature,
        };
        self.post(&request).await
    }

    async fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let mut system_text = None;
        let mut anth_messages: Vec<AnthropicMessage> = Vec::new();

        for msg in messages {
            match msg.role.as_str() {
                "system" => {
                    if system_text.is_none() {
                        system_text = Some(msg.content.clone());
                    }
                }
                "tool" => {
                    let text = format!("<tool_result>\n{}\n</tool_result>", msg.content);
                    let last_is_user = if let Some(m) = anth_messages.last() { m.role == "user" } else { false };
                    if last_is_user {
                        anth_messages.last_mut().unwrap().content.push_str(&format!("\n\n{text}"));
                    } else {
                        anth_messages.push(AnthropicMessage { role: "user".into(), content: text });
                    }
                }
                _ => {
                    anth_messages.push(AnthropicMessage { role: msg.role.clone(), content: msg.content.clone() });
                }
            }
        }

        let request = ChatRequest {
            model: model.to_string(),
            max_tokens: self.max_tokens,
            system: system_text,
            messages: anth_messages,
            temperature,
        };
        self.post(&request).await
    }

    async fn warmup(&self) -> anyhow::Result<()> {
        if let Some(key) = self.api_key.as_ref() {
            let _ = self
                .apply_auth(
                    self.client()
                        .post(format!("{}/v1/messages", self.base_url))
                        .header("anthropic-version", "2023-06-01"),
                    key,
                )
                .send()
                .await?;
        }
        Ok(())
    }
}

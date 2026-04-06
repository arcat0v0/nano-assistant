use super::{ChatMessage, Provider};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub struct CompatibleProvider {
    name: String,
    base_url: String,
    api_key: Option<String>,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    temperature: f64,
}

#[derive(Serialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
    reasoning_content: Option<String>,
}

impl ResponseMessage {
    fn effective_content(&self) -> String {
        match &self.content {
            Some(c) if !c.is_empty() => c.clone(),
            _ => self.reasoning_content.clone().unwrap_or_default(),
        }
    }
}

impl CompatibleProvider {
    pub fn new(name: &str, default_base_url: &str, api_key: Option<&str>, custom_url: Option<&str>) -> Self {
        let base_url = custom_url
            .map(|u| u.trim_end_matches('/').to_string())
            .unwrap_or_else(|| default_base_url.trim_end_matches('/').to_string());

        let key = api_key
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(ToString::to_string)
            .or_else(|| match name {
                "DeepSeek" => std::env::var("DEEPSEEK_API_KEY").ok(),
                "Moonshot" | "Kimi" => std::env::var("MOONSHOT_API_KEY").ok(),
                "Qwen" => std::env::var("DASHSCOPE_API_KEY").ok(),
                _ => None,
            })
            .filter(|k| !k.is_empty());

        Self { name: name.to_string(), base_url, api_key: key }
    }

    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }

    fn chat_completions_url(&self) -> String {
        let has_endpoint = reqwest::Url::parse(&self.base_url)
            .map(|url| url.path().trim_end_matches('/').ends_with("/chat/completions"))
            .unwrap_or_else(|_| self.base_url.trim_end_matches('/').ends_with("/chat/completions"));

        if has_endpoint {
            self.base_url.clone()
        } else {
            format!("{}/chat/completions", self.base_url)
        }
    }

    async fn post(&self, request: &ChatRequest) -> anyhow::Result<String> {
        let key = self.api_key.as_ref().ok_or_else(|| {
            anyhow::anyhow!("{} API key not set. Set the appropriate env var or edit config.toml.", self.name)
        })?;

        let url = self.chat_completions_url();
        let response = self
            .client()
            .post(&url)
            .header("Authorization", format!("Bearer {key}"))
            .json(request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("{} API error ({status}): {body}", self.name);
        }

        let chat_response: ChatResponse = response.json().await?;
        chat_response
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.effective_content())
            .ok_or_else(|| anyhow::anyhow!("No response from {}", self.name))
    }
}

#[async_trait]
impl Provider for CompatibleProvider {
    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let mut messages = Vec::new();
        if let Some(sys) = system_prompt {
            messages.push(OpenAiMessage { role: "system".into(), content: sys.to_string() });
        }
        messages.push(OpenAiMessage { role: "user".into(), content: message.to_string() });

        let request = ChatRequest { model: model.to_string(), messages, temperature };
        self.post(&request).await
    }

    async fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let api_messages: Vec<OpenAiMessage> = messages
            .iter()
            .map(|m| OpenAiMessage { role: m.role.clone(), content: m.content.clone() })
            .collect();

        let request = ChatRequest { model: model.to_string(), messages: api_messages, temperature };
        self.post(&request).await
    }
}

use super::{BoxStream, ChatMessage, Provider, ProviderCapabilities, StreamChunk};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};

pub struct OpenAiProvider {
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

#[derive(Deserialize)]
struct StreamDelta {
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: Option<StreamMessageDelta>,
}

#[derive(Deserialize)]
struct StreamMessageDelta {
    content: Option<String>,
}

impl ResponseMessage {
    fn effective_content(&self) -> String {
        match &self.content {
            Some(c) if !c.is_empty() => c.clone(),
            _ => self.reasoning_content.clone().unwrap_or_default(),
        }
    }
}

impl OpenAiProvider {
    pub fn new(api_key: Option<&str>) -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: api_key.map(ToString::to_string),
        }
    }

    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = base_url.trim_end_matches('/').to_string();
        self
    }

    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }

    async fn post(&self, request: &ChatRequest) -> anyhow::Result<String> {
        let key = self.api_key.as_ref().ok_or_else(|| {
            anyhow::anyhow!("OpenAI API key not set. Set OPENAI_API_KEY or edit config.toml.")
        })?;

        let response = self
            .client()
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {key}"))
            .json(request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI API error ({status}): {body}");
        }

        let chat_response: ChatResponse = response.json().await?;
        chat_response
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.effective_content())
            .ok_or_else(|| anyhow::anyhow!("No response from OpenAI"))
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            native_tool_calling: false,
            streaming: true,
        }
    }

    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let mut messages = Vec::new();
        if let Some(sys) = system_prompt {
            messages.push(OpenAiMessage {
                role: "system".into(),
                content: sys.to_string(),
            });
        }
        messages.push(OpenAiMessage {
            role: "user".into(),
            content: message.to_string(),
        });

        let request = ChatRequest {
            model: model.to_string(),
            messages,
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
        let api_messages: Vec<OpenAiMessage> = messages
            .iter()
            .map(|m| OpenAiMessage {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect();

        let request = ChatRequest {
            model: model.to_string(),
            messages: api_messages,
            temperature,
        };
        self.post(&request).await
    }

    async fn warmup(&self) -> anyhow::Result<()> {
        if let Some(key) = self.api_key.as_ref() {
            let _ = self
                .client()
                .get(format!("{}/models", self.base_url))
                .header("Authorization", format!("Bearer {key}"))
                .send()
                .await?;
        }
        Ok(())
    }

    fn stream_chat(
        &self,
        messages: &[ChatMessage],
        model: &str,
        temperature: f64,
    ) -> BoxStream<'static, anyhow::Result<StreamChunk>> {
        let api_messages: Vec<OpenAiMessage> = messages
            .iter()
            .map(|m| OpenAiMessage {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect();

        let body = serde_json::json!({
            "model": model,
            "messages": api_messages,
            "temperature": temperature,
            "stream": true,
        });

        let key = self.api_key.clone();
        let url = format!("{}/chat/completions", self.base_url);
        let client = self.client();

        let stream = async_stream::stream! {
            let key = match key.as_ref() {
                Some(k) => k.clone(),
                None => {
                    yield Err(anyhow::anyhow!("OpenAI API key not set"));
                    return;
                }
            };

            let response = match client
                .post(&url)
                .header("Authorization", format!("Bearer {key}"))
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    yield Err(anyhow::anyhow!("stream request failed: {e}"));
                    return;
                }
            };

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                yield Err(anyhow::anyhow!("OpenAI stream error ({status}): {body}"));
                return;
            }

            let mut bytes = response.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk_result) = bytes.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        buffer.push_str(&String::from_utf8_lossy(&chunk));

                        while let Some(pos) = buffer.find('\n') {
                            let line = buffer[..pos].trim().to_string();
                            buffer = buffer[pos + 1..].to_string();

                            if line.is_empty() || !line.starts_with("data: ") {
                                continue;
                            }
                            let data = &line[6..];
                            if data == "[DONE]" {
                                yield Ok(StreamChunk::final_chunk());
                                return;
                            }

                            if let Ok(delta) = serde_json::from_str::<StreamDelta>(data) {
                                if let Some(choice) = delta.choices.first() {
                                    if let Some(msg) = &choice.delta {
                                        if let Some(content) = &msg.content {
                                            if !content.is_empty() {
                                                yield Ok(StreamChunk::delta(content));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        yield Err(anyhow::anyhow!("stream read error: {e}"));
                        return;
                    }
                }
            }

            yield Ok(StreamChunk::final_chunk());
        };

        Box::pin(stream)
    }
}

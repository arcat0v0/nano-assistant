use super::{BoxStream, ChatMessage, Provider, ProviderCapabilities, StreamChunk};
use crate::hub::HubClient;
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct HubProvider {
    client: HubClient,
}

#[derive(Serialize)]
struct ChatRequest {
    messages: Vec<OpenAiMessage>,
    model: String,
    stream: bool,
    temperature: f64,
}

#[derive(Serialize)]
struct OpenAiMessage {
    content: String,
    role: String,
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
            Some(content) if !content.is_empty() => content.clone(),
            _ => self.reasoning_content.clone().unwrap_or_default(),
        }
    }
}

impl HubProvider {
    pub fn new(client: HubClient) -> Self {
        Self { client }
    }

    async fn post(&self, request: &ChatRequest) -> anyhow::Result<String> {
        let response = self
            .client
            .signed_json(
                "POST",
                "/v1/chat/completions",
                serde_json::to_vec(request)?,
                true,
            )
            .await?;

        let chat_response: ChatResponse = response.json().await?;
        chat_response
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message.effective_content())
            .ok_or_else(|| anyhow::anyhow!("No response from hub"))
    }
}

#[async_trait]
impl Provider for HubProvider {
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
        if let Some(system_prompt) = system_prompt {
            messages.push(OpenAiMessage {
                content: system_prompt.to_string(),
                role: "system".to_string(),
            });
        }
        messages.push(OpenAiMessage {
            content: message.to_string(),
            role: "user".to_string(),
        });

        self.post(&ChatRequest {
            messages,
            model: model.to_string(),
            stream: false,
            temperature,
        })
        .await
    }

    async fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let request = ChatRequest {
            messages: messages
                .iter()
                .map(|message| OpenAiMessage {
                    content: message.content.clone(),
                    role: message.role.clone(),
                })
                .collect(),
            model: model.to_string(),
            stream: false,
            temperature,
        };

        self.post(&request).await
    }

    fn stream_chat(
        &self,
        messages: &[ChatMessage],
        model: &str,
        temperature: f64,
    ) -> BoxStream<'static, anyhow::Result<StreamChunk>> {
        let request = ChatRequest {
            messages: messages
                .iter()
                .map(|message| OpenAiMessage {
                    content: message.content.clone(),
                    role: message.role.clone(),
                })
                .collect(),
            model: model.to_string(),
            stream: true,
            temperature,
        };

        let client = self.client.clone();
        let stream = async_stream::stream! {
            let response = match client
                .signed_json(
                    "POST",
                    "/v1/chat/completions",
                    serde_json::to_vec(&request).unwrap_or_default(),
                    true,
                )
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    yield Err(error);
                    return;
                }
            };

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
                                    if let Some(message_delta) = &choice.delta {
                                        if let Some(content) = &message_delta.content {
                                            if !content.is_empty() {
                                                yield Ok(StreamChunk::delta(content));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(error) => {
                        yield Err(anyhow::anyhow!("hub stream read error: {error}"));
                        return;
                    }
                }
            }

            yield Ok(StreamChunk::final_chunk());
        };

        Box::pin(stream)
    }
}

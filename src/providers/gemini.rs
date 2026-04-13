use super::{ChatMessage, Provider};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub struct GeminiProvider {
    api_key: Option<String>,
    base_url: String,
}

#[derive(Serialize)]
struct GenerateContentRequest {
    contents: Vec<Content>,
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    system_instruction: Option<Content>,
    #[serde(rename = "generationConfig")]
    generation_config: GenerationConfig,
}

#[derive(Serialize, Clone)]
struct Content {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<Part>,
}

#[derive(Serialize, Clone)]
struct Part {
    text: String,
}

#[derive(Serialize, Clone)]
struct GenerationConfig {
    temperature: f64,
    #[serde(rename = "maxOutputTokens")]
    max_output_tokens: u32,
}

#[derive(Deserialize)]
struct GenerateContentResponse {
    candidates: Option<Vec<Candidate>>,
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct Candidate {
    content: Option<CandidateContent>,
}

#[derive(Deserialize)]
struct CandidateContent {
    parts: Vec<ResponsePart>,
}

#[derive(Deserialize)]
struct ResponsePart {
    text: Option<String>,
    thought: bool,
}

impl CandidateContent {
    fn effective_text(self) -> Option<String> {
        let mut answer_parts: Vec<String> = Vec::new();
        let mut first_thinking: Option<String> = None;

        for part in self.parts {
            if let Some(text) = part.text {
                if text.is_empty() {
                    continue;
                }
                if !part.thought {
                    answer_parts.push(text);
                } else if first_thinking.is_none() {
                    first_thinking = Some(text);
                }
            }
        }

        if answer_parts.is_empty() {
            first_thinking
        } else {
            Some(answer_parts.join(""))
        }
    }
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
}

impl GeminiProvider {
    pub fn new(api_key: Option<&str>) -> Self {
        let key = api_key
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(ToString::to_string)
            .or_else(|| {
                std::env::var("GEMINI_API_KEY")
                    .ok()
                    .filter(|k| !k.is_empty())
            });

        Self {
            api_key: key,
            base_url: "https://generativelanguage.googleapis.com/v1beta".to_string(),
        }
    }

    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }

    fn format_model(model: &str) -> String {
        if model.starts_with("models/") {
            model.to_string()
        } else {
            format!("models/{model}")
        }
    }

    async fn send(
        &self,
        contents: Vec<Content>,
        system_instruction: Option<Content>,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let key = self.api_key.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Gemini API key not set. Set GEMINI_API_KEY or edit config.toml.")
        })?;

        let request = GenerateContentRequest {
            contents,
            system_instruction,
            generation_config: GenerationConfig {
                temperature,
                max_output_tokens: 8192,
            },
        };

        let model_name = Self::format_model(model);
        let url = format!(
            "{}/{model_name}:generateContent?key={key}",
            self.base_url,
            model_name = model_name
        );

        let response = self.client().post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Gemini API error ({status}): {body}");
        }

        let result: GenerateContentResponse = response.json().await?;
        if let Some(err) = &result.error {
            anyhow::bail!("Gemini API error: {}", err.message);
        }

        result
            .candidates
            .and_then(|c| c.into_iter().next())
            .and_then(|c| c.content)
            .and_then(|c| c.effective_text())
            .ok_or_else(|| anyhow::anyhow!("No response from Gemini"))
    }
}

#[async_trait]
impl Provider for GeminiProvider {
    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let system = system_prompt.map(|s| Content {
            role: None,
            parts: vec![Part {
                text: s.to_string(),
            }],
        });
        let contents = vec![Content {
            role: Some("user".into()),
            parts: vec![Part {
                text: message.to_string(),
            }],
        }];
        self.send(contents, system, model, temperature).await
    }

    async fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let mut system_parts: Vec<&str> = Vec::new();
        let mut contents: Vec<Content> = Vec::new();

        for msg in messages {
            match msg.role.as_str() {
                "system" => system_parts.push(&msg.content),
                "user" => contents.push(Content {
                    role: Some("user".into()),
                    parts: vec![Part {
                        text: msg.content.clone(),
                    }],
                }),
                "assistant" => contents.push(Content {
                    role: Some("model".into()),
                    parts: vec![Part {
                        text: msg.content.clone(),
                    }],
                }),
                _ => {}
            }
        }

        let system = if system_parts.is_empty() {
            None
        } else {
            Some(Content {
                role: None,
                parts: vec![Part {
                    text: system_parts.join("\n\n"),
                }],
            })
        };

        self.send(contents, system, model, temperature).await
    }
}

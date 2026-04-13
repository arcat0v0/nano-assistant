use super::{ChatMessage, Provider};
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

pub struct GlmProvider {
    api_key_id: String,
    api_key_secret: String,
    base_url: String,
    token_cache: Mutex<Option<(String, u64)>>,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<GlmMessage>,
    temperature: f64,
}

#[derive(Serialize)]
struct GlmMessage {
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
    content: String,
}

fn base64url_encode(data: &[u8]) -> String {
    use base64::engine::{general_purpose::URL_SAFE_NO_PAD, Engine};
    URL_SAFE_NO_PAD.encode(data)
}

impl GlmProvider {
    pub fn new(api_key: Option<&str>) -> Self {
        let (id, secret) = api_key
            .and_then(|k| k.split_once('.'))
            .map(|(id, secret)| (id.to_string(), secret.to_string()))
            .unwrap_or_default();

        Self {
            api_key_id: id,
            api_key_secret: secret,
            base_url: "https://api.z.ai/api/paas/v4".to_string(),
            token_cache: Mutex::new(None),
        }
    }

    fn generate_token(&self) -> anyhow::Result<String> {
        if self.api_key_id.is_empty() || self.api_key_secret.is_empty() {
            anyhow::bail!("GLM API key not set or invalid format. Expected 'id.secret'. Set GLM_API_KEY env var.");
        }

        let now_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;

        if let Ok(cache) = self.token_cache.lock() {
            if let Some((ref token, expiry)) = *cache {
                if now_ms < expiry {
                    return Ok(token.clone());
                }
            }
        }

        let exp_ms = now_ms + 210_000;

        let header_json = r#"{"alg":"HS256","typ":"JWT","sign_type":"SIGN"}"#;
        let header_b64 = base64url_encode(header_json.as_bytes());
        let payload_json = format!(
            r#"{{"api_key":"{}","exp":{},"timestamp":{}}}"#,
            self.api_key_id, exp_ms, now_ms
        );
        let payload_b64 = base64url_encode(payload_json.as_bytes());
        let signing_input = format!("{header_b64}.{payload_b64}");

        let mut mac = HmacSha256::new_from_slice(self.api_key_secret.as_bytes())?;
        mac.update(signing_input.as_bytes());
        let result = mac.finalize();
        let sig_b64 = base64url_encode(result.into_bytes().as_slice());

        let token = format!("{signing_input}.{sig_b64}");

        if let Ok(mut cache) = self.token_cache.lock() {
            *cache = Some((token.clone(), now_ms + 180_000));
        }

        Ok(token)
    }

    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }

    async fn post(&self, request: &ChatRequest) -> anyhow::Result<String> {
        let token = self.generate_token()?;

        let response = self
            .client()
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {token}"))
            .json(request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error = response.text().await.unwrap_or_default();
            anyhow::bail!("GLM API error: {error}");
        }

        let chat_response: ChatResponse = response.json().await?;
        chat_response
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| anyhow::anyhow!("No response from GLM"))
    }
}

#[async_trait]
impl Provider for GlmProvider {
    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let mut messages = Vec::new();
        if let Some(sys) = system_prompt {
            messages.push(GlmMessage {
                role: "system".into(),
                content: sys.to_string(),
            });
        }
        messages.push(GlmMessage {
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
        let api_messages: Vec<GlmMessage> = messages
            .iter()
            .map(|m| GlmMessage {
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
}

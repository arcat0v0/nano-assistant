use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) struct GlmAuth {
    id: String,
    secret: String,
    cached: Mutex<Option<(String, u64)>>,
}

impl GlmAuth {
    pub(super) fn new(key: &str) -> anyhow::Result<Self> {
        let (id, secret) = key.split_once('.').unwrap_or_default();
        if id.is_empty() || secret.is_empty() {
            anyhow::bail!("GLM API key not set or invalid format. Expected 'id.secret'. Set GLM_API_KEY env var.");
        }
        Ok(Self {
            id: id.to_owned(),
            secret: secret.to_owned(),
            cached: Mutex::new(None),
        })
    }

    pub(super) fn token(&self) -> anyhow::Result<String> {
        let now_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
        let mut cached = self
            .cached
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((token, expiry)) = cached.as_ref() {
            if now_ms < *expiry {
                return Ok(token.clone());
            }
        }

        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"HS256","typ":"JWT","sign_type":"SIGN"}"#);
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&serde_json::json!({
            "api_key": self.id,
            "exp": now_ms + 210_000,
            "timestamp": now_ms,
        }))?);
        let signing_input = format!("{header}.{payload}");
        let mut mac = Hmac::<Sha256>::new_from_slice(self.secret.as_bytes())?;
        mac.update(signing_input.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        let token = format!("{signing_input}.{signature}");
        *cached = Some((token.clone(), now_ms + 180_000));
        Ok(token)
    }
}

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use bytes::Bytes;
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::config::{load_config_or_default, save_config, Config};

const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const IDENTITY_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct ResolvedHubConfig {
    pub ad_display: String,
    pub auto_register: bool,
    pub enabled: bool,
    pub identity_path: PathBuf,
    pub machine_id: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HubIdentityDocument {
    pub public_key_base64: String,
    pub seed_base64: String,
    pub version: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HubIdentityExport {
    pub identity: HubIdentityDocument,
    pub machine_id: Option<String>,
    pub version: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubAd {
    pub body_md: String,
    pub id: String,
    pub image_url: Option<String>,
    pub placement: String,
    pub target_url: Option<String>,
    pub title: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubAdResponse {
    pub ad: Option<HubAd>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubQuotaUsage {
    pub concurrent_current: u64,
    pub daily_remaining: u64,
    pub daily_used: u64,
    pub rpm_current: u64,
    pub tpm_current: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubQuotaLimits {
    pub concurrent: u64,
    pub daily: u64,
    pub rpm: u64,
    pub tpm: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubQuotaResetAt {
    pub daily: u64,
    pub rpm: u64,
    pub tpm: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubQuotaResponse {
    pub client_version: String,
    pub limits: HubQuotaLimits,
    pub machine_id: String,
    pub reset_at: HubQuotaResetAt,
    pub status: String,
    pub usage: HubQuotaUsage,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubRegisterResponse {
    pub issued_at: u64,
    pub machine_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubApiErrorEnvelope {
    pub error: HubApiErrorBody,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubApiErrorBody {
    pub code: String,
    pub message: String,
    pub nana: Option<Value>,
    pub param: Option<String>,
    #[allow(dead_code)]
    pub r#type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HubApiError {
    pub code: String,
    pub message: String,
    pub nana: Option<Value>,
    pub param: Option<String>,
    pub status: u16,
}

impl HubApiError {
    pub fn captcha_url(&self) -> Option<&str> {
        self.nana
            .as_ref()
            .and_then(|value| value.get("captcha_url"))
            .and_then(Value::as_str)
    }
}

impl fmt::Display for HubApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "hub error {} ({}): {}",
            self.code, self.status, self.message
        )?;

        if let Some(captcha_url) = self.captcha_url() {
            write!(f, " [{captcha_url}]")?;
        }

        Ok(())
    }
}

impl std::error::Error for HubApiError {}

#[derive(Clone)]
pub struct HubClient {
    client: reqwest::Client,
    config_path: PathBuf,
    state: Arc<Mutex<HubState>>,
}

#[derive(Clone)]
struct HubState {
    config: Config,
    resolved: ResolvedHubConfig,
}

impl HubClient {
    pub fn new(config_path: PathBuf, config: Config) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(
                config.provider.timeout_secs.max(30),
            ))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Ok(Self {
            client,
            config_path,
            state: Arc::new(Mutex::new(HubState {
                resolved: resolve_hub_config(&config),
                config,
            })),
        })
    }

    pub async fn register_machine(&self, force_new: bool) -> anyhow::Result<HubRegisterResponse> {
        let identity = self.ensure_identity().await?;
        let (url, machine_id_before) = {
            let state = self.state.lock().await;
            (
                join_url(&state.resolved.url, "/register"),
                state.resolved.machine_id.clone(),
            )
        };

        if force_new && machine_id_before.is_some() {
            self.update_machine_id(None).await?;
        }

        let body = serde_json::json!({
            "fingerprint": build_machine_fingerprint().await?,
            "pubkey": identity.public_key_base64,
        });

        let response = self
            .client
            .post(url)
            .header("content-type", "application/json")
            .header("x-client-version", CLIENT_VERSION)
            .body(serde_json::to_vec(&body)?)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(parse_api_error(response).await.into());
        }

        let payload = response.json::<HubRegisterResponse>().await?;
        self.update_machine_id(Some(payload.machine_id.clone()))
            .await?;
        Ok(payload)
    }

    pub async fn disable(&self) -> anyhow::Result<()> {
        let mut state = self.state.lock().await;
        state.config.hub.enabled = false;
        state.resolved.enabled = false;
        save_config(&self.config_path, &state.config)
    }

    pub async fn export_identity(&self, target_path: &Path) -> anyhow::Result<HubIdentityExport> {
        let identity = self.ensure_identity().await?;
        let machine_id = {
            let state = self.state.lock().await;
            state.resolved.machine_id.clone()
        };

        let export = HubIdentityExport {
            identity,
            machine_id,
            version: IDENTITY_VERSION,
        };

        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(target_path, serde_json::to_vec_pretty(&export)?)?;
        Ok(export)
    }

    pub async fn fetch_ad(&self, placement: &str) -> anyhow::Result<Option<HubAd>> {
        let snapshot = self.snapshot().await;
        if !snapshot.resolved.enabled || !model_routes_via_hub(&snapshot.config) {
            return Ok(None);
        }

        if snapshot.resolved.ad_display == "none" {
            return Ok(None);
        }

        let mut url = reqwest::Url::parse(&join_url(&snapshot.resolved.url, "/public/ads/fetch"))?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("placement", placement);
            if let Some(machine_id) = snapshot.resolved.machine_id.as_ref() {
                pairs.append_pair("machine_id", machine_id);
            }
        }

        let response = self.client.get(url).send().await?;
        if !response.status().is_success() {
            return Err(parse_api_error(response).await.into());
        }

        Ok(response.json::<HubAdResponse>().await?.ad)
    }

    pub async fn import_identity(&self, source_path: &Path) -> anyhow::Result<HubIdentityExport> {
        let raw = std::fs::read(source_path)?;
        let export: HubIdentityExport = serde_json::from_slice(&raw)?;
        write_identity_document(
            &self.resolved_config().await.identity_path,
            &export.identity,
        )?;

        if let Some(machine_id) = export.machine_id.clone() {
            self.update_machine_id(Some(machine_id)).await?;
        }

        Ok(export)
    }

    pub async fn quota_status(&self) -> anyhow::Result<HubQuotaResponse> {
        let response = self
            .send_signed("GET", "/v1/quota", Bytes::new(), None, true)
            .await?;
        Ok(response.json::<HubQuotaResponse>().await?)
    }

    pub async fn resolved_config(&self) -> ResolvedHubConfig {
        self.state.lock().await.resolved.clone()
    }

    async fn snapshot(&self) -> HubState {
        self.state.lock().await.clone()
    }

    pub async fn should_render_ads(&self) -> bool {
        let state = self.state.lock().await;
        state.resolved.enabled
            && state.resolved.ad_display != "none"
            && model_routes_via_hub(&state.config)
    }

    pub async fn signed_bytes(
        &self,
        method: &str,
        path: &str,
        body: Bytes,
        retry_on_registration_error: bool,
    ) -> anyhow::Result<reqwest::Response> {
        self.send_signed(
            method,
            path,
            body,
            Some("application/json"),
            retry_on_registration_error,
        )
        .await
    }

    async fn ensure_identity(&self) -> anyhow::Result<HubIdentityDocument> {
        let identity_path = self.resolved_config().await.identity_path;

        if identity_path.exists() {
            return read_identity_document(&identity_path);
        }

        let signing_key = SigningKey::generate(&mut rand_core::OsRng);
        let identity = HubIdentityDocument {
            public_key_base64: STANDARD.encode(signing_key.verifying_key().to_bytes()),
            seed_base64: STANDARD.encode(signing_key.to_bytes()),
            version: IDENTITY_VERSION,
        };
        write_identity_document(&identity_path, &identity)?;
        Ok(identity)
    }

    async fn ensure_machine_id(&self) -> anyhow::Result<String> {
        {
            let state = self.state.lock().await;
            if let Some(machine_id) = state.resolved.machine_id.clone() {
                return Ok(machine_id);
            }

            if !state.resolved.auto_register {
                anyhow::bail!(
                    "hub machine is not registered and auto_register is disabled; run `na hub register`"
                );
            }
        }

        Ok(self.register_machine(false).await?.machine_id)
    }

    async fn send_signed(
        &self,
        method: &str,
        path: &str,
        body: Bytes,
        content_type: Option<&str>,
        retry_on_registration_error: bool,
    ) -> anyhow::Result<reqwest::Response> {
        let response = self
            .send_signed_once(method, path, body.clone(), content_type)
            .await;

        match response {
            Ok(response) => Ok(response),
            Err(error)
                if retry_on_registration_error
                    && matches!(
                        error.code.as_str(),
                        "machine_not_registered" | "signature_invalid"
                    ) =>
            {
                self.register_machine(true).await?;
                self.send_signed_once(method, path, body, content_type)
                    .await
                    .map_err(anyhow::Error::new)
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn send_signed_once(
        &self,
        method: &str,
        path: &str,
        body: Bytes,
        content_type: Option<&str>,
    ) -> Result<reqwest::Response, HubApiError> {
        let resolved = self.resolved_config().await;
        if !resolved.enabled {
            return Err(HubApiError {
                code: "hub_disabled".to_string(),
                message: "Hub is disabled in config.".to_string(),
                nana: None,
                param: None,
                status: 400,
            });
        }

        let identity = self.ensure_identity().await.map_err(|error| HubApiError {
            code: "identity_error".to_string(),
            message: error.to_string(),
            nana: None,
            param: None,
            status: 500,
        })?;
        let machine_id = self
            .ensure_machine_id()
            .await
            .map_err(|error| HubApiError {
                code: "registration_error".to_string(),
                message: error.to_string(),
                nana: None,
                param: None,
                status: 500,
            })?;
        let timestamp = current_unix_ms().to_string();
        let nonce = Uuid::new_v4().simple().to_string();
        let signature = build_signature(
            method,
            path,
            &machine_id,
            &timestamp,
            &nonce,
            &body,
            &identity,
        )
        .map_err(|error| HubApiError {
            code: "signature_error".to_string(),
            message: error.to_string(),
            nana: None,
            param: None,
            status: 500,
        })?;

        let url = join_url(&resolved.url, path);
        let mut request = self
            .client
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET),
                url,
            )
            .header("x-client-version", CLIENT_VERSION)
            .header("x-machine-id", machine_id)
            .header("x-nonce", nonce)
            .header("x-signature", signature)
            .header("x-timestamp", timestamp)
            .body(body);

        if let Some(content_type) = content_type {
            request = request.header("content-type", content_type);
        }

        let response = request.send().await.map_err(|error| HubApiError {
            code: "transport_error".to_string(),
            message: error.to_string(),
            nana: None,
            param: None,
            status: 502,
        })?;

        if !response.status().is_success() {
            return Err(parse_api_error(response).await);
        }

        Ok(response)
    }

    async fn update_machine_id(&self, machine_id: Option<String>) -> anyhow::Result<()> {
        let mut state = self.state.lock().await;
        state.config.hub.machine_id = machine_id.clone();
        state.resolved.machine_id = machine_id;
        save_config(&self.config_path, &state.config)
    }
}

pub fn build_signature(
    method: &str,
    path_with_query: &str,
    machine_id: &str,
    timestamp: &str,
    nonce: &str,
    body: &[u8],
    identity: &HubIdentityDocument,
) -> anyhow::Result<String> {
    let seed = STANDARD
        .decode(identity.seed_base64.as_bytes())
        .context("invalid hub identity seed encoding")?;
    let signing_key = SigningKey::from_bytes(
        &seed
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid hub identity seed length"))?,
    );
    let digest =
        canonical_payload_digest(method, path_with_query, machine_id, timestamp, nonce, body);
    let signature = signing_key.sign(&digest);
    Ok(STANDARD.encode(signature.to_bytes()))
}

pub fn canonical_payload_digest(
    method: &str,
    path_with_query: &str,
    machine_id: &str,
    timestamp: &str,
    nonce: &str,
    body: &[u8],
) -> Vec<u8> {
    let body_hash = hex_sha256(body);
    let canonical = [
        method.to_uppercase(),
        path_with_query.to_string(),
        machine_id.to_string(),
        timestamp.to_string(),
        nonce.to_string(),
        body_hash,
    ]
    .join("\n");

    Sha256::digest(canonical.as_bytes()).to_vec()
}

pub async fn maybe_render_ad(config_path: PathBuf, placement: &str) -> anyhow::Result<()> {
    let config = load_config_or_default(&config_path);
    let client = HubClient::new(config_path, config)?;
    if !client.should_render_ads().await {
        return Ok(());
    }

    let Some(ad) = client.fetch_ad(placement).await? else {
        return Ok(());
    };

    let mode = client.resolved_config().await.ad_display;
    render_ad(&ad, &mode);
    Ok(())
}

pub fn model_routes_via_hub(config: &Config) -> bool {
    let resolved = resolve_hub_config(config);
    resolved.enabled
        && config
            .provider
            .model
            .as_deref()
            .is_some_and(|model| model.starts_with("free/"))
}

pub fn resolve_hub_config(config: &Config) -> ResolvedHubConfig {
    let url = std::env::var("NANA_HUB_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| config.hub.url.clone());
    let disabled_by_env = std::env::var("NANA_HUB_DISABLED")
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE"));
    let identity_path = std::env::var("NANA_IDENTITY_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| crate::platform::current_platform().expand_tilde(&value))
        .unwrap_or_else(|| {
            crate::platform::current_platform().expand_tilde(&config.hub.identity_path)
        });

    ResolvedHubConfig {
        ad_display: config.hub.ad_display.clone(),
        auto_register: config.hub.auto_register,
        enabled: config.hub.enabled && !disabled_by_env,
        identity_path,
        machine_id: config.hub.machine_id.clone(),
        url: url.trim_end_matches('/').to_string(),
    }
}

pub fn render_ad(ad: &HubAd, mode: &str) {
    match mode {
        "none" => {}
        "minimal" => {
            println!();
            println!("\x1b[2m[sponsored]\x1b[0m {}", ad.title);
            if let Some(target_url) = ad.target_url.as_ref() {
                println!("\x1b[2m{target_url}\x1b[0m");
            }
            println!();
        }
        _ => {
            println!();
            println!("\x1b[1;33mSponsored\x1b[0m");
            println!("\x1b[1m{}\x1b[0m", ad.title);
            crate::render::render_markdown_to_stdout(&ad.body_md);
            println!();
            if let Some(target_url) = ad.target_url.as_ref() {
                println!("\x1b[2m{target_url}\x1b[0m");
            }
            println!();
        }
    }
}

pub fn write_identity_document(path: &Path, identity: &HubIdentityDocument) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(path, serde_json::to_vec_pretty(identity)?)?;
    Ok(())
}

fn build_machine_fingerprint() -> impl std::future::Future<Output = anyhow::Result<String>> {
    async {
        let info = crate::system_info::detect().await;
        let source = format!(
            "{}|{}|{}|{}|{}",
            info.hostname, info.username, info.os_name, info.os_version, info.architecture
        );
        Ok(hex_sha256(source.as_bytes()))
    }
}

fn current_unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn join_url(base_url: &str, path: &str) -> String {
    format!("{}{}", base_url.trim_end_matches('/'), path)
}

async fn parse_api_error(response: reqwest::Response) -> HubApiError {
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();

    if let Ok(parsed) = serde_json::from_str::<HubApiErrorEnvelope>(&body) {
        return HubApiError {
            code: parsed.error.code,
            message: parsed.error.message,
            nana: parsed.error.nana,
            param: parsed.error.param,
            status,
        };
    }

    HubApiError {
        code: "http_error".to_string(),
        message: if body.trim().is_empty() {
            format!("request failed with status {status}")
        } else {
            body
        },
        nana: None,
        param: None,
        status,
    }
}

fn read_identity_document(path: &Path) -> anyhow::Result<HubIdentityDocument> {
    let raw = std::fs::read(path)?;
    let identity: HubIdentityDocument = serde_json::from_slice(&raw)?;
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn canonical_digest_changes_with_nonce() {
        let left = canonical_payload_digest(
            "POST",
            "/v1/chat/completions",
            "machine-1",
            "1",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            br#"{"hello":"world"}"#,
        );
        let right = canonical_payload_digest(
            "POST",
            "/v1/chat/completions",
            "machine-1",
            "1",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            br#"{"hello":"world"}"#,
        );

        assert_ne!(left, right);
    }

    #[test]
    #[serial(hub_env)]
    fn model_routes_via_hub_only_for_free_models() {
        std::env::remove_var("NANA_HUB_DISABLED");
        let mut config = Config::default();
        config.provider.model = Some("free/mock-chat".to_string());
        assert!(model_routes_via_hub(&config));

        config.provider.model = Some("gpt-4o-mini".to_string());
        assert!(!model_routes_via_hub(&config));
    }

    #[test]
    #[serial(hub_env)]
    fn env_can_disable_hub() {
        let config = Config::default();
        std::env::set_var("NANA_HUB_DISABLED", "1");
        let resolved = resolve_hub_config(&config);
        std::env::remove_var("NANA_HUB_DISABLED");
        assert!(!resolved.enabled);
    }
}

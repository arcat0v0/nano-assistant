//! Configuration schema for nano-assistant.
//!
//! Config file location: ~/.config/nano-assistant/config.toml
//!
//! Priority: CLI flag > environment variable > config file

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

/// Top-level configuration loaded from `config.toml`.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Config {
    /// Provider configuration (API key, model selection, etc.)
    #[serde(default)]
    pub provider: ProviderConfig,

    /// In-memory credential for one selected provider; never serialized.
    #[serde(skip)]
    pub runtime_api_key: Option<RuntimeApiKey>,

    #[serde(default, skip_serializing_if = "ModelsConfig::is_empty")]
    pub models: ModelsConfig,

    #[serde(skip)]
    pub active_profile: Option<String>,

    /// Memory backend configuration.
    #[serde(default)]
    pub memory: MemoryConfig,

    /// Security and access control configuration.
    #[serde(default)]
    pub security: SecurityConfig,

    /// Behavior configuration (autonomy level, tool restrictions, etc.).
    #[serde(default)]
    pub behavior: BehaviorConfig,

    /// Skills configuration.
    #[serde(default)]
    pub skills: SkillsConfig,

    /// MCP server configuration.
    #[serde(default)]
    pub mcp: McpConfig,

    /// Hub configuration for `free/*` models and ad delivery.
    #[serde(default)]
    pub hub: HubConfig,
}

#[derive(Clone)]
pub struct RuntimeApiKey {
    provider: String,
    secret: String,
}

impl RuntimeApiKey {
    pub fn new(provider: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            secret: secret.into(),
        }
    }

    pub fn for_provider(&self, provider: &str) -> Option<&str> {
        (self.provider == provider).then_some(self.secret.as_str())
    }
}

impl fmt::Debug for RuntimeApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeApiKey")
            .field("provider", &self.provider)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ModelsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, ModelProfile>,
}

impl ModelsConfig {
    fn is_empty(&self) -> bool {
        self.default.is_none() && self.profiles.is_empty()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelProfile {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderConfig {
    /// API key for the selected provider.
    /// Can be overridden by NA_API_KEY environment variable.
    #[serde(default)]
    pub api_key: Option<String>,

    /// Provider name (e.g., "openai", "anthropic", "ollama").
    /// Can be overridden by NA_PROVIDER environment variable.
    #[serde(default)]
    pub provider: Option<String>,

    /// Model name (e.g., "gpt-4", "claude-3-sonnet").
    /// Can be overridden by NA_MODEL environment variable.
    #[serde(default)]
    pub model: Option<String>,

    /// Base URL override for provider API.
    /// Useful for local models or custom endpoints.
    #[serde(default)]
    pub api_url: Option<String>,

    /// Request timeout in seconds. Default: 120.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    /// Default temperature (0.0-2.0). Default: 0.7.
    #[serde(default = "default_temperature")]
    pub temperature: f64,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            provider: Some("openai".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            api_url: None,
            timeout_secs: default_timeout(),
            temperature: default_temperature(),
        }
    }
}

pub(super) fn default_timeout() -> u64 {
    120
}

pub(super) fn default_temperature() -> f64 {
    0.7
}

/// Memory backend configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemoryConfig {
    /// Enable conversation memory. Default: true.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Maximum number of conversation messages to retain. Default: 100.
    #[serde(default = "default_max_messages")]
    pub max_messages: usize,

    /// Enable embeddings for semantic search. Default: false.
    #[serde(default)]
    pub embeddings_enabled: bool,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_messages: default_max_messages(),
            embeddings_enabled: false,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_max_messages() -> usize {
    100
}

/// Security configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SecurityConfig {
    /// Autonomy level: "manual", "review", "auto". Default: "review".
    #[serde(default = "default_autonomy_level")]
    pub autonomy_level: String,

    /// Allowed tools (empty means all allowed). Default: empty (all allowed).
    #[serde(default)]
    pub allowed_tools: Vec<String>,

    /// Blocked tools. Default: empty (none blocked).
    #[serde(default)]
    pub blocked_tools: Vec<String>,

    /// Security enforcement mode: "direct", "confirm", "whitelist". Default: "direct".
    #[serde(default = "default_security_mode")]
    pub mode: String,

    /// Whitelist of allowed commands for whitelist mode. Supports glob wildcards.
    /// Example: ["ls", "cat", "docker *", "systemctl status *"]
    #[serde(default)]
    pub whitelist: Vec<String>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            autonomy_level: default_autonomy_level(),
            allowed_tools: Vec::new(),
            blocked_tools: Vec::new(),
            mode: default_security_mode(),
            whitelist: Vec::new(),
        }
    }
}

fn default_security_mode() -> String {
    "direct".to_string()
}

fn default_autonomy_level() -> String {
    "review".to_string()
}

/// Behavior configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BehaviorConfig {
    /// Maximum tool-call iterations per user message. Default: 10.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,

    /// Enable debug diagnostics to stderr. Default: false.
    #[serde(default)]
    pub debug: bool,

    /// Verbose error messages. Default: true.
    #[serde(default = "default_true")]
    pub verbose_errors: bool,

    /// Enable inline tool explanations. Default: true.
    #[serde(default = "default_true")]
    pub explain_tools: bool,

    /// Enable streaming output (real-time LLM response display). Default: true.
    #[serde(default = "default_true")]
    pub streaming: bool,
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            max_iterations: default_max_iterations(),
            debug: false,
            verbose_errors: true,
            explain_tools: true,
            streaming: true,
        }
    }
}

fn default_max_iterations() -> usize {
    10
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default)]
    pub allow_scripts: bool,

    #[serde(default)]
    pub skills_dir: Option<String>,

    /// Additional directories to scan for skills.
    /// `~/.agents/skills` is always scanned automatically (hardcoded default).
    /// Use this for custom paths beyond the defaults.
    #[serde(default)]
    pub extra_paths: Vec<String>,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_scripts: false,
            skills_dir: None,
            extra_paths: Vec::new(),
        }
    }
}

/// MCP transport protocol.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    #[default]
    Stdio,
    Http,
    Sse,
}

/// Configuration for a single MCP server.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct McpServerConfig {
    /// Server name, used as tool name prefix.
    pub name: String,

    /// Transport protocol. Default: Stdio.
    #[serde(default)]
    pub transport: McpTransport,

    /// URL for HTTP/SSE transports.
    #[serde(default)]
    pub url: Option<String>,

    /// Command to spawn for Stdio transport.
    #[serde(default)]
    pub command: String,

    /// Arguments for the Stdio command.
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment variables for Stdio command.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,

    /// HTTP headers for HTTP/SSE transports.
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,

    /// Per-tool call timeout in seconds. Default: 180, max: 600.
    #[serde(default)]
    pub tool_timeout_secs: Option<u64>,
}

/// MCP (Model Context Protocol) configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpConfig {
    /// Enable MCP server connections. Default: false.
    #[serde(default)]
    pub enabled: bool,

    /// Use deferred loading. Default: true.
    #[serde(default = "default_true")]
    pub deferred_loading: bool,

    /// MCP server definitions.
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            deferred_loading: true,
            servers: Vec::new(),
        }
    }
}

/// Hub routing configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HubConfig {
    /// Hub base URL for `free/*` models.
    #[serde(default = "default_hub_url")]
    pub url: String,

    /// Whether hub routing is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Registered machine identifier assigned by the hub.
    #[serde(default)]
    pub machine_id: Option<String>,

    /// Local identity file path.
    #[serde(default = "default_identity_path")]
    pub identity_path: String,

    /// Whether the client may auto-register on first use.
    #[serde(default = "default_true")]
    pub auto_register: bool,

    /// Ad display mode: inline | banner | minimal | none.
    #[serde(default = "default_ad_display")]
    pub ad_display: String,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            url: default_hub_url(),
            enabled: true,
            machine_id: None,
            identity_path: default_identity_path(),
            auto_register: true,
            ad_display: default_ad_display(),
        }
    }
}

fn default_hub_url() -> String {
    "https://hub.nana.dev".to_string()
}

fn default_identity_path() -> String {
    crate::platform::current_platform()
        .config_dir()
        .join("identity.key")
        .to_string_lossy()
        .to_string()
}

fn default_ad_display() -> String {
    "inline".to_string()
}

/// Environment variable prefix for configuration overrides.
pub const ENV_PREFIX: &str = "NA";

/// Get the default config file path: ~/.config/nano-assistant/config.toml
pub fn default_config_path() -> PathBuf {
    crate::platform::current_platform().config_path()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_verifies_all_nested_defaults() {
        let config = Config::default();
        assert_eq!(config.provider.provider, Some("openai".to_string()));
        assert_eq!(config.provider.model, Some("gpt-4o-mini".to_string()));
        assert_eq!(config.provider.temperature, 0.7);
        assert_eq!(config.provider.timeout_secs, 120);
        assert!(config.provider.api_key.is_none());
        assert!(config.provider.api_url.is_none());
        assert!(config.memory.enabled);
        assert_eq!(config.memory.max_messages, 100);
        assert!(!config.memory.embeddings_enabled);
        assert_eq!(config.security.mode, "direct");
        assert!(config.security.whitelist.is_empty());
        assert_eq!(config.security.autonomy_level, "review");
        assert!(config.security.allowed_tools.is_empty());
        assert!(config.security.blocked_tools.is_empty());
        assert_eq!(config.behavior.max_iterations, 10);
        assert!(config.behavior.streaming);
        assert!(config.behavior.verbose_errors);
        assert!(config.behavior.explain_tools);
        assert!(config.skills.enabled);
        assert!(!config.skills.allow_scripts);
        assert!(config.skills.skills_dir.is_none());
        assert_eq!(config.hub.url, "https://hub.nana.dev");
        assert!(config.hub.enabled);
        assert!(config.hub.machine_id.is_none());
        assert!(config.hub.auto_register);
        assert_eq!(config.hub.ad_display, "inline");
        assert!(config.hub.identity_path.ends_with("identity.key"));
    }

    #[test]
    fn provider_config_default() {
        let p = ProviderConfig::default();
        assert_eq!(p.provider, Some("openai".to_string()));
        assert_eq!(p.model, Some("gpt-4o-mini".to_string()));
        assert_eq!(p.temperature, 0.7);
        assert_eq!(p.timeout_secs, 120);
        assert!(p.api_key.is_none());
        assert!(p.api_url.is_none());
    }

    #[test]
    fn memory_config_default() {
        let m = MemoryConfig::default();
        assert!(m.enabled);
        assert_eq!(m.max_messages, 100);
        assert!(!m.embeddings_enabled);
    }

    #[test]
    fn security_config_default() {
        let s = SecurityConfig::default();
        assert_eq!(s.mode, "direct");
        assert!(s.whitelist.is_empty());
        assert_eq!(s.autonomy_level, "review");
        assert!(s.allowed_tools.is_empty());
        assert!(s.blocked_tools.is_empty());
    }

    #[test]
    fn behavior_config_default() {
        let b = BehaviorConfig::default();
        assert_eq!(b.max_iterations, 10);
        assert!(b.streaming);
        assert!(b.verbose_errors);
        assert!(b.explain_tools);
    }

    #[test]
    fn hub_config_default() {
        let hub = HubConfig::default();
        assert_eq!(hub.url, "https://hub.nana.dev");
        assert!(hub.enabled);
        assert!(hub.machine_id.is_none());
        assert!(hub.auto_register);
        assert_eq!(hub.ad_display, "inline");
        assert!(hub.identity_path.ends_with("identity.key"));
    }

    #[test]
    fn default_config_path_contains_expected_segments() {
        let path = default_config_path();
        let path_str = path.to_string_lossy();
        assert!(
            path_str.contains(".config/nano-assistant/config.toml"),
            "path was: {path_str}"
        );
    }

    #[test]
    fn toml_deserialization_full_config() {
        let toml_str = r#"
            [provider]
            provider = "anthropic"
            model = "claude-3-sonnet"
            api_key = "sk-test-123"
            temperature = 0.5
            timeout_secs = 60

            [memory]
            enabled = false
            max_messages = 50

            [security]
            mode = "confirm"
            whitelist = ["ls", "cat"]

            [behavior]
            max_iterations = 5
            streaming = false

            [skills]
            enabled = false
            allow_scripts = true
            skills_dir = "/custom/skills"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.provider.provider, Some("anthropic".to_string()));
        assert_eq!(config.provider.model, Some("claude-3-sonnet".to_string()));
        assert_eq!(config.provider.api_key, Some("sk-test-123".to_string()));
        assert_eq!(config.provider.temperature, 0.5);
        assert_eq!(config.provider.timeout_secs, 60);
        assert!(!config.memory.enabled);
        assert_eq!(config.memory.max_messages, 50);
        assert_eq!(config.security.mode, "confirm");
        assert_eq!(config.security.whitelist, vec!["ls", "cat"]);
        assert_eq!(config.behavior.max_iterations, 5);
        assert!(!config.behavior.streaming);
        assert!(!config.skills.enabled);
        assert!(config.skills.allow_scripts);
        assert_eq!(config.skills.skills_dir.as_deref(), Some("/custom/skills"));
    }

    #[test]
    fn toml_deserialization_partial_config_merges_with_defaults() {
        let toml_str = r#"
            [provider]
            provider = "gemini"
            temperature = 0.9

            [behavior]
            max_iterations = 3
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.provider.provider, Some("gemini".to_string()));
        assert_eq!(config.provider.temperature, 0.9);
        assert_eq!(config.behavior.max_iterations, 3);
        assert!(config.provider.model.is_none());
        assert_eq!(config.provider.timeout_secs, 120);
        assert!(config.provider.api_key.is_none());
        assert!(config.provider.api_url.is_none());
        assert!(config.memory.enabled);
        assert_eq!(config.memory.max_messages, 100);
        assert!(!config.memory.embeddings_enabled);
        assert_eq!(config.security.mode, "direct");
        assert!(config.security.whitelist.is_empty());
        assert!(config.behavior.streaming);
    }

    #[test]
    fn toml_deserialization_empty_config_gives_all_defaults() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.provider.provider, Some("openai".to_string()));
        assert_eq!(config.provider.model, Some("gpt-4o-mini".to_string()));
        assert_eq!(config.provider.temperature, 0.7);
        assert_eq!(config.provider.timeout_secs, 120);
        assert!(config.provider.api_key.is_none());
        assert!(config.provider.api_url.is_none());
        assert!(config.memory.enabled);
        assert_eq!(config.memory.max_messages, 100);
        assert!(!config.memory.embeddings_enabled);
        assert_eq!(config.security.mode, "direct");
        assert!(config.security.whitelist.is_empty());
        assert_eq!(config.security.autonomy_level, "review");
        assert!(config.security.allowed_tools.is_empty());
        assert!(config.security.blocked_tools.is_empty());
        assert_eq!(config.behavior.max_iterations, 10);
        assert!(config.behavior.streaming);
        assert!(config.behavior.verbose_errors);
        assert!(config.behavior.explain_tools);
        assert!(config.skills.enabled);
        assert!(!config.skills.allow_scripts);
        assert!(config.skills.skills_dir.is_none());
    }

    #[test]
    fn skills_config_default() {
        let s = SkillsConfig::default();
        assert!(s.enabled);
        assert!(!s.allow_scripts);
        assert!(s.skills_dir.is_none());
    }

    #[test]
    fn skills_config_extra_paths_default_empty() {
        let s = SkillsConfig::default();
        assert!(s.extra_paths.is_empty());
    }

    #[test]
    fn toml_deserialization_skills_extra_paths() {
        let toml_str = r#"
            [skills]
            extra_paths = ["/opt/my-skills", "~/.agents/skills"]
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.skills.extra_paths,
            vec!["/opt/my-skills", "~/.agents/skills"]
        );
    }

    #[test]
    fn mcp_config_default() {
        let m = McpConfig::default();
        assert!(!m.enabled);
        assert!(m.deferred_loading);
        assert!(m.servers.is_empty());
    }

    #[test]
    fn mcp_transport_default_is_stdio() {
        let t = McpTransport::default();
        assert_eq!(t, McpTransport::Stdio);
    }

    #[test]
    fn toml_deserialization_mcp_config() {
        let toml_str = r#"
            [mcp]
            enabled = true

            [[mcp.servers]]
            name = "context7"
            command = "npx"
            args = ["-y", "@upstash/context7-mcp@latest"]
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.mcp.enabled);
        assert_eq!(config.mcp.servers.len(), 1);
        assert_eq!(config.mcp.servers[0].name, "context7");
        assert_eq!(config.mcp.servers[0].transport, McpTransport::Stdio);
    }

    #[test]
    fn toml_deserialization_mcp_server_with_env() {
        let toml_str = r#"
            [mcp]
            enabled = true

            [[mcp.servers]]
            name = "exa"
            command = "npx"
            args = ["-y", "exa-mcp-server"]

            [mcp.servers.env]
            EXA_API_KEY = "test-key"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.mcp.servers[0].env.get("EXA_API_KEY").unwrap(),
            "test-key"
        );
    }
    #[test]
    fn model_profiles_deserialize_and_serialize_without_runtime_selection() {
        let source = r#"
            [provider]
            provider = "openai"
            api_key = "legacy-key"

            [models]
            default = "private"

            [models.profiles.private]
            provider = "anthropic"
            model = "claude-sonnet"
            api_key_env = "PRIVATE_ANTHROPIC_KEY"
            api_url = "https://private.example"
            temperature = 0.3
            timeout_secs = 45
        "#;
        let mut config: Config = toml::from_str(source).unwrap();
        assert_eq!(config.models.default.as_deref(), Some("private"));
        let profile = &config.models.profiles["private"];
        assert_eq!(profile.provider, "anthropic");
        assert_eq!(profile.model, "claude-sonnet");
        assert_eq!(
            profile.api_key_env.as_deref(),
            Some("PRIVATE_ANTHROPIC_KEY")
        );
        assert_eq!(profile.api_url.as_deref(), Some("https://private.example"));
        assert_eq!(profile.temperature, Some(0.3));
        assert_eq!(profile.timeout_secs, Some(45));
        config.active_profile = Some("private".into());
        let output = toml::to_string(&config).unwrap();
        assert!(!output.contains("active_profile"));
        let restored: Config = toml::from_str(&output).unwrap();
        assert!(restored.active_profile.is_none());
        assert_eq!(
            restored.models.profiles["private"].api_key_env,
            profile.api_key_env
        );
        assert_eq!(restored.provider.api_key.as_deref(), Some("legacy-key"));
    }
}

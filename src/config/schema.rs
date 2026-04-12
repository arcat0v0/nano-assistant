//! Configuration schema for nano-assistant.
//!
//! Config file location: ~/.config/nano-assistant/config.toml
//!
//! Priority: CLI flag > environment variable > config file

use serde::Deserialize;
use std::path::PathBuf;

/// Top-level configuration loaded from `config.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Provider configuration (API key, model selection, etc.)
    #[serde(default)]
    pub provider: ProviderConfig,

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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: ProviderConfig::default(),
            memory: MemoryConfig::default(),
            security: SecurityConfig::default(),
            behavior: BehaviorConfig::default(),
            skills: SkillsConfig::default(),
        }
    }
}

/// Provider configuration section.
#[derive(Debug, Clone, Deserialize)]
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

fn default_timeout() -> u64 {
    120
}

fn default_temperature() -> f64 {
    0.7
}

/// Memory backend configuration.
#[derive(Debug, Clone, Deserialize)]
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
#[derive(Debug, Clone, Deserialize)]
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
#[derive(Debug, Clone, Deserialize)]
pub struct BehaviorConfig {
    /// Maximum tool-call iterations per user message. Default: 10.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,

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
            verbose_errors: true,
            explain_tools: true,
            streaming: true,
        }
    }
}

fn default_max_iterations() -> usize {
    10
}

#[derive(Debug, Clone, Deserialize)]
pub struct SkillsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default)]
    pub allow_scripts: bool,

    #[serde(default)]
    pub skills_dir: Option<String>,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_scripts: false,
            skills_dir: None,
        }
    }
}

/// Environment variable prefix for configuration overrides.
pub const ENV_PREFIX: &str = "NA";

/// Get the default config file path: ~/.config/nano-assistant/config.toml
pub fn default_config_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        return home
            .join(".config")
            .join("nano-assistant")
            .join("config.toml");
    }
    PathBuf::from("~/.config/nano-assistant/config.toml")
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
}

pub mod models;
pub mod schema;

pub use schema::{
    BehaviorConfig, Config, HubConfig, McpConfig, McpServerConfig, McpTransport, MemoryConfig,
    ModelProfile, ModelsConfig, ProviderConfig, SecurityConfig, SkillsConfig,
};

use std::path::Path;

pub fn load_config_or_default(path: &Path) -> Config {
    if path.exists() {
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) => {
                eprintln!(
                    "[cli] warning: failed to read config {}: {error}",
                    path.display()
                );
                return Config::default();
            }
        };

        match toml::from_str(&content) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("[cli] warning: failed to parse config: {error}");
                Config::default()
            }
        }
    } else {
        Config::default()
    }
}

pub fn save_config(path: &Path, config: &Config) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let serialized = toml::to_string_pretty(config)?;
    std::fs::write(path, serialized)?;
    Ok(())
}

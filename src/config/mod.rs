pub mod credentials;
pub mod models;
pub mod schema;

pub use models::{ResolvedModel, SelectionSource};
pub use schema::{
    BehaviorConfig, Config, HubConfig, McpConfig, McpServerConfig, McpTransport, MemoryConfig,
    ModelProfile, ModelsConfig, ProviderConfig, SecurityConfig, SkillsConfig,
};

use std::io::Write;
use std::path::Path;

use anyhow::Context;

pub fn load_or_initialize_config(path: &Path) -> anyhow::Result<Config> {
    if path.exists() {
        return Ok(load_config_or_default(path));
    }

    let config = Config::first_run_default();
    let serialized = toml::to_string_pretty(&config)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating config directory {}", parent.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating config {}", path.display()))?;
    temporary.write_all(serialized.as_bytes())?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    match temporary.persist_noclobber(path) {
        Ok(_) => Ok(config),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(load_config_or_default(path))
        }
        Err(error) => {
            Err(error.error).with_context(|| format!("creating config {}", path.display()))
        }
    }
}

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
        Config::first_run_default()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_missing_config_gets_the_first_run_deepseek_default() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");

        let fresh = load_config_or_default(&path);
        assert!(fresh.first_run);
        assert_eq!(fresh.provider.provider.as_deref(), Some("deepseek"));
        assert_eq!(fresh.provider.model.as_deref(), Some("deepseek-flash"));

        std::fs::write(
            &path,
            "[provider]\nprovider = 'openai'\nmodel = 'gpt-4o-mini'\napi_key = 'fixture-only'\n",
        )
        .unwrap();
        let existing = load_config_or_default(&path);
        assert!(!existing.first_run);
        assert_eq!(existing.provider.provider.as_deref(), Some("openai"));
        assert_eq!(existing.provider.model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(existing.provider.api_key.as_deref(), Some("fixture-only"));
    }
}

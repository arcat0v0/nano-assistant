use std::io::Write;
use std::path::Path;

use anyhow::{bail, Context};
use toml_edit::{value, DocumentMut, Item, Table};

use super::{Config, ModelProfile, ProviderConfig};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionSource {
    CommandLine,
    Environment,
    SavedDefault,
    LegacyConfig,
    FirstRunDefault,
    Session,
}

/// The single, resolved model selection used by the request builder and UI.
/// Credentials are deliberately resolved separately and are never displayed.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedModel {
    pub profile_name: Option<String>,
    pub provider: String,
    pub model: String,
    pub api_url: Option<String>,
    pub api_key_env: Option<String>,
    pub temperature: f64,
    pub timeout_secs: u64,
    pub source: SelectionSource,
    pub allows_legacy_key: bool,
    pub label: String,
}

impl ResolvedModel {
    pub fn apply_to_config(&self, config: &Config) -> Config {
        let mut effective = config.clone();
        effective.provider = ProviderConfig {
            api_key: self
                .allows_legacy_key
                .then(|| config.provider.api_key.clone())
                .flatten(),
            provider: Some(self.provider.clone()),
            model: Some(self.model.clone()),
            api_url: self.api_url.clone(),
            temperature: self.temperature,
            timeout_secs: self.timeout_secs,
        };
        effective
    }

    pub fn is_provider(&self, provider: &str) -> bool {
        self.provider == provider
    }

    pub fn session_copy(&self) -> Self {
        let mut resolved = self.clone();
        resolved.source = SelectionSource::Session;
        resolved
    }
}

pub fn list_profiles(config: &Config) -> Vec<(&str, &ModelProfile)> {
    config
        .models
        .profiles
        .iter()
        .map(|(name, profile)| (name.as_str(), profile))
        .collect()
}

pub fn resolve_selection(
    config: &Config,
    profile: Option<&str>,
    model_id: Option<&str>,
    provider: Option<&str>,
) -> anyhow::Result<ResolvedModel> {
    if profile.is_some() && (model_id.is_some() || provider.is_some()) {
        bail!("a model profile cannot be combined with a model or provider override");
    }
    if provider.is_some() && model_id.is_none() {
        bail!("a provider override requires a model");
    }

    if let Some(name) = profile {
        return resolve_profile(config, name, SelectionSource::CommandLine);
    }

    let mut selected = if let Some(name) = config.models.default.as_deref() {
        resolve_profile(config, name, SelectionSource::SavedDefault)?
    } else if config.first_run {
        resolve_root_provider(config, SelectionSource::FirstRunDefault, false, "default")
    } else {
        resolve_root_provider(config, SelectionSource::LegacyConfig, true, "default")
    };
    if let Some(model) = model_id {
        return override_model(selected, model, provider, SelectionSource::CommandLine);
    }

    let env_model = std::env::var("NA_MODEL").ok();
    let env_provider = std::env::var("NA_PROVIDER").ok();
    if env_provider.is_some() && env_model.is_none() {
        bail!("NA_PROVIDER requires NA_MODEL");
    }
    if let Some(model) = env_model.as_deref() {
        selected = override_model(
            selected,
            model,
            env_provider.as_deref(),
            SelectionSource::Environment,
        )?;
    }

    Ok(selected)
}

pub fn resolve_profile(
    config: &Config,
    name: &str,
    source: SelectionSource,
) -> anyhow::Result<ResolvedModel> {
    if name == "default" {
        return Ok(resolve_root_provider(
            config,
            source,
            !config.first_run,
            "default",
        ));
    }
    let profile = config
        .models
        .profiles
        .get(name)
        .with_context(|| format!("model profile '{name}' not found"))?;
    validate_profile(profile)?;
    Ok(ResolvedModel {
        profile_name: Some(name.into()),
        provider: profile.provider.clone(),
        model: profile.model.clone(),
        api_url: profile.api_url.clone(),
        api_key_env: profile.api_key_env.clone(),
        temperature: profile
            .temperature
            .unwrap_or(super::schema::default_temperature()),
        timeout_secs: profile
            .timeout_secs
            .unwrap_or(super::schema::default_timeout()),
        source,
        allows_legacy_key: false,
        label: name.into(),
    })
}

fn resolve_root_provider(
    config: &Config,
    source: SelectionSource,
    allows_legacy_key: bool,
    label: &str,
) -> ResolvedModel {
    ResolvedModel {
        profile_name: None,
        provider: config
            .provider
            .provider
            .clone()
            .unwrap_or_else(|| "openai".into()),
        model: config
            .provider
            .model
            .clone()
            .unwrap_or_else(|| "gpt-4o-mini".into()),
        api_url: config.provider.api_url.clone(),
        api_key_env: None,
        temperature: config.provider.temperature,
        timeout_secs: config.provider.timeout_secs,
        source,
        allows_legacy_key,
        label: label.into(),
    }
}

fn override_model(
    mut selected: ResolvedModel,
    model: &str,
    provider: Option<&str>,
    source: SelectionSource,
) -> anyhow::Result<ResolvedModel> {
    if model.trim().is_empty() {
        bail!("model name must not be empty");
    }
    if let Some(provider) = provider {
        if provider.trim().is_empty() {
            bail!("provider name must not be empty");
        }
        if selected.provider != provider {
            selected.profile_name = None;
            selected.provider = provider.into();
            selected.model = model.into();
            selected.api_url = None;
            selected.api_key_env = None;
            selected.temperature = super::schema::default_temperature();
            selected.timeout_secs = super::schema::default_timeout();
            selected.allows_legacy_key = false;
            selected.source = source;
            selected.label = model.into();
            return Ok(selected);
        }
    }
    selected.model = model.into();
    selected.source = source;
    selected.label = model.into();
    Ok(selected)
}

fn validate_profile(profile: &ModelProfile) -> anyhow::Result<()> {
    if !matches!(
        profile.provider.as_str(),
        "openai"
            | "anthropic"
            | "gemini"
            | "deepseek"
            | "kimi"
            | "glm"
            | "mimo"
            | "qwen"
            | "ollama"
            | "compatible"
    ) {
        bail!("unknown model profile provider: '{}'", profile.provider);
    }
    if profile.model.trim().is_empty() {
        bail!("model profile requires a model");
    }
    if let Some(name) = profile.api_key_env.as_deref() {
        if !valid_env_name(name) {
            bail!("invalid API key environment variable name: '{name}'");
        }
    }
    if profile
        .api_url
        .as_ref()
        .is_some_and(|url| url.trim().is_empty())
    {
        bail!("model profile API URL must not be empty");
    }
    if let Some(temperature) = profile.temperature {
        if !temperature.is_finite() || !(0.0..=2.0).contains(&temperature) {
            bail!("model profile temperature must be between 0 and 2");
        }
    }
    if profile
        .timeout_secs
        .is_some_and(|timeout| timeout == 0 || timeout > i64::MAX as u64)
    {
        bail!("model profile timeout_secs must be positive and fit in TOML");
    }
    Ok(())
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.bytes();
    matches!(chars.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && chars.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

fn validate_name(name: &str) -> anyhow::Result<()> {
    if name.trim().is_empty() || name != name.trim() || name == "default" {
        bail!("invalid model profile name: '{name}'");
    }
    Ok(())
}

fn read_document(path: &Path) -> anyhow::Result<(DocumentMut, Config)> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let config: Config =
        toml::from_str(&source).with_context(|| format!("parsing config {}", path.display()))?;
    let document: DocumentMut = source
        .parse()
        .with_context(|| format!("parsing config {}", path.display()))?;
    Ok((document, config))
}

fn models_table(document: &mut DocumentMut) -> anyhow::Result<&mut Table> {
    if !document.as_table().contains_key("models") {
        document["models"] = Item::Table(Table::new());
    }
    document["models"]
        .as_table_mut()
        .context("[models] must be a table")
}

fn profiles_table(document: &mut DocumentMut) -> anyhow::Result<&mut Table> {
    let models = models_table(document)?;
    if !models.contains_key("profiles") {
        models["profiles"] = Item::Table(Table::new());
    }
    models["profiles"]
        .as_table_mut()
        .context("[models.profiles] must be a table")
}

fn write_document(path: &Path, document: &DocumentMut) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    if let Ok(metadata) = std::fs::metadata(path) {
        temporary
            .as_file()
            .set_permissions(metadata.permissions())?;
    }
    temporary.write_all(document.to_string().as_bytes())?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub fn add_profile(path: &Path, name: &str, profile: ModelProfile) -> anyhow::Result<()> {
    validate_name(name)?;
    validate_profile(&profile)?;
    let (mut document, config) = read_document(path)?;
    if config.models.profiles.contains_key(name) {
        bail!("model profile '{name}' already exists");
    }
    let mut table = Table::new();
    table["provider"] = value(profile.provider.as_str());
    table["model"] = value(profile.model.as_str());
    if let Some(api_url) = profile.api_url {
        table["api_url"] = value(api_url);
    }
    if let Some(api_key_env) = profile.api_key_env {
        table["api_key_env"] = value(api_key_env);
    }
    if let Some(temperature) = profile.temperature {
        table["temperature"] = value(temperature);
    }
    if let Some(timeout_secs) = profile.timeout_secs {
        table["timeout_secs"] = value(timeout_secs as i64);
    }
    profiles_table(&mut document)?.insert(name, Item::Table(table));
    write_document(path, &document)
}

pub fn set_default_profile(path: &Path, name: &str) -> anyhow::Result<()> {
    let (mut document, config) = read_document(path)?;
    if name == "default" {
        if config.models.default.is_some() {
            models_table(&mut document)?.remove("default");
            write_document(path, &document)?;
        }
        return Ok(());
    }
    let profile = config
        .models
        .profiles
        .get(name)
        .with_context(|| format!("model profile '{name}' not found"))?;
    validate_profile(profile)?;
    models_table(&mut document)?.insert("default", value(name));
    write_document(path, &document)
}

pub fn remove_profile(path: &Path, name: &str) -> anyhow::Result<()> {
    let (mut document, config) = read_document(path)?;
    if config.models.default.as_deref() == Some(name) {
        bail!("cannot remove default model profile '{name}'");
    }
    if !config.models.profiles.contains_key(name) {
        bail!("model profile '{name}' not found");
    }
    profiles_table(&mut document)?.remove(name);
    write_document(path, &document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load_config_or_default;
    use std::ffi::OsString;

    fn profile(provider: &str, model: &str) -> ModelProfile {
        ModelProfile {
            provider: provider.into(),
            model: model.into(),
            api_url: None,
            api_key_env: None,
            temperature: None,
            timeout_secs: None,
        }
    }

    struct SavedEnv(Vec<(&'static str, Option<OsString>)>);

    impl SavedEnv {
        fn new(names: &[&'static str]) -> Self {
            Self(
                names
                    .iter()
                    .map(|name| (*name, std::env::var_os(name)))
                    .collect(),
            )
        }
    }

    impl Drop for SavedEnv {
        fn drop(&mut self) {
            for (name, previous) in &self.0 {
                match previous {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn named_profile_isolates_legacy_credentials_url_and_provider_defaults() {
        let _env = SavedEnv::new(&["NA_MODEL", "NA_PROVIDER"]);
        std::env::remove_var("NA_MODEL");
        std::env::remove_var("NA_PROVIDER");
        let mut config = Config::default();
        config.provider.api_key = Some("legacy-secret".into());
        config.provider.api_url = Some("https://legacy.example".into());
        config.provider.temperature = 1.5;
        config.provider.timeout_secs = 9;
        let mut selected = profile("anthropic", "claude-custom");
        selected.api_key_env = Some("PROFILE_KEY".into());
        config.models.profiles.insert("work".into(), selected);
        config.models.default = Some("work".into());

        let effective = resolve_selection(&config, None, None, None).unwrap();
        assert_eq!(effective.label, "work");
        assert_eq!(effective.profile_name.as_deref(), Some("work"));
        assert_eq!(effective.provider, "anthropic");
        assert_eq!(effective.model, "claude-custom");
        assert!(!effective.allows_legacy_key);
        assert_eq!(effective.api_url, None);
        assert_eq!(effective.temperature, 0.7);
        assert_eq!(effective.timeout_secs, 120);
        assert_eq!(config.provider.api_key.as_deref(), Some("legacy-secret"));
        let mut tailored = profile("compatible", "custom");
        tailored.api_url = Some("http://localhost:5000/v1".into());
        tailored.temperature = Some(0.2);
        tailored.timeout_secs = Some(30);
        config.models.profiles.insert("local".into(), tailored);
        let effective = resolve_profile(&config, "local", SelectionSource::CommandLine).unwrap();
        assert_eq!(
            effective.api_url.as_deref(),
            Some("http://localhost:5000/v1")
        );
        assert_eq!(effective.temperature, 0.2);
        assert_eq!(effective.timeout_secs, 30);
        assert!(!effective.allows_legacy_key);
    }

    #[test]
    #[serial_test::serial]
    fn explicit_selection_beats_environment_then_default_then_legacy() {
        let _env = SavedEnv::new(&["NA_MODEL", "NA_PROVIDER"]);
        std::env::remove_var("NA_MODEL");
        std::env::remove_var("NA_PROVIDER");
        let mut config = Config::default();
        config
            .models
            .profiles
            .insert("work".into(), profile("anthropic", "claude"));
        config.models.default = Some("work".into());
        std::env::set_var("NA_MODEL", "env-model");
        std::env::set_var("NA_PROVIDER", "gemini");

        let effective = resolve_selection(&config, Some("work"), None, None).unwrap();
        assert_eq!(effective.label, "work");
        assert_eq!(effective.model, "claude");
        let effective =
            resolve_selection(&config, None, Some("cli-model"), Some("ollama")).unwrap();
        assert_eq!(effective.label, "cli-model");
        assert_eq!(effective.provider, "ollama");
        assert!(!effective.allows_legacy_key);
        let effective = resolve_selection(&config, None, None, None).unwrap();
        assert_eq!(effective.label, "env-model");
        assert_eq!(effective.provider, "gemini");
        std::env::remove_var("NA_MODEL");
        std::env::remove_var("NA_PROVIDER");
        let effective = resolve_selection(&config, None, None, None).unwrap();
        assert_eq!(effective.label, "work");
        let effective = resolve_selection(&config, Some("default"), None, None).unwrap();
        assert_eq!(effective.label, "default");
        assert!(effective.profile_name.is_none());
        assert_eq!(effective.model, "gpt-4o-mini");
        assert!(effective.allows_legacy_key);
    }

    #[test]
    #[serial_test::serial]
    fn raw_model_override_uses_saved_profile_and_provider_change_isolates_it() {
        let _env = SavedEnv::new(&["NA_MODEL", "NA_PROVIDER"]);
        std::env::remove_var("NA_MODEL");
        std::env::remove_var("NA_PROVIDER");
        let mut config = Config::default();
        config.provider.api_key = Some("legacy-key".into());
        config.provider.api_url = Some("https://legacy.example".into());
        let mut saved = profile("compatible", "saved-model");
        saved.api_key_env = Some("SAVED_KEY".into());
        saved.api_url = Some("https://saved.example/v1".into());
        saved.timeout_secs = Some(45);
        config.models.profiles.insert("second".into(), saved);
        config.models.default = Some("second".into());

        let effective = resolve_selection(&config, None, Some("temporary"), None).unwrap();
        assert_eq!(effective.label, "temporary");
        assert_eq!(effective.model, "temporary");
        assert_eq!(effective.provider, "compatible");
        assert_eq!(
            effective.api_url.as_deref(),
            Some("https://saved.example/v1")
        );
        assert_eq!(effective.timeout_secs, 45);
        assert_eq!(effective.profile_name.as_deref(), Some("second"));
        assert!(!effective.allows_legacy_key);

        std::env::set_var("NA_MODEL", "environment-model");
        let effective = resolve_selection(&config, None, None, None).unwrap();
        assert_eq!(effective.model, "environment-model");
        assert_eq!(
            effective.api_url.as_deref(),
            Some("https://saved.example/v1")
        );
        assert_eq!(effective.profile_name.as_deref(), Some("second"));

        std::env::set_var("NA_PROVIDER", "anthropic");
        let effective = resolve_selection(&config, None, None, None).unwrap();
        assert_eq!(effective.provider, "anthropic");
        assert_eq!(effective.model, "environment-model");
        assert!(!effective.allows_legacy_key);
        assert!(effective.api_url.is_none());
        assert_eq!(effective.timeout_secs, 120);
        assert!(effective.profile_name.is_none());
        let effective =
            resolve_selection(&config, None, Some("cli-model"), Some("openai")).unwrap();
        assert_eq!(effective.provider, "openai");
        assert!(effective.api_url.is_none());
        assert!(effective.profile_name.is_none());
    }

    #[test]
    #[serial_test::serial]
    fn invalid_selection_and_missing_default_do_not_fall_back() {
        let _env = SavedEnv::new(&["NA_MODEL", "NA_PROVIDER"]);
        std::env::remove_var("NA_MODEL");
        std::env::remove_var("NA_PROVIDER");
        let mut config = Config::default();
        config.models.default = Some("deleted".into());
        assert!(resolve_selection(&config, None, None, None)
            .unwrap_err()
            .to_string()
            .contains("deleted"));
        assert!(resolve_selection(&config, Some("missing"), None, None)
            .unwrap_err()
            .to_string()
            .contains("missing"));
        assert!(resolve_selection(&config, Some("default"), Some("model"), None).is_err());
        assert!(resolve_selection(&config, None, None, Some("openai")).is_err());
        std::env::set_var("NA_PROVIDER", "anthropic");
        assert!(resolve_selection(&Config::default(), None, None, None)
            .unwrap_err()
            .to_string()
            .contains("NA_PROVIDER"));
    }

    #[test]
    #[serial_test::serial]
    fn legacy_free_model_is_unchanged_by_profile_catalog() {
        let _env = SavedEnv::new(&["NA_MODEL", "NA_PROVIDER"]);
        std::env::remove_var("NA_MODEL");
        std::env::remove_var("NA_PROVIDER");
        let mut config = Config::default();
        config.provider.model = Some("free/community".into());
        config
            .models
            .profiles
            .insert("paid".into(), profile("openai", "gpt-4o"));
        let effective = resolve_selection(&config, None, None, None).unwrap();
        assert_eq!(effective.label, "default");
        assert_eq!(effective.model, "free/community");
    }

    #[test]
    fn profile_crud_preserves_other_settings_comments_and_rejects_invalid_changes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let original = "# user comment\n[security]\nmode = 'confirm' # keep this\n\n[provider]\napi_key = 'legacy-secret'\n";
        std::fs::write(&path, original).unwrap();
        let mut added = profile("compatible", "local-model");
        added.api_url = Some("http://localhost:1234/v1".into());
        add_profile(&path, "local", added).unwrap();
        let persisted = std::fs::read_to_string(&path).unwrap();
        assert!(persisted.contains("# user comment"));
        assert!(persisted.contains("mode = 'confirm' # keep this"));
        let parsed: Config = toml::from_str(&persisted).unwrap();
        assert_eq!(
            parsed.models.profiles["local"].api_url.as_deref(),
            Some("http://localhost:1234/v1")
        );
        assert_eq!(parsed.provider.api_key.as_deref(), Some("legacy-secret"));
        assert!(add_profile(&path, "local", profile("openai", "other")).is_err());
        assert!(add_profile(&path, "default", profile("openai", "other")).is_err());
        assert!(add_profile(&path, " ", profile("openai", "other")).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), persisted);
        set_default_profile(&path, "local").unwrap();
        assert!(remove_profile(&path, "local").is_err());
        assert!(set_default_profile(&path, "missing").is_err());
        assert_eq!(
            load_config_or_default(&path).models.default.as_deref(),
            Some("local")
        );
        set_default_profile(&path, "default").unwrap();
        assert!(load_config_or_default(&path).models.default.is_none());
        remove_profile(&path, "local").unwrap();
        assert!(load_config_or_default(&path).models.profiles.is_empty());
        assert!(remove_profile(&path, "local").is_err());
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("# user comment"));
    }

    #[test]
    fn profile_validation_and_new_config_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested").join("config.toml");
        let mut invalid = profile("other", "model");
        assert!(add_profile(&path, "invalid", invalid.clone()).is_err());
        invalid.provider = "openai".into();
        invalid.api_key_env = Some("INVALID-NAME".into());
        assert!(add_profile(&path, "invalid", invalid.clone()).is_err());
        invalid.api_key_env = None;
        invalid.temperature = Some(f64::NAN);
        assert!(add_profile(&path, "invalid", invalid.clone()).is_err());
        invalid.temperature = None;
        invalid.timeout_secs = Some(u64::MAX);
        assert!(add_profile(&path, "invalid", invalid).is_err());
        assert!(!path.exists());

        let mut valid = profile("openai", "gpt-4o-mini");
        valid.api_key_env = Some("OPENAI_PRIVATE_KEY".into());
        valid.timeout_secs = Some(45);
        add_profile(&path, "user.private", valid).unwrap();
        assert!(path.exists());
        let config = load_config_or_default(&path);
        assert_eq!(list_profiles(&config)[0].0, "user.private");
        assert_eq!(
            config.models.profiles["user.private"].timeout_secs,
            Some(45)
        );
        set_default_profile(&path, "user.private").unwrap();
        assert_eq!(
            load_config_or_default(&path).models.default.as_deref(),
            Some("user.private")
        );
    }

    #[test]
    fn built_in_vendor_profiles_are_valid_and_unknown_provider_is_rejected() {
        for provider in ["deepseek", "kimi", "glm", "mimo", "qwen"] {
            assert!(validate_profile(&profile(provider, "live-model")).is_ok());
        }
        assert!(validate_profile(&profile("unlisted-provider", "live-model")).is_err());
    }

    #[test]
    fn malformed_config_rejects_persistence_without_replacing_input() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let original = "[provider\ninvalid";
        std::fs::write(&path, original).unwrap();
        assert!(add_profile(&path, "valid", profile("openai", "gpt-4o-mini")).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }
}

use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;

use crate::agent::turn_streamed_to_stdout;
use crate::agent::{Agent, AgentModelContext};
use crate::config::models::{
    add_profile, list_profiles, remove_profile, resolve_selection, set_default_profile,
};
use crate::config::schema::default_config_path;
use crate::config::{
    load_config_or_default, load_or_initialize_config, Config, ModelProfile, ResolvedModel,
};
use crate::hub::{maybe_render_ad, model_routes_via_hub, HubClient};
use crate::security::{SecurityManager, SecurityMode};
use anyhow::Context;
use rig::agent::model::ModelHandle;

use super::CliArgs;
use super::HubSubcommand;
use super::IdentitySubcommand;
use super::ModelSubcommand;
use super::SkillsSubcommand;

struct CliArgsInner {
    prompt: Vec<String>,
    mode: Option<String>,
    debug: bool,
    config_path: Option<std::path::PathBuf>,
    verbose: bool,
    profile: Option<String>,
    model: Option<String>,
    provider: Option<String>,
}

impl CliArgsInner {
    fn prompt_text(&self) -> Option<String> {
        if self.prompt.is_empty() {
            None
        } else {
            Some(self.prompt.join(" "))
        }
    }
}

pub async fn run(args: CliArgs) -> anyhow::Result<()> {
    match args.command {
        Some(super::Commands::Chat {
            prompt,
            mode,
            debug,
            config_path,
            verbose,
            profile,
            model,
            provider,
        }) => {
            let inner = CliArgsInner {
                prompt,
                mode,
                debug,
                config_path,
                verbose,
                profile,
                model,
                provider,
            };
            run_chat(inner).await
        }
        Some(super::Commands::Skills { action }) => handle_skills_command(action).await,
        Some(super::Commands::Hub { action }) => handle_hub_command(action).await,
        Some(super::Commands::Identity { action }) => handle_identity_command(action).await,
        Some(super::Commands::Model {
            config_path,
            action,
        }) => handle_model_command(config_path.unwrap_or_else(default_config_path), action).await,
        None => {
            let config_path = default_config_path();
            let catalog = load_or_initialize_config(&config_path)?;
            let selected = resolve_selection(&catalog, None, None, None)?;
            let config = selected.apply_to_config(&catalog);
            let security_mode = resolve_security_mode(None, &config)?;
            let security = build_interactive_security_manager(
                &catalog,
                &config_path,
                security_mode,
                &selected,
            )?;
            run_interactive(config, config_path, security, catalog, selected).await
        }
    }
}

async fn run_chat(args: CliArgsInner) -> anyhow::Result<()> {
    let config_path = args.config_path.clone().unwrap_or_else(default_config_path);
    let catalog = load_or_initialize_config(&config_path)?;
    let selected = resolve_selection(
        &catalog,
        args.profile.as_deref(),
        args.model.as_deref(),
        args.provider.as_deref(),
    )?;
    let mut config = selected.apply_to_config(&catalog);
    let security_mode = resolve_security_mode(args.mode.as_deref(), &config)?;
    let security = if args.prompt.is_empty() {
        build_interactive_security_manager(&catalog, &config_path, security_mode, &selected)?
    } else {
        build_security_manager(&catalog, &config_path, security_mode, &selected)?
    };
    config.behavior.debug = resolve_debug_mode(args.debug, &config);
    let streaming = config.behavior.streaming;

    if args.verbose {
        eprintln!(
            "[cli] config loaded, security mode: {}, debug: {}",
            security_mode, config.behavior.debug
        );
    }

    match args.prompt_text() {
        Some(prompt) => {
            let model = crate::providers::build_model(&selected, &catalog, &config_path)?;
            let system_info = load_or_create_memory_md().await;
            let agent = build_agent(
                model,
                selected.clone(),
                &config,
                Arc::clone(&security),
                system_info,
                config_path.clone(),
            )
            .await;
            run_single(agent, &prompt, streaming, &config, &config_path).await
        }
        None => run_interactive(config, config_path, security, catalog, selected).await,
    }
}

async fn handle_model_command(
    config_path: std::path::PathBuf,
    action: ModelSubcommand,
) -> anyhow::Result<()> {
    let config = load_config_or_default(&config_path);
    match action {
        ModelSubcommand::List => {
            let selected = config.models.default.as_deref().unwrap_or("default");
            let provider = config.provider.provider.as_deref().unwrap_or("openai");
            let model = config.provider.model.as_deref().unwrap_or("gpt-4o-mini");
            println!(
                "{} default  {provider}  {model}",
                if selected == "default" { "*" } else { " " }
            );
            for (name, profile) in list_profiles(&config) {
                println!(
                    "{} {name}  {}  {}",
                    if selected == name { "*" } else { " " },
                    profile.provider,
                    profile.model
                );
            }
        }
        ModelSubcommand::Add {
            name,
            provider,
            model,
            api_url,
            api_key_env,
        } => {
            add_profile(
                &config_path,
                &name,
                ModelProfile {
                    provider,
                    model,
                    api_url,
                    api_key_env,
                    temperature: None,
                    timeout_secs: None,
                    reasoning_effort: None,
                },
            )?;
            println!("Added model profile {name}");
        }
        ModelSubcommand::Discover {
            provider,
            api_url,
            api_key_env,
        } => {
            for model in crate::providers::discover_models(
                &provider,
                api_url.as_deref(),
                api_key_env.as_deref(),
            )
            .await?
            {
                println!("{model}");
            }
        }
        ModelSubcommand::Use { name } => {
            let effective = resolve_selection(&config, Some(&name), None, None)?;
            crate::providers::build_model(&effective, &config, &config_path)?;
            set_default_profile(&config_path, &name)?;
            println!("Default model: {name}");
        }
        ModelSubcommand::Remove { name } => {
            remove_profile(&config_path, &name)?;
            println!("Removed model profile {name}");
        }
    }
    Ok(())
}

fn resolve_security_mode(
    mode_override: Option<&str>,
    config: &Config,
) -> anyhow::Result<SecurityMode> {
    mode_override
        .unwrap_or(&config.security.mode)
        .parse()
        .map_err(anyhow::Error::msg)
}

fn build_interactive_security_manager(
    config: &Config,
    config_path: &Path,
    mode: SecurityMode,
    selected: &ResolvedModel,
) -> anyhow::Result<Arc<SecurityManager>> {
    if mode == SecurityMode::Auto
        && config
            .security
            .review_profile
            .as_deref()
            .is_none_or(|name| name.trim().is_empty())
        && !model_routes_via_hub(&selected.apply_to_config(config))
        && !crate::providers::credentials_available(config, selected, config_path)?
    {
        let manager = SecurityManager::from_config_with_override(&config.security, Some(mode))
            .map_err(anyhow::Error::msg)?;
        return Ok(Arc::new(manager));
    }
    build_security_manager(config, config_path, mode, selected)
}

pub(crate) fn build_security_manager(
    config: &Config,
    config_path: &Path,
    mode: SecurityMode,
    selected: &ResolvedModel,
) -> anyhow::Result<Arc<SecurityManager>> {
    let manager = SecurityManager::from_config_with_override(&config.security, Some(mode))
        .map_err(anyhow::Error::msg)?;
    if mode != SecurityMode::Auto {
        return Ok(Arc::new(manager));
    }
    let resolved = match config
        .security
        .review_profile
        .as_deref()
        .filter(|name| !name.trim().is_empty())
    {
        None => selected.clone(),
        Some("default") => {
            anyhow::bail!("safety review profile must be a named [models.profiles] entry")
        }
        Some(name) => crate::config::models::resolve_profile(
            config,
            name,
            crate::config::SelectionSource::Session,
        )
        .context("resolving safety review profile")?,
    };
    let model = crate::providers::build_model(&resolved, config, config_path)
        .context("building safety review model")?;
    let reviewer = crate::security::review::ModelSafetyReviewer::new(
        model,
        resolved.temperature,
        std::time::Duration::from_secs(resolved.timeout_secs),
    )
    .with_protected_config(config_path.to_path_buf());
    Ok(Arc::new(manager.with_reviewer(Arc::new(reviewer))))
}

fn resolve_debug_mode(cli_debug: bool, config: &Config) -> bool {
    cli_debug || config.behavior.debug
}

pub(crate) fn memory_md_path() -> std::path::PathBuf {
    crate::platform::current_platform().memory_md_path()
}

pub(crate) async fn load_or_create_memory_md() -> Option<String> {
    let path = memory_md_path();

    if path.exists() {
        let content = std::fs::read_to_string(&path).ok()?;
        if let Some(system_info) =
            crate::memory::MarkdownMemory::extract_system_info_markdown(&content)
        {
            return Some(system_info);
        }

        let info = crate::system_info::detect().await;
        let content = crate::memory::MarkdownMemory::upsert_system_info_markdown(
            &content,
            &info.format_as_markdown(),
        );

        return if std::fs::write(&path, &content).is_ok() {
            crate::memory::MarkdownMemory::extract_system_info_markdown(&content)
        } else {
            None
        };
    }

    let info = crate::system_info::detect().await;
    let content =
        crate::memory::MarkdownMemory::upsert_system_info_markdown("", &info.format_as_markdown());

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    if std::fs::write(&path, &content).is_ok() {
        crate::memory::MarkdownMemory::extract_system_info_markdown(&content)
    } else {
        None
    }
}

pub(crate) async fn build_agent(
    model: ModelHandle,
    resolved_model: ResolvedModel,
    config: &Config,
    security: Arc<SecurityManager>,
    system_info: Option<String>,
    config_path: std::path::PathBuf,
) -> Agent {
    let mut dynamic_tools = Vec::new();

    let skills = if config.skills.enabled {
        crate::skills::load_skills(&config.skills)
    } else {
        Vec::new()
    };

    let skill_tools = crate::skills::skills_to_tools(&skills, Arc::clone(&security));
    dynamic_tools.extend(skill_tools);

    // Register knowledge source tools from skills with type = "knowledge-source"
    for skill in &skills {
        if let Some(ks_config) = crate::skills::parse_knowledge_source_config(skill) {
            let source = crate::knowledge::create_source(&ks_config);
            let ks_tools = crate::knowledge::source_to_tools(source);
            dynamic_tools.extend(ks_tools);
        }
    }

    let memory: Option<Arc<dyn crate::memory::Memory>> = if config.memory.enabled {
        Some(Arc::new(crate::memory::MarkdownMemory::new(
            memory_md_path(),
        )))
    } else {
        None
    };

    Agent::new(
        model,
        AgentModelContext {
            selection: resolved_model,
            config: config.clone(),
        },
        dynamic_tools,
        memory,
        skills,
        system_info,
        security,
        config_path,
    )
    .await
}

async fn run_single(
    mut agent: Agent,
    prompt: &str,
    streaming: bool,
    config: &Config,
    config_path: &Path,
) -> anyhow::Result<()> {
    let ctrl_c_handle = spawn_immediate_ctrl_c_exit();

    if streaming {
        let result = turn_streamed_to_stdout(&mut agent, prompt).await?;
        if result.tool_calls_count > 0 {
            eprintln!(
                "{}",
                crate::console::format_tool_summary(result.tool_calls_count)
            );
        }
        render_post_response_ad(config, config_path).await;
        ctrl_c_handle.abort();
        Ok(())
    } else {
        let result = match agent.turn(prompt).await {
            Ok(result) => {
                crate::render::render_markdown_to_stdout(&result.response);
                if result.tool_calls_count > 0 {
                    eprintln!(
                        "{}",
                        crate::console::format_tool_summary(result.tool_calls_count)
                    );
                }
                Ok(())
            }
            Err(e) => {
                eprintln!("[cli] error: {e:#}");
                Err(e)
            }
        };
        if result.is_ok() {
            render_post_response_ad(config, config_path).await;
        }
        ctrl_c_handle.abort();
        result
    }
}

async fn run_interactive(
    config: Config,
    config_path: std::path::PathBuf,
    security: Arc<SecurityManager>,
    catalog: Config,
    selected: ResolvedModel,
) -> anyhow::Result<()> {
    let history_path = config_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("history.txt");
    crate::tui::run_tui(
        config,
        config_path,
        catalog,
        selected,
        history_path,
        security,
    )
    .await
}

fn spawn_immediate_ctrl_c_exit() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            crate::tui::interaction::restore_terminal_interaction();
            let _ = writeln!(io::stderr());
            std::process::exit(130);
        }
    })
}

async fn handle_hub_command(command: HubSubcommand) -> anyhow::Result<()> {
    match command {
        HubSubcommand::Status { config_path } => {
            let config_path = config_path.unwrap_or_else(default_config_path);
            let config = load_config_or_default(&config_path);
            let client = HubClient::new(config_path.clone(), config)?;
            let resolved = client.resolved_config().await;

            println!("hub url: {}", resolved.url);
            println!("enabled: {}", resolved.enabled);
            println!("identity: {}", resolved.identity_path.display());
            println!(
                "machine_id: {}",
                resolved.machine_id.as_deref().unwrap_or("(not registered)")
            );
            println!("auto_register: {}", resolved.auto_register);
            println!("ad_display: {}", resolved.ad_display);

            if resolved.enabled && resolved.machine_id.is_some() {
                match client.quota_status().await {
                    Ok(quota) => {
                        println!();
                        println!("status: {}", quota.status);
                        println!(
                            "quota: rpm {}/{} | tpm {}/{} | daily {}/{} | concurrent {}/{}",
                            quota.usage.rpm_current,
                            quota.limits.rpm,
                            quota.usage.tpm_current,
                            quota.limits.tpm,
                            quota.usage.daily_used,
                            quota.limits.daily,
                            quota.usage.concurrent_current,
                            quota.limits.concurrent
                        );
                        println!(
                            "daily remaining: {} | reset daily @ {}",
                            quota.usage.daily_remaining, quota.reset_at.daily
                        );
                    }
                    Err(error) => {
                        println!();
                        println!("quota: unavailable");
                        println!("reason: {error}");
                    }
                }
            }

            Ok(())
        }
        HubSubcommand::Register { config_path } => {
            let config_path = config_path.unwrap_or_else(default_config_path);
            let config = load_config_or_default(&config_path);
            let client = HubClient::new(config_path, config)?;
            let registration = client.register_machine(true).await?;
            println!("registered machine_id: {}", registration.machine_id);
            println!("issued_at: {}", registration.issued_at);
            Ok(())
        }
        HubSubcommand::Disable { config_path } => {
            let config_path = config_path.unwrap_or_else(default_config_path);
            let config = load_config_or_default(&config_path);
            let client = HubClient::new(config_path, config)?;
            client.disable().await?;
            println!("hub disabled in config");
            Ok(())
        }
    }
}

async fn handle_identity_command(command: IdentitySubcommand) -> anyhow::Result<()> {
    match command {
        IdentitySubcommand::Export { path, config_path } => {
            let config_path = config_path.unwrap_or_else(default_config_path);
            let config = load_config_or_default(&config_path);
            let client = HubClient::new(config_path, config)?;
            let export = client.export_identity(&path).await?;
            println!("identity exported to {}", path.display());
            if let Some(machine_id) = export.machine_id.as_deref() {
                println!("machine_id: {machine_id}");
            }
            Ok(())
        }
        IdentitySubcommand::Import { path, config_path } => {
            let config_path = config_path.unwrap_or_else(default_config_path);
            let config = load_config_or_default(&config_path);
            let client = HubClient::new(config_path, config)?;
            let import = client.import_identity(&path).await?;
            println!("identity imported from {}", path.display());
            if let Some(machine_id) = import.machine_id.as_deref() {
                println!("machine_id restored: {machine_id}");
            }
            Ok(())
        }
    }
}

async fn render_post_response_ad(config: &Config, config_path: &Path) {
    if !model_routes_via_hub(config) {
        return;
    }

    if let Err(error) = maybe_render_ad(config_path.to_path_buf(), "inline_after_response").await {
        eprintln!("[hub] ad fetch skipped: {error}");
    }
}

async fn handle_skills_command(command: SkillsSubcommand) -> anyhow::Result<()> {
    let config_path = default_config_path();
    let config = load_config_or_default(&config_path);

    match command {
        SkillsSubcommand::List => {
            let skills = crate::skills::load_skills(&config.skills);
            if skills.is_empty() {
                println!("No skills installed.");
                println!();
                println!("  Create one: mkdir -p ~/.config/nano-assistant/skills/my-skill");
                println!("              echo '# My Skill' > ~/.config/nano-assistant/skills/my-skill/SKILL.md");
                println!();
                println!("  Or install: na skills install <source>");
            } else {
                // Compute column widths
                let name_width = skills
                    .iter()
                    .map(|s| s.name.len())
                    .max()
                    .unwrap_or(4)
                    .max(4);
                let version_width = skills
                    .iter()
                    .map(|s| s.version.len())
                    .max()
                    .unwrap_or(7)
                    .max(7);

                println!("Installed skills ({}):", skills.len());
                println!();
                println!(
                    "  {:<name_w$}  {:<ver_w$}  SOURCE",
                    "NAME",
                    "VERSION",
                    name_w = name_width,
                    ver_w = version_width,
                );
                println!(
                    "  {:<name_w$}  {:<ver_w$}  ------",
                    "----",
                    "-------",
                    name_w = name_width,
                    ver_w = version_width,
                );
                for skill in &skills {
                    let source_str = match &skill.source {
                        Some(src) => src.to_string(),
                        None => "unknown".to_string(),
                    };
                    println!(
                        "  {:<name_w$}  {:<ver_w$}  {}",
                        skill.name,
                        skill.version,
                        source_str,
                        name_w = name_width,
                        ver_w = version_width,
                    );
                }
            }
            println!();
            Ok(())
        }
        SkillsSubcommand::Install { source } => {
            println!("Installing skill from: {source}");
            let skills_dir = crate::skills::skills_dir();
            std::fs::create_dir_all(&skills_dir)?;

            let allow_scripts = config.skills.allow_scripts;

            let (installed_dir, files_scanned) = if crate::skills::is_clawhub_source(&source) {
                crate::skills::install_clawhub_skill_source(&source, &skills_dir, allow_scripts)
                    .with_context(|| format!("failed to install from ClawHub: {source}"))?
            } else if crate::skills::is_git_source(&source) {
                crate::skills::install_git_skill_source(&source, &skills_dir, allow_scripts)
                    .with_context(|| format!("failed to install from git: {source}"))?
            } else {
                crate::skills::install_local_skill_source(&source, &skills_dir, allow_scripts)
                    .with_context(|| format!("failed to install local skill: {source}"))?
            };

            println!(
                "  ✓ Skill installed and audited: {} ({} files scanned)",
                installed_dir.display(),
                files_scanned
            );
            Ok(())
        }
        SkillsSubcommand::Remove { name } => {
            if name.contains("..") || name.contains('/') || name.contains('\\') {
                anyhow::bail!("Invalid skill name: {name}");
            }
            let skill_path = crate::skills::skills_dir().join(&name);
            if !skill_path.exists() {
                anyhow::bail!("Skill not found: {name}");
            }
            std::fs::remove_dir_all(&skill_path)?;
            println!("  ✓ Skill '{}' removed.", name);
            Ok(())
        }
        SkillsSubcommand::Audit { source } => {
            let source_path = std::path::PathBuf::from(&source);
            let target = if source_path.exists() {
                source_path
            } else {
                crate::skills::skills_dir().join(&source)
            };
            if !target.exists() {
                anyhow::bail!("Skill source not found: {source}");
            }
            let report = crate::skills::audit::audit_skill_directory_with_options(
                &target,
                crate::skills::audit::SkillAuditOptions {
                    allow_scripts: config.skills.allow_scripts,
                },
            )?;
            if report.is_clean() {
                println!(
                    "  ✓ Skill audit passed for {} ({} files scanned).",
                    target.display(),
                    report.files_scanned
                );
            } else {
                println!("  ✗ Skill audit failed for {}", target.display());
                for finding in report.findings {
                    println!("    - {finding}");
                }
                anyhow::bail!("Skill audit failed.");
            }
            Ok(())
        }
        SkillsSubcommand::Test { name } => {
            let results = if let Some(ref skill_name) = name {
                let target = crate::skills::skills_dir().join(skill_name);
                if !target.exists() {
                    anyhow::bail!("Skill not found: {}", skill_name);
                }
                let r = crate::skills::testing::test_skill(&target, skill_name, false)?;
                if r.tests_run == 0 {
                    println!("  - No TEST.sh found for skill '{}'.", skill_name);
                    return Ok(());
                }
                vec![r]
            } else {
                crate::skills::testing::test_all_skills(&[crate::skills::skills_dir()], false)?
            };
            crate::skills::testing::print_results(&results);
            let any_failed = results.iter().any(|r| !r.failures.is_empty());
            if any_failed {
                anyhow::bail!("Some skill tests failed.");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::Config;
    use crate::security::SecurityMode;
    use clap::Parser;
    use serial_test::serial;

    fn parse_chat(args: &[&str]) -> CliArgs {
        let mut full = vec!["na", "chat"];
        full.extend(args);
        CliArgs::parse_from(full)
    }

    #[test]
    fn resolve_security_mode_cli_override_precedence() {
        let args = parse_chat(&["--mode", "confirm"]);
        let config = Config::default();
        let mode = resolve_security_mode(args.mode(), &config).unwrap();
        assert_eq!(mode, SecurityMode::Confirm);
    }

    #[test]
    fn resolve_security_mode_invalid_cli_stops_startup() {
        let args = parse_chat(&["--mode", "bogus"]);
        let config = Config::default();
        let mode = resolve_security_mode(args.mode(), &config);
        assert!(mode.is_err());
    }

    #[test]
    fn resolve_security_mode_no_cli_uses_config_mode() {
        let args = parse_chat(&[]);
        let mut config = Config::default();
        config.security.mode = "whitelist".to_string();
        let mode = resolve_security_mode(args.mode(), &config).unwrap();
        assert_eq!(mode, SecurityMode::Whitelist);
    }

    #[test]
    fn load_config_nonexistent_uses_deepseek_first_run_default() {
        let path = std::path::Path::new("/tmp/does_not_exist_na_test_config_99999.toml");
        let config = load_config_or_default(path);
        assert_eq!(config.provider.provider, Some("deepseek".to_string()));
        assert_eq!(config.provider.model, Some("deepseek-flash".to_string()));
        assert!(config.first_run);
        assert_eq!(config.provider.temperature, 0.7);
        assert!(config.memory.enabled);
        assert_eq!(config.security.mode, "auto");
        assert_eq!(config.behavior.max_iterations, 50);
        assert!(!config.behavior.debug);
        assert!(config.behavior.streaming);
    }

    #[test]
    fn load_config_invalid_toml_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("bad_config.toml");
        std::fs::write(&config_path, "this is not valid toml {{{").unwrap();
        let config = load_config_or_default(&config_path);
        assert_eq!(config.provider.provider, Some("openai".to_string()));
        assert_eq!(config.provider.model, Some("gpt-4o-mini".to_string()));
        assert_eq!(config.provider.temperature, 0.7);
        assert!(config.memory.enabled);
    }

    #[test]
    fn prompt_text_empty_returns_none() {
        let args = parse_chat(&[]);
        assert!(args.prompt_text().is_none());
    }

    #[test]
    fn prompt_text_single_word_returns_some() {
        let args = parse_chat(&["hello"]);
        assert_eq!(args.prompt_text(), Some("hello".to_string()));
    }

    #[test]
    fn prompt_text_multiple_words_joined_with_space() {
        let args = parse_chat(&["hello", "world"]);
        assert_eq!(args.prompt_text(), Some("hello world".to_string()));
    }

    #[test]
    fn cli_none_command_is_interactive() {
        let args = CliArgs::parse_from(["na"]);
        assert!(args.command.is_none());
        assert!(args.prompt_text().is_none());
    }

    #[test]
    fn cli_skills_subcommand_parses() {
        let args = CliArgs::parse_from(["na", "skills", "list"]);
        match args.command {
            Some(crate::cli::Commands::Skills { action }) => {
                assert!(matches!(action, SkillsSubcommand::List));
            }
            _ => panic!("expected Skills command"),
        }
    }

    #[test]
    fn cli_hub_subcommand_parses() {
        let args = CliArgs::parse_from(["na", "hub", "status"]);
        match args.command {
            Some(crate::cli::Commands::Hub { action }) => {
                assert!(matches!(action, HubSubcommand::Status { .. }));
            }
            _ => panic!("expected Hub command"),
        }
    }

    #[test]
    fn cli_identity_export_subcommand_parses() {
        let args = CliArgs::parse_from(["na", "identity", "export", "/tmp/id.json"]);
        match args.command {
            Some(crate::cli::Commands::Identity { action }) => match action {
                IdentitySubcommand::Export { path, .. } => {
                    assert_eq!(path, std::path::PathBuf::from("/tmp/id.json"));
                }
                _ => panic!("expected identity export command"),
            },
            _ => panic!("expected Identity command"),
        }
    }

    #[test]
    fn cli_chat_mode_flag() {
        let args = parse_chat(&["--mode", "confirm"]);
        assert_eq!(args.mode(), Some("confirm"));
    }

    #[test]
    fn cli_chat_verbose_flag() {
        let args = parse_chat(&["-v"]);
        assert!(args.is_verbose());
    }

    #[test]
    fn cli_chat_debug_flag() {
        let args = parse_chat(&["--debug"]);
        assert!(args.is_debug());
    }

    #[test]
    fn chat_help_is_not_sent_as_a_model_prompt() {
        let error = CliArgs::try_parse_from(["na", "chat", "--help"]).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
    }

    #[test]
    fn chat_model_override_and_prompt_are_parsed_together() {
        let args = parse_chat(&["--model", "llama3:8b", "explain", "the", "result"]);
        match args.command {
            Some(super::super::Commands::Chat { model, prompt, .. }) => {
                assert_eq!(model.as_deref(), Some("llama3:8b"));
                assert_eq!(prompt, ["explain", "the", "result"]);
            }
            _ => panic!("expected chat command"),
        }
    }

    #[test]
    fn resolve_debug_mode_cli_takes_precedence() {
        let args = parse_chat(&["--debug"]);
        let mut config = Config::default();
        config.behavior.debug = false;
        assert!(resolve_debug_mode(args.is_debug(), &config));
    }

    #[test]
    fn resolve_debug_mode_uses_config_when_cli_off() {
        let args = parse_chat(&[]);
        let mut config = Config::default();
        config.behavior.debug = true;
        assert!(resolve_debug_mode(args.is_debug(), &config));
    }

    #[test]
    fn cli_chat_config_path_flag() {
        let args = parse_chat(&["--config-path", "/tmp/test.toml"]);
        assert_eq!(
            args.config_path(),
            std::path::PathBuf::from("/tmp/test.toml")
        );
    }

    #[test]
    fn memory_md_path_returns_config_dir() {
        let path = memory_md_path();
        let path_str = path.to_string_lossy();
        assert!(
            path_str.ends_with("/nano-assistant/MEMORY.md")
                || path_str.ends_with("\\nano-assistant\\MEMORY.md"),
            "path was: {path_str}"
        );
    }

    #[tokio::test]
    #[serial(home_env)]
    async fn load_or_create_memory_md_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let memory_path = dir.path().join(".config/nano-assistant/MEMORY.md");
        let saved_home = std::env::var_os("HOME");
        let saved_xdg = std::env::var_os("XDG_CONFIG_HOME");

        std::env::set_var("HOME", dir.path());
        std::env::remove_var("XDG_CONFIG_HOME");
        let path = memory_md_path();
        assert_eq!(path, memory_path);

        let content = load_or_create_memory_md().await;
        assert!(content.is_some());
        assert!(memory_path.exists());
        let content = content.unwrap();
        assert!(content.contains("### System"));
        let written = std::fs::read_to_string(&memory_path).unwrap();
        assert!(written.contains("# Nano-Assistant Memory"));
        assert!(written.contains("## System Information"));

        match saved_home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }
        match saved_xdg {
            Some(xdg) => std::env::set_var("XDG_CONFIG_HOME", xdg),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }

    #[tokio::test]
    #[serial(home_env)]
    async fn memory_md_lifecycle_read_existing_returns_content() {
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join(".config").join("nano-assistant");
        std::fs::create_dir_all(&memory_dir).unwrap();
        let memory_path = memory_dir.join("MEMORY.md");
        let test_content =
            "# Nano-Assistant Memory\n\n<!-- SYSTEM_INFO_START -->\n## System Information\n\n### System\n\nTest content.\n<!-- SYSTEM_INFO_END -->\n";
        std::fs::write(&memory_path, &test_content).unwrap();
        let saved_home = std::env::var_os("HOME");
        let saved_xdg = std::env::var_os("XDG_CONFIG_HOME");

        std::env::set_var("HOME", dir.path());
        std::env::remove_var("XDG_CONFIG_HOME");
        let content = load_or_create_memory_md().await;
        match saved_home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }
        match saved_xdg {
            Some(xdg) => std::env::set_var("XDG_CONFIG_HOME", xdg),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }

        assert_eq!(content, Some("### System\n\nTest content.".to_string()));
    }

    #[tokio::test]
    async fn load_memory_md_handles_readonly_fs() {
        let _ = load_or_create_memory_md().await;
    }
}

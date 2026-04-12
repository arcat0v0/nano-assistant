use std::io::{self, Write};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use anyhow::Context;
use crate::agent::turn_streamed_to_stdout;
use crate::agent::Agent;
use crate::config::schema::{Config, default_config_path};
use crate::providers::Provider;
use crate::security::{SecurityManager, SecurityMode, UserConfirmation};
use crate::tools;

use super::CliArgs;
use super::SkillsSubcommand;

struct CliArgsInner {
    prompt: Vec<String>,
    mode: Option<String>,
    config: bool,
    config_path: Option<std::path::PathBuf>,
    verbose: bool,
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
        Some(super::Commands::Chat { prompt, mode, config, config_path, verbose }) => {
            let inner = CliArgsInner { prompt, mode, config, config_path, verbose };
            run_chat(inner).await
        }
        Some(super::Commands::Skills { action }) => {
            handle_skills_command(action).await
        }
        None => {
            let config_path = default_config_path();
            let config = load_config(&config_path);
            let security_mode = SecurityMode::Direct;
            let streaming = config.behavior.streaming;
            let provider = build_provider(&config)?;
            run_interactive(provider, &config, config_path, security_mode, streaming).await
        }
    }
}

async fn run_chat(args: CliArgsInner) -> anyhow::Result<()> {
    if args.config {
        let config_path = args.config_path
            .clone()
            .unwrap_or_else(default_config_path);
        return open_config_editor(config_path);
    }

    let config = load_config(&args.config_path.clone().unwrap_or_else(default_config_path));
    let security_mode = resolve_security_mode(args.mode.as_deref(), &config);
    let streaming = config.behavior.streaming;

    if args.verbose {
        eprintln!(
            "[cli] config loaded, security mode: {}",
            security_mode
        );
    }

    match args.prompt_text() {
        Some(prompt) => {
            let provider = build_provider(&config)?;
            let agent = build_agent(provider, &config, security_mode, None);
            run_single(agent, &prompt, streaming).await
        }
        None => {
            let provider = build_provider(&config)?;
            let config_path = args.config_path.unwrap_or_else(default_config_path);
            run_interactive(provider, &config, config_path, security_mode, streaming).await
        }
    }
}

fn load_config(path: &std::path::Path) -> Config {
    if path.exists() {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[cli] warning: failed to read config {}: {e}", path.display());
                return Config::default();
            }
        };
        match toml::from_str(&content) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("[cli] warning: failed to parse config: {e}");
                Config::default()
            }
        }
    } else {
        Config::default()
    }
}

fn resolve_security_mode(mode_override: Option<&str>, config: &Config) -> SecurityMode {
    match mode_override {
        Some(m) => match m.parse::<SecurityMode>() {
            Ok(mode) => mode,
            Err(e) => {
                eprintln!("[cli] warning: invalid --mode '{m}': {e}, using default");
                config.security.mode.parse().unwrap_or_default()
            }
        },
        None => config.security.mode.parse().unwrap_or_default(),
    }
}

fn resolve_api_key(config: &Config, env_vars: &[&str]) -> Option<String> {
    config
        .provider
        .api_key
        .clone()
        .or_else(|| env_vars.iter().find_map(|v| std::env::var(v).ok()))
}

fn build_provider(config: &Config) -> anyhow::Result<Arc<dyn Provider>> {
    let provider_name = config
        .provider
        .provider
        .as_deref()
        .unwrap_or("openai");

    let base_url = config.provider.api_url.as_deref();

    let provider: Arc<dyn Provider> = match provider_name {
        "openai" => {
            let key = resolve_api_key(config, &["NA_API_KEY", "OPENAI_API_KEY"]);
            Arc::new(
                crate::providers::openai::OpenAiProvider::new(key.as_deref())
                    .with_base_url(base_url.unwrap_or("https://api.openai.com/v1")),
            )
        }
        "anthropic" => {
            let key = resolve_api_key(config, &["NA_API_KEY", "ANTHROPIC_API_KEY"]);
            Arc::new(crate::providers::anthropic::AnthropicProvider::new(key.as_deref()))
        }
        "gemini" => {
            let key = resolve_api_key(config, &["NA_API_KEY", "GEMINI_API_KEY"]);
            Arc::new(crate::providers::gemini::GeminiProvider::new(key.as_deref()))
        }
        "glm" => {
            let key = resolve_api_key(config, &["NA_API_KEY", "GLM_API_KEY"]);
            Arc::new(crate::providers::glm::GlmProvider::new(key.as_deref()))
        }
        "ollama" | "compatible" => {
            let key = resolve_api_key(config, &["NA_API_KEY"]);
            let default_url = if provider_name == "ollama" {
                "http://localhost:11434/v1"
            } else {
                "http://localhost:8080/v1"
            };
            let url = base_url.unwrap_or(default_url);
            Arc::new(crate::providers::compatible::CompatibleProvider::new(
                provider_name,
                default_url,
                key.as_deref(),
                Some(url),
            ))
        }
        other => anyhow::bail!(
            "unknown provider: '{other}'. Valid: openai, anthropic, gemini, glm, ollama"
        ),
    };

    Ok(provider)
}

fn build_agent(
    provider: Arc<dyn Provider>,
    config: &Config,
    security_mode: SecurityMode,
    confirmer: Option<Arc<dyn UserConfirmation>>,
) -> Agent {
    let raw_tools = tools::default_tools();
    let mut secured_tools: Vec<Box<dyn crate::tools::Tool>> = raw_tools
        .into_iter()
        .map(|t| {
            let mut sec_mgr = SecurityManager::from_config_with_override(
                &config.security,
                Some(security_mode),
            );
            if let Some(confirmer) = confirmer.clone() {
                sec_mgr = sec_mgr.with_confirmer(confirmer);
            }
            Box::new(crate::security::SecureTool::new(t, Arc::new(sec_mgr)))
                as Box<dyn crate::tools::Tool>
        })
        .collect();

    let skills = if config.skills.enabled {
        crate::skills::load_skills(&config.skills)
    } else {
        Vec::new()
    };

    let skill_tools = crate::skills::skills_to_tools(&skills);
    secured_tools.extend(skill_tools);

    let memory: Option<Arc<dyn crate::memory::Memory>> = if config.memory.enabled {
        let memory_dir = default_config_path()
            .parent()
            .unwrap_or(Path::new("."))
            .join("memory");
        Some(Arc::new(crate::memory::MarkdownMemory::new(memory_dir)))
    } else {
        None
    };

    Agent::with_skills(provider, secured_tools, memory, config.clone(), skills)
}

async fn run_single(mut agent: Agent, prompt: &str, streaming: bool) -> anyhow::Result<()> {
    let ctrl_c_handle = spawn_immediate_ctrl_c_exit();

    if streaming {
        let result = turn_streamed_to_stdout(&mut agent, prompt).await?;
        if result.tool_calls_count > 0 {
            eprintln!("{}", crate::console::format_tool_summary(result.tool_calls_count));
        }
        ctrl_c_handle.abort();
        Ok(())
    } else {
        let result = match agent.turn(prompt).await {
            Ok(result) => {
                println!("{}", result.response);
                if result.tool_calls_count > 0 {
                    eprintln!("{}", crate::console::format_tool_summary(result.tool_calls_count));
                }
                Ok(())
            }
            Err(e) => {
                eprintln!("[cli] error: {e:#}");
                Err(e)
            }
        };
        ctrl_c_handle.abort();
        result
    }
}

async fn run_interactive(
    provider: Arc<dyn Provider>,
    config: &Config,
    config_path: std::path::PathBuf,
    security_mode: SecurityMode,
    streaming: bool,
) -> anyhow::Result<()> {
    let agent = build_agent(provider, config, security_mode, None);
    let history_path = config_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("history.txt");
    crate::tui::run_tui(agent, streaming, history_path).await
}

fn spawn_immediate_ctrl_c_exit() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = writeln!(io::stderr());
            std::process::exit(130);
        }
    })
}

fn open_config_editor(config_path: std::path::PathBuf) -> anyhow::Result<()> {
    if !config_path.exists() {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let default_config = r#"# nano-assistant configuration
[provider]
# provider = "openai"    # openai | anthropic | gemini | glm | ollama
# model = "gpt-4o-mini"
# api_key = "sk-..."
# api_url = ""
# temperature = 0.7

[memory]
# enabled = true
# max_messages = 100

[security]
# mode = "direct"        # direct | confirm | whitelist
# whitelist = ["ls", "cat", "docker *"]

[behavior]
# max_iterations = 10
# verbose_errors = true
# explain_tools = true
"#;
        std::fs::write(&config_path, default_config)?;
        eprintln!("[cli] created default config at {}", config_path.display());
    }

    let editor = std::env::var("EDITOR")
        .unwrap_or_else(|_| {
            if Command::new("nano").arg("--version").output().is_ok() {
                "nano".to_string()
            } else {
                "vim".to_string()
            }
        });

    let status = Command::new(&editor)
        .arg(&config_path)
        .status()?;

    if status.success() {
        eprintln!("[cli] config saved to {}", config_path.display());
    } else {
        anyhow::bail!("editor '{editor}' exited with status {}", status);
    }

    Ok(())
}

async fn handle_skills_command(command: SkillsSubcommand) -> anyhow::Result<()> {
    let config_path = default_config_path();
    let config = load_config(&config_path);

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
                println!("Installed skills ({}):", skills.len());
                println!();
                for skill in &skills {
                    println!("  {} v{} — {}", skill.name, skill.version, skill.description);
                    if !skill.tools.is_empty() {
                        println!("    Tools: {}", skill.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", "));
                    }
                    if !skill.tags.is_empty() {
                        println!("    Tags:  {}", skill.tags.join(", "));
                    }
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

            println!("  ✓ Skill installed and audited: {} ({} files scanned)", installed_dir.display(), files_scanned);
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
                crate::skills::audit::SkillAuditOptions { allow_scripts: config.skills.allow_scripts },
            )?;
            if report.is_clean() {
                println!("  ✓ Skill audit passed for {} ({} files scanned).", target.display(), report.files_scanned);
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

    fn parse_chat(args: &[&str]) -> CliArgs {
        let mut full = vec!["na", "chat"];
        full.extend(args);
        CliArgs::parse_from(full)
    }

    #[test]
    fn resolve_security_mode_cli_override_precedence() {
        let args = parse_chat(&["--mode", "confirm"]);
        let config = Config::default();
        let mode = resolve_security_mode(args.mode(), &config);
        assert_eq!(mode, SecurityMode::Confirm);
    }

    #[test]
    fn resolve_security_mode_invalid_cli_falls_back_to_config() {
        let args = parse_chat(&["--mode", "bogus"]);
        let config = Config::default();
        let mode = resolve_security_mode(args.mode(), &config);
        assert_eq!(mode, SecurityMode::Direct);
    }

    #[test]
    fn resolve_security_mode_no_cli_uses_config_mode() {
        let args = parse_chat(&[]);
        let mut config = Config::default();
        config.security.mode = "whitelist".to_string();
        let mode = resolve_security_mode(args.mode(), &config);
        assert_eq!(mode, SecurityMode::Whitelist);
    }

    #[test]
    fn load_config_nonexistent_returns_default() {
        let path = std::path::Path::new("/tmp/does_not_exist_na_test_config_99999.toml");
        let config = load_config(path);
        assert_eq!(config.provider.provider, Some("openai".to_string()));
        assert_eq!(config.provider.model, Some("gpt-4o-mini".to_string()));
        assert_eq!(config.provider.temperature, 0.7);
        assert!(config.memory.enabled);
        assert_eq!(config.security.mode, "direct");
        assert_eq!(config.behavior.max_iterations, 10);
        assert!(config.behavior.streaming);
    }

    #[test]
    fn load_config_invalid_toml_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("bad_config.toml");
        std::fs::write(&config_path, "this is not valid toml {{{").unwrap();
        let config = load_config(&config_path);
        assert_eq!(config.provider.provider, Some("openai".to_string()));
        assert_eq!(config.provider.model, Some("gpt-4o-mini".to_string()));
        assert_eq!(config.provider.temperature, 0.7);
        assert!(config.memory.enabled);
    }

    #[test]
    fn resolve_api_key_config_key_takes_precedence() {
        let mut config = Config::default();
        config.provider.api_key = Some("config-key".to_string());
        std::env::set_var("NA_API_KEY", "env-key");
        let key = resolve_api_key(&config, &["NA_API_KEY"]);
        std::env::remove_var("NA_API_KEY");
        assert_eq!(key, Some("config-key".to_string()));
    }

    #[test]
    fn resolve_api_key_falls_back_to_env_var() {
        let config = Config::default();
        std::env::set_var("NA_API_KEY", "env-key-fallback");
        let key = resolve_api_key(&config, &["NA_API_KEY"]);
        std::env::remove_var("NA_API_KEY");
        assert_eq!(key, Some("env-key-fallback".to_string()));
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
    fn cli_chat_config_flag() {
        let args = parse_chat(&["--config"]);
        assert!(args.is_config_flag());
    }

    #[test]
    fn cli_chat_config_path_flag() {
        let args = parse_chat(&["--config-path", "/tmp/test.toml"]);
        assert_eq!(args.config_path(), std::path::PathBuf::from("/tmp/test.toml"));
    }
}

use std::io::{self, Write};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use crate::agent::turn_streamed_to_stdout;
use crate::agent::Agent;
use crate::config::schema::{Config, default_config_path};
use crate::providers::Provider;
use crate::security::{SecurityManager, SecurityMode};
use crate::tools;

use super::CliArgs;

pub async fn run(args: CliArgs) -> anyhow::Result<()> {
    if args.config {
        return open_config_editor(args.config_path());
    }

    let config = load_config(&args.config_path());
    let security_mode = resolve_security_mode(&args, &config);
    let streaming = config.behavior.streaming;

    if args.verbose {
        eprintln!(
            "[cli] config loaded, security mode: {}",
            security_mode
        );
    }

    let provider = build_provider(&config)?;
    let agent = build_agent(provider, &config, security_mode);

    match args.prompt_text() {
        Some(prompt) => run_single(agent, &prompt, streaming).await,
        None => run_interactive(agent, streaming).await,
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

fn resolve_security_mode(args: &CliArgs, config: &Config) -> SecurityMode {
    match &args.mode {
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

fn build_agent(provider: Arc<dyn Provider>, config: &Config, _security_mode: SecurityMode) -> Agent {
    let raw_tools = tools::default_tools();
    let secured_tools: Vec<Box<dyn crate::tools::Tool>> = raw_tools
        .into_iter()
        .map(|t| {
            let sec_mgr = SecurityManager::from_config_with_override(
                &config.security,
                Some(_security_mode),
            );
            Box::new(crate::security::SecureTool::new(t, Arc::new(sec_mgr)))
                as Box<dyn crate::tools::Tool>
        })
        .collect();

    let memory: Option<Arc<dyn crate::memory::Memory>> = if config.memory.enabled {
        let memory_dir = default_config_path()
            .parent()
            .unwrap_or(Path::new("."))
            .join("memory");
        Some(Arc::new(crate::memory::MarkdownMemory::new(memory_dir)))
    } else {
        None
    };

    Agent::new(provider, secured_tools, memory, config.clone())
}

async fn run_single(mut agent: Agent, prompt: &str, streaming: bool) -> anyhow::Result<()> {
    if streaming {
        let result = turn_streamed_to_stdout(&mut agent, prompt).await?;
        if result.tool_calls_count > 0 {
            eprintln!("[cli] {} tool call(s) executed", result.tool_calls_count);
        }
        Ok(())
    } else {
        match agent.turn(prompt).await {
            Ok(result) => {
                println!("{}", result.response);
                if result.tool_calls_count > 0 {
                    eprintln!("[cli] {} tool call(s) executed", result.tool_calls_count);
                }
                Ok(())
            }
            Err(e) => {
                eprintln!("[cli] error: {e:#}");
                Err(e)
            }
        }
    }
}

async fn run_interactive(mut agent: Agent, streaming: bool) -> anyhow::Result<()> {
    println!("nano-assistant v{}", env!("CARGO_PKG_VERSION"));
    println!("Type your prompt and press Enter. Type `exit`, `quit`, or Ctrl+D to quit.\n");

    let mut stdin_line = String::new();
    loop {
        print!("❯ ");
        io::stderr().flush().map_err(|e| anyhow::anyhow!("{e}"))?;
        io::stdout().flush().map_err(|e| anyhow::anyhow!("{e}"))?;

        stdin_line.clear();
        match io::stdin().read_line(&mut stdin_line) {
            Ok(0) => {
                println!();
                break;
            }
            Ok(_) => {}
            Err(e) => return Err(e.into()),
        }

        let line = stdin_line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "exit" || line == "quit" {
            break;
        }
        if line == "clear" {
            agent.clear_history();
            eprintln!("[cli] conversation history cleared");
            continue;
        }

        if streaming {
            match turn_streamed_to_stdout(&mut agent, line).await {
                Ok(result) => {
                    if result.tool_calls_count > 0 {
                        eprintln!(
                            "[cli] {} tool call(s) executed",
                            result.tool_calls_count
                        );
                    }
                }
                Err(e) => {
                    eprintln!("[cli] error: {e:#}");
                }
            }
        } else {
            match agent.turn(line).await {
                Ok(result) => {
                    println!("{}", result.response);
                    if result.tool_calls_count > 0 {
                        eprintln!(
                            "[cli] {} tool call(s) executed",
                            result.tool_calls_count
                        );
                    }
                }
                Err(e) => {
                    eprintln!("[cli] error: {e:#}");
                }
            }
        }
    }

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::Config;
    use crate::security::SecurityMode;
    use clap::Parser;

    #[test]
    fn resolve_security_mode_cli_override_precedence() {
        let args = CliArgs::parse_from(["na", "--mode", "confirm"]);
        let config = Config::default();
        assert_eq!(resolve_security_mode(&args, &config), SecurityMode::Confirm);
    }

    #[test]
    fn resolve_security_mode_invalid_cli_falls_back_to_config() {
        let args = CliArgs::parse_from(["na", "--mode", "bogus"]);
        let config = Config::default();
        assert_eq!(resolve_security_mode(&args, &config), SecurityMode::Direct);
    }

    #[test]
    fn resolve_security_mode_no_cli_uses_config_mode() {
        let args = CliArgs::parse_from(["na"]);
        let mut config = Config::default();
        config.security.mode = "whitelist".to_string();
        assert_eq!(resolve_security_mode(&args, &config), SecurityMode::Whitelist);
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
        let args = CliArgs::parse_from(["na"]);
        assert!(args.prompt_text().is_none());
    }

    #[test]
    fn prompt_text_single_word_returns_some() {
        let args = CliArgs::parse_from(["na", "hello"]);
        assert_eq!(args.prompt_text(), Some("hello".to_string()));
    }

    #[test]
    fn prompt_text_multiple_words_joined_with_space() {
        let args = CliArgs::parse_from(["na", "hello", "world"]);
        assert_eq!(args.prompt_text(), Some("hello world".to_string()));
    }
}

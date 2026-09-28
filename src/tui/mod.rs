use std::io::{self, Write};
use std::path::{Path, PathBuf};

use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use crate::agent::{turn_streamed_to_stdout, Agent};
use crate::config::schema::Config;
use crate::hub::{maybe_render_ad, model_routes_via_hub};

const PROMPT: &str = "❯ ";

pub async fn run_tui(
    mut agent: Agent,
    streaming: bool,
    history_path: PathBuf,
    config: Config,
    config_path: PathBuf,
) -> anyhow::Result<()> {
    print_welcome();
    render_startup_ad(&config, &config_path).await;

    let mut editor = DefaultEditor::new()?;
    load_history(&mut editor, &history_path);

    loop {
        match editor.readline(PROMPT) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                let _ = editor.add_history_entry(line);
                save_history(&mut editor, &history_path);

                match handle_inline_command(&mut agent, line) {
                    InlineCommandResult::Handled => continue,
                    InlineCommandResult::Quit => break,
                    InlineCommandResult::Rescan => {
                        match rescan_system_info().await {
                            Ok(tool_count) => {
                                println!(
                                    "{}",
                                    accent(&format!(
                                        "✓ System info updated ({} tools detected)",
                                        tool_count
                                    ))
                                );
                            }
                            Err(e) => {
                                eprintln!("[tui] rescan failed: {e}");
                            }
                        }
                        continue;
                    }
                    InlineCommandResult::Prompt(prompt) => {
                        run_prompt(&mut agent, prompt, streaming, &config, &config_path).await?;
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!();
                break;
            }
            Err(ReadlineError::Eof) => {
                println!();
                break;
            }
            Err(err) => return Err(err.into()),
        }
    }

    Ok(())
}

async fn run_prompt(
    agent: &mut Agent,
    prompt: &str,
    streaming: bool,
    config: &Config,
    config_path: &Path,
) -> anyhow::Result<()> {
    if streaming {
        let result = turn_streamed_to_stdout(agent, prompt).await?;
        if result.tool_calls_count > 0 {
            eprintln!(
                "{}",
                crate::console::format_tool_summary(result.tool_calls_count)
            );
        }
        render_inline_ad(config, config_path).await;
    } else {
        match agent.turn(prompt).await {
            Ok(result) => {
                crate::render::render_markdown_to_stdout(&result.response);
                if result.tool_calls_count > 0 {
                    eprintln!(
                        "{}",
                        crate::console::format_tool_summary(result.tool_calls_count)
                    );
                }
                render_inline_ad(config, config_path).await;
            }
            Err(e) => {
                eprintln!("[cli] error: {e:#}");
            }
        }
    }

    Ok(())
}

enum InlineCommandResult<'a> {
    Handled,
    Quit,
    Prompt(&'a str),
    Rescan,
}

fn handle_inline_command<'a>(agent: &mut Agent, line: &'a str) -> InlineCommandResult<'a> {
    match line {
        "exit" | "quit" | "/exit" | "/quit" => InlineCommandResult::Quit,
        "clear" | "/clear" => {
            agent.clear_history();
            clear_terminal();
            println!("{}", dim("conversation history cleared"));
            InlineCommandResult::Handled
        }
        "/help" => {
            print_help();
            InlineCommandResult::Handled
        }
        "rescan" | "/rescan" => InlineCommandResult::Rescan,
        "memory" | "/memory" => {
            match std::fs::read_to_string(crate::cli::commands::memory_md_path()) {
                Ok(content) => {
                    crate::render::render_markdown_to_stdout(&content);
                    println!();
                }
                Err(_) => {
                    println!("{}", dim("No MEMORY.md found. Use /rescan to create one."));
                }
            }
            InlineCommandResult::Handled
        }
        cmd if cmd.starts_with('/') => {
            println!("{}", warn(&format!("unknown command: {cmd}")));
            println!("{}", dim("type /help to see available commands"));
            InlineCommandResult::Handled
        }
        prompt => InlineCommandResult::Prompt(prompt),
    }
}

fn load_history(editor: &mut DefaultEditor, history_path: &Path) {
    if history_path.exists() {
        let _ = editor.load_history(history_path);
    }
}

fn save_history(editor: &mut DefaultEditor, history_path: &Path) {
    if let Some(parent) = history_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = editor.save_history(history_path);
}

fn clear_terminal() {
    print!("\x1b[2J\x1b[H");
    let _ = io::stdout().flush();
}

fn print_welcome() {
    println!("nano-assistant v{}", env!("CARGO_PKG_VERSION"));
    println!(
        "{}",
        dim("Simple interactive mode • full terminal scrollback preserved • Ctrl+C/Ctrl+D to quit")
    );
    println!(
        "{}",
        dim("Commands: /help /clear /rescan /memory /exit /quit")
    );
    println!();
}

fn print_help() {
    println!("{}", accent("Available commands"));
    println!(
        "  {}  clear the conversation history and screen",
        accent("/clear")
    );
    println!(
        "  {}   re-detect system info and update MEMORY.md",
        accent("/rescan")
    );
    println!("  {}  show current MEMORY.md contents", accent("/memory"));
    println!("  {}   show this help", accent("/help"));
    println!("  {}   quit interactive mode", accent("/exit"));
    println!("  {}   quit interactive mode", accent("/quit"));
    println!(
        "{}",
        dim("Prompt history is available with ↑ / ↓ and is saved to the local history file.")
    );
    println!();
}

fn accent(text: &str) -> String {
    format!("\x1b[1;36m{text}\x1b[0m")
}

fn dim(text: &str) -> String {
    format!("\x1b[2m{text}\x1b[0m")
}

fn warn(text: &str) -> String {
    format!("\x1b[33m{text}\x1b[0m")
}

async fn render_inline_ad(config: &Config, config_path: &Path) {
    if !model_routes_via_hub(config) {
        return;
    }

    if let Err(error) = maybe_render_ad(config_path.to_path_buf(), "inline_after_response").await {
        eprintln!("[hub] ad fetch skipped: {error}");
    }
}

async fn render_startup_ad(config: &Config, config_path: &Path) {
    if !model_routes_via_hub(config) {
        return;
    }

    if let Err(error) = maybe_render_ad(config_path.to_path_buf(), "startup").await {
        eprintln!("[hub] startup ad skipped: {error}");
    }
}

async fn rescan_system_info() -> anyhow::Result<usize> {
    let info = crate::system_info::detect().await;
    let path = crate::cli::commands::memory_md_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let content = crate::memory::MarkdownMemory::upsert_system_info_markdown(
        &existing,
        &info.format_as_markdown(),
    );

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &content)?;

    Ok(info.installed_tools.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::{SecurityManager, SecurityMode};
    use std::sync::Arc;

    async fn create_test_agent() -> Agent {
        let mut config = Config::default();
        config.provider.provider = Some("compatible".to_string());
        config.provider.api_url = Some("http://localhost:8080/v1".to_string());
        let model = crate::providers::build_model(&config, Path::new("")).expect("model build");
        Agent::new(
            model,
            vec![],
            None,
            config,
            vec![],
            None,
            Arc::new(SecurityManager::new(SecurityMode::Direct)),
            PathBuf::from("config.toml"),
        )
        .await
    }

    #[tokio::test]
    async fn slash_quit_is_recognized() {
        let mut agent = create_test_agent().await;
        assert!(matches!(
            handle_inline_command(&mut agent, "/quit"),
            InlineCommandResult::Quit
        ));
        assert!(matches!(
            handle_inline_command(&mut agent, "quit"),
            InlineCommandResult::Quit
        ));
        assert!(matches!(
            handle_inline_command(&mut agent, "/exit"),
            InlineCommandResult::Quit
        ));
        assert!(matches!(
            handle_inline_command(&mut agent, "exit"),
            InlineCommandResult::Quit
        ));
    }

    #[tokio::test]
    async fn unknown_slash_command_is_handled() {
        let mut agent = create_test_agent().await;
        assert!(matches!(
            handle_inline_command(&mut agent, "/wat"),
            InlineCommandResult::Handled
        ));
    }

    #[tokio::test]
    async fn plain_prompt_is_passed_through() {
        let mut agent = create_test_agent().await;
        match handle_inline_command(&mut agent, "hello") {
            InlineCommandResult::Prompt(prompt) => assert_eq!(prompt, "hello"),
            _ => panic!("expected prompt passthrough"),
        }
    }

    #[tokio::test]
    async fn rescan_is_recognized() {
        let mut agent = create_test_agent().await;
        assert!(matches!(
            handle_inline_command(&mut agent, "/rescan"),
            InlineCommandResult::Rescan
        ));
        assert!(matches!(
            handle_inline_command(&mut agent, "rescan"),
            InlineCommandResult::Rescan
        ));
    }

    #[tokio::test]
    async fn memory_command_is_recognized() {
        let mut agent = create_test_agent().await;
        assert!(matches!(
            handle_inline_command(&mut agent, "/memory"),
            InlineCommandResult::Handled
        ));
        assert!(matches!(
            handle_inline_command(&mut agent, "memory"),
            InlineCommandResult::Handled
        ));
    }

    #[tokio::test]
    async fn clear_command_is_recognized() {
        let mut agent = create_test_agent().await;
        assert!(matches!(
            handle_inline_command(&mut agent, "/clear"),
            InlineCommandResult::Handled
        ));
        assert!(matches!(
            handle_inline_command(&mut agent, "clear"),
            InlineCommandResult::Handled
        ));
    }

    #[tokio::test]
    async fn help_command_is_recognized() {
        let mut agent = create_test_agent().await;
        assert!(matches!(
            handle_inline_command(&mut agent, "/help"),
            InlineCommandResult::Handled
        ));
    }
}

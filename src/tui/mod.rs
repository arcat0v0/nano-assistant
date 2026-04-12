use std::io::{self, Write};
use std::path::{Path, PathBuf};

use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

use crate::agent::{Agent, turn_streamed_to_stdout};

const PROMPT: &str = "❯ ";

pub async fn run_tui(mut agent: Agent, streaming: bool, history_path: PathBuf) -> anyhow::Result<()> {
    print_welcome();

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
                    InlineCommandResult::Prompt(prompt) => {
                        run_prompt(&mut agent, prompt, streaming).await?;
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

async fn run_prompt(agent: &mut Agent, prompt: &str, streaming: bool) -> anyhow::Result<()> {
    if streaming {
        let result = turn_streamed_to_stdout(agent, prompt).await?;
        if result.tool_calls_count > 0 {
            eprintln!("{}", crate::console::format_tool_summary(result.tool_calls_count));
        }
    } else {
        match agent.turn(prompt).await {
            Ok(result) => {
                println!("{}", result.response);
                if result.tool_calls_count > 0 {
                    eprintln!("{}", crate::console::format_tool_summary(result.tool_calls_count));
                }
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
    println!("{}", dim("Commands: /help /clear /exit /quit"));
    println!();
}

fn print_help() {
    println!("{}", accent("Available commands"));
    println!("  {}  clear the conversation history and screen", accent("/clear"));
    println!("  {}   show this help", accent("/help"));
    println!("  {}   quit interactive mode", accent("/exit"));
    println!("  {}   quit interactive mode", accent("/quit"));
    println!("{}", dim("Prompt history is available with ↑ / ↓ and is saved to the local history file."));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_quit_is_recognized() {
        let provider = std::sync::Arc::new(crate::providers::compatible::CompatibleProvider::new(
            "compatible",
            "http://localhost:8080/v1",
            None,
            Some("http://localhost:8080/v1"),
        ));
        let agent = crate::agent::Agent::new(provider, vec![], None, crate::config::schema::Config::default());
        let mut agent = agent;
        assert!(matches!(handle_inline_command(&mut agent, "/quit"), InlineCommandResult::Quit));
    }

    #[test]
    fn unknown_slash_command_is_handled() {
        let provider = std::sync::Arc::new(crate::providers::compatible::CompatibleProvider::new(
            "compatible",
            "http://localhost:8080/v1",
            None,
            Some("http://localhost:8080/v1"),
        ));
        let agent = crate::agent::Agent::new(provider, vec![], None, crate::config::schema::Config::default());
        let mut agent = agent;
        assert!(matches!(handle_inline_command(&mut agent, "/wat"), InlineCommandResult::Handled));
    }

    #[test]
    fn plain_prompt_is_passed_through() {
        let provider = std::sync::Arc::new(crate::providers::compatible::CompatibleProvider::new(
            "compatible",
            "http://localhost:8080/v1",
            None,
            Some("http://localhost:8080/v1"),
        ));
        let agent = crate::agent::Agent::new(provider, vec![], None, crate::config::schema::Config::default());
        let mut agent = agent;
        match handle_inline_command(&mut agent, "hello") {
            InlineCommandResult::Prompt(prompt) => assert_eq!(prompt, "hello"),
            _ => panic!("expected prompt passthrough"),
        }
    }
}

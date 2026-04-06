pub mod commands;

use clap::Parser;
use std::path::PathBuf;

/// nano-assistant — a lightweight AI assistant.
///
/// # Modes
///
/// * **Single command**: `na "deploy nginx"` — run one prompt and exit.
/// * **Interactive**: `na` — enter a REPL loop until you type `exit` or `quit`.
/// * **Config edit**: `na --config` — open the config file in $EDITOR.
///
/// # Security mode
///
/// Override the security mode from config.toml with `--mode`:
///
/// ```bash
/// na --mode direct  "rm -rf /tmp/old"   # no confirmation
/// na --mode confirm "rm -rf /tmp/old"   # confirm each tool call
/// na --mode whitelist "ls /etc"         # only whitelisted commands
/// ```
#[derive(Parser, Debug)]
#[command(name = "na")]
#[command(author = "nano-assistant team")]
#[command(version)]
#[command(about = "A lightweight AI assistant", long_about = None)]
#[command(disable_help_subcommand = true)]
pub struct CliArgs {
    /// The prompt to execute. If omitted, enters interactive mode.
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,

    /// Override the security mode from config.toml.
    /// Valid values: direct, confirm, whitelist.
    #[arg(long, value_name = "MODE")]
    pub mode: Option<String>,

    /// Open the configuration file in $EDITOR (falls back to nano, then vim).
    #[arg(long)]
    pub config: bool,

    /// Path to the configuration file.
    /// Default: ~/.config/nano-assistant/config.toml
    #[arg(long, value_name = "PATH")]
    pub config_path: Option<PathBuf>,

    /// Verbose output.
    #[arg(short, long)]
    pub verbose: bool,
}

impl CliArgs {
    /// Return the prompt as a single string, or None for interactive mode.
    pub fn prompt_text(&self) -> Option<String> {
        if self.prompt.is_empty() {
            None
        } else {
            Some(self.prompt.join(" "))
        }
    }

    /// Resolve the config file path.
    pub fn config_path(&self) -> PathBuf {
        self.config_path
            .clone()
            .unwrap_or_else(crate::config::schema::default_config_path)
    }
}

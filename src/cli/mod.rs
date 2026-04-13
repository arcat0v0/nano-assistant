pub mod commands;

use clap::Parser;
use std::path::PathBuf;

#[derive(clap::Subcommand, Debug, Clone)]
pub enum SkillsSubcommand {
    List,
    Install { source: String },
    Remove { name: String },
    Audit { source: String },
    Test { name: Option<String> },
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum Commands {
    #[command(trailing_var_arg = true)]
    Chat {
        #[arg(trailing_var_arg = true)]
        prompt: Vec<String>,
        #[arg(long, value_name = "MODE")]
        mode: Option<String>,
        #[arg(long)]
        debug: bool,
        #[arg(long)]
        config: bool,
        #[arg(long, value_name = "PATH")]
        config_path: Option<PathBuf>,
        #[arg(short, long)]
        verbose: bool,
    },
    Skills {
        #[command(subcommand)]
        action: SkillsSubcommand,
    },
}

#[derive(Parser, Debug)]
#[command(name = "na")]
#[command(author = "nano-assistant team")]
#[command(version)]
#[command(about = "A lightweight AI assistant", long_about = None)]
#[command(disable_help_subcommand = true)]
pub struct CliArgs {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

impl CliArgs {
    pub fn prompt_text(&self) -> Option<String> {
        match &self.command {
            Some(Commands::Chat { prompt, .. }) => {
                if prompt.is_empty() {
                    None
                } else {
                    Some(prompt.join(" "))
                }
            }
            _ => None,
        }
    }

    pub fn mode(&self) -> Option<&str> {
        match &self.command {
            Some(Commands::Chat { mode, .. }) => mode.as_deref(),
            _ => None,
        }
    }

    pub fn config_path(&self) -> PathBuf {
        match &self.command {
            Some(Commands::Chat { config_path, .. }) => config_path
                .clone()
                .unwrap_or_else(crate::config::schema::default_config_path),
            _ => crate::config::schema::default_config_path(),
        }
    }

    pub fn is_config_flag(&self) -> bool {
        matches!(&self.command, Some(Commands::Chat { config: true, .. }))
    }

    pub fn is_verbose(&self) -> bool {
        matches!(&self.command, Some(Commands::Chat { verbose: true, .. }))
    }

    pub fn is_debug(&self) -> bool {
        matches!(&self.command, Some(Commands::Chat { debug: true, .. }))
    }
}

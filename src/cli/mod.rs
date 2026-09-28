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
pub enum HubSubcommand {
    Status {
        #[arg(long, value_name = "PATH")]
        config_path: Option<PathBuf>,
    },
    Register {
        #[arg(long, value_name = "PATH")]
        config_path: Option<PathBuf>,
    },
    Disable {
        #[arg(long, value_name = "PATH")]
        config_path: Option<PathBuf>,
    },
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum IdentitySubcommand {
    Export {
        path: PathBuf,
        #[arg(long, value_name = "PATH")]
        config_path: Option<PathBuf>,
    },
    Import {
        path: PathBuf,
        #[arg(long, value_name = "PATH")]
        config_path: Option<PathBuf>,
    },
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum ModelSubcommand {
    List,
    Add {
        name: String,
        #[arg(long)]
        provider: String,
        #[arg(long)]
        model: String,
        #[arg(long)]
        api_url: Option<String>,
        #[arg(long)]
        api_key_env: Option<String>,
    },
    Remove {
        name: String,
    },
    Use {
        name: String,
    },
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum Commands {
    Chat {
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
        #[arg(long, conflicts_with = "model")]
        profile: Option<String>,
        #[arg(long, conflicts_with = "profile")]
        model: Option<String>,
        #[arg(long, requires = "model")]
        provider: Option<String>,
    },
    Skills {
        #[command(subcommand)]
        action: SkillsSubcommand,
    },
    Hub {
        #[command(subcommand)]
        action: HubSubcommand,
    },
    Identity {
        #[command(subcommand)]
        action: IdentitySubcommand,
    },
    Model {
        #[arg(long, value_name = "PATH")]
        config_path: Option<PathBuf>,
        #[command(subcommand)]
        action: ModelSubcommand,
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

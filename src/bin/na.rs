use clap::Parser;
use nano_assistant::cli::{CliArgs, commands};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = CliArgs::parse();
    commands::run(args).await
}

use clap::Parser;
use nano_assistant::cli::{CliArgs, commands};

fn is_bare_global_flag(arg: &str) -> bool {
    matches!(arg, "--help" | "-h" | "--version" | "-V")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let raw: Vec<String> = std::env::args().collect();

    let args = match CliArgs::try_parse() {
        Ok(args) => args,
        Err(e) => {
            // Global flags (--help/--version) or "skills" subcommand must not
            // fall through to the chat-mode backward-compat path.
            if raw.get(1).is_some_and(|s| is_bare_global_flag(s) || s == "skills") {
                eprintln!("{e}");
                std::process::exit(e.exit_code());
            }
            // Backward compat: `na "prompt"` → `na chat "prompt"`
            let mut with_chat = vec![raw[0].clone(), "chat".to_string()];
            with_chat.extend_from_slice(&raw[1..]);
            CliArgs::parse_from(with_chat)
        }
    };
    commands::run(args).await
}

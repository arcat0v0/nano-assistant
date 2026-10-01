use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;

use rustyline::completion::Completer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::{Hint, Hinter};
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{
    Cmd, ConditionalEventHandler, Context, Editor, Event, EventContext, EventHandler, Helper,
    KeyCode, KeyEvent, Modifiers, RepeatCount,
};

use crossterm::{
    event::{self, Event as TerminalEvent, KeyCode as TerminalKeyCode, KeyEventKind, KeyModifiers},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode},
};

use crate::agent::{turn_streamed_to_stdout, Agent};
use crate::config::credentials::{deepseek_key_path, save_deepseek_key};
use crate::config::models::{
    add_profile, list_profiles, remove_profile, resolve_selection, set_default_profile,
};
use crate::config::schema::{Config, RuntimeApiKey};
use crate::config::ModelProfile;
use crate::hub::{maybe_render_ad, model_routes_via_hub};
use crate::security::SecurityMode;
use rig::agent::model::ModelHandle;

mod onboarding;
mod resources;

const PROMPT: &str = "❯ ";

type TuiEditor = Editor<PaletteHelper, DefaultHistory>;

const SLASH_COMMANDS: &[&str] = &[
    "/model", "/memory", "/help", "/clear", "/rescan", "/exit", "/quit",
];

#[derive(Clone)]
enum ModelMenuAction {
    Switch(String),
    Add,
    Back,
}

struct ModelMenuOption {
    label: String,
    action: ModelMenuAction,
}

#[derive(Clone, Copy)]
enum PaletteSelection {
    Slash(&'static str),
    Menu(usize),
}

#[derive(Default)]
struct PaletteState {
    filter: String,
    model_label: String,
    selected: usize,
    menu: Option<Vec<ModelMenuOption>>,
    accepted: Option<PaletteSelection>,
    active: bool,
}

impl PaletteState {
    fn synchronize(&mut self, line: &str) {
        if self.filter != line {
            self.filter.clear();
            self.filter.push_str(line);
            self.selected = 0;
        }
    }

    fn selected_command(&self, line: &str) -> Option<&'static str> {
        SLASH_COMMANDS
            .iter()
            .copied()
            .filter(|command| command.starts_with(line))
            .nth(self.selected)
    }

    fn count(&mut self, line: &str) -> usize {
        if let Some(menu) = &self.menu {
            menu.len()
        } else {
            self.synchronize(line);
            SLASH_COMMANDS
                .iter()
                .filter(|command| command.starts_with(line))
                .count()
        }
    }
}

fn append_palette_entry(text: &mut String, label: &str, selected: bool) {
    text.push_str(if selected { "\n  > " } else { "\n    " });
    text.push_str(label);
}

struct PaletteHelper {
    state: Arc<Mutex<PaletteState>>,
}

struct PaletteHint(String);

impl Hint for PaletteHint {
    fn display(&self) -> &str {
        &self.0
    }

    fn completion(&self) -> Option<&str> {
        None
    }
}

impl Hinter for PaletteHelper {
    type Hint = PaletteHint;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<Self::Hint> {
        let mut state = self.state.lock();
        if !state.active || pos != line.len() {
            return None;
        }
        let mut text = String::new();
        if let Some(menu) = &state.menu {
            if !line.is_empty() {
                return None;
            }
            for (index, option) in menu.iter().enumerate() {
                append_palette_entry(&mut text, &option.label, state.selected == index);
            }
        } else if !line.starts_with('/') {
            text.push_str("\n  ");
            text.push_str(&state.model_label);
        } else if line.starts_with('/') && !line.contains(char::is_whitespace) {
            state.synchronize(line);
            for (index, command) in SLASH_COMMANDS
                .iter()
                .copied()
                .filter(|command| command.starts_with(line))
                .enumerate()
            {
                append_palette_entry(&mut text, command, state.selected == index);
            }
        }
        (!text.is_empty()).then_some(PaletteHint(text))
    }
}

impl Completer for PaletteHelper {
    type Candidate = String;
}

impl Highlighter for PaletteHelper {}
impl Validator for PaletteHelper {}
impl Helper for PaletteHelper {}

enum PaletteKey {
    Up,
    Down,
    Right,
    Enter,
}

struct PaletteBinding {
    state: Arc<Mutex<PaletteState>>,
    key: PaletteKey,
}

impl ConditionalEventHandler for PaletteBinding {
    fn handle(
        &self,
        _event: &Event,
        _repeat: RepeatCount,
        _positive: bool,
        ctx: &EventContext<'_>,
    ) -> Option<Cmd> {
        let line = ctx.line();
        if ctx.pos() != line.len() {
            return None;
        }
        let mut state = self.state.lock();
        if !state.active {
            return None;
        }
        if state.menu.is_some() {
            if !line.is_empty() {
                return None;
            }
            match self.key {
                PaletteKey::Up | PaletteKey::Down => {
                    let count = state.count(line);
                    state.selected = if matches!(self.key, PaletteKey::Up) {
                        (state.selected + count - 1) % count
                    } else {
                        (state.selected + 1) % count
                    };
                    Some(Cmd::Repaint)
                }
                PaletteKey::Enter => {
                    state.accepted = Some(PaletteSelection::Menu(state.selected));
                    Some(Cmd::AcceptLine)
                }
                PaletteKey::Right => None,
            }
        } else if line.starts_with('/') && !line.contains(char::is_whitespace) {
            let count = state.count(line);
            if count == 0 {
                return None;
            }
            match self.key {
                PaletteKey::Up => {
                    state.selected = (state.selected + count - 1) % count;
                    Some(Cmd::Repaint)
                }
                PaletteKey::Down => {
                    state.selected = (state.selected + 1) % count;
                    Some(Cmd::Repaint)
                }
                PaletteKey::Right => {
                    let command = state.selected_command(line).unwrap();
                    let suffix = &command[line.len()..];
                    if suffix.is_empty() {
                        Some(Cmd::Noop)
                    } else {
                        Some(Cmd::Insert(1, suffix.to_string()))
                    }
                }
                PaletteKey::Enter => {
                    state.accepted = Some(PaletteSelection::Slash(
                        state.selected_command(line).unwrap(),
                    ));
                    Some(Cmd::AcceptLine)
                }
            }
        } else {
            None
        }
    }
}

fn bind_palette_keys(editor: &mut TuiEditor, state: &Arc<Mutex<PaletteState>>) {
    for (key, action) in [
        (KeyCode::Up, PaletteKey::Up),
        (KeyCode::Down, PaletteKey::Down),
        (KeyCode::Right, PaletteKey::Right),
        (KeyCode::Enter, PaletteKey::Enter),
    ] {
        editor.bind_sequence(
            KeyEvent(key, Modifiers::NONE),
            EventHandler::Conditional(Box::new(PaletteBinding {
                state: Arc::clone(state),
                key: action,
            })),
        );
    }
    editor.bind_sequence(
        KeyEvent::from('\n'),
        EventHandler::Conditional(Box::new(PaletteBinding {
            state: Arc::clone(state),
            key: PaletteKey::Enter,
        })),
    );
}

fn model_menu_options(catalog: &Config, current: &Config) -> Vec<ModelMenuOption> {
    let mut options = vec![ModelMenuOption {
        label: format!(
            "default: {}/{}{}",
            catalog.provider.provider.as_deref().unwrap_or("openai"),
            catalog.provider.model.as_deref().unwrap_or("gpt-4o-mini"),
            if current.active_profile.is_none() {
                " (current)"
            } else {
                ""
            }
        ),
        action: ModelMenuAction::Switch("default".to_string()),
    }];
    for (name, profile) in list_profiles(catalog) {
        options.push(ModelMenuOption {
            label: format!(
                "{name}: {}/{}{}",
                profile.provider,
                profile.model,
                if current.active_profile.as_deref() == Some(name) {
                    " (current)"
                } else {
                    ""
                }
            ),
            action: ModelMenuAction::Switch(name.to_string()),
        });
    }
    options.push(ModelMenuOption {
        label: "Add provider".to_string(),
        action: ModelMenuAction::Add,
    });
    options.push(ModelMenuOption {
        label: "Back".to_string(),
        action: ModelMenuAction::Back,
    });
    options
}

fn choose_model_menu(
    editor: &mut TuiEditor,
    state: &Arc<Mutex<PaletteState>>,
    catalog: &Config,
    current: &Config,
) -> anyhow::Result<ModelMenuAction> {
    let options = model_menu_options(catalog, current);
    {
        let mut palette = state.lock();
        palette.menu = Some(options);
        palette.selected = 0;
        palette.accepted = None;
        palette.active = true;
    }
    let result = editor.readline("Model ❯ ");
    let mut palette = state.lock();
    let selected = palette.accepted.take();
    let options = palette.menu.take().unwrap();
    palette.active = false;
    palette.filter.clear();
    match result {
        Ok(line) if matches!(line.trim(), "q" | "back" | "/cancel" | "cancel") => {
            Ok(ModelMenuAction::Back)
        }
        Ok(line) if line.trim().is_empty() => match selected {
            Some(PaletteSelection::Menu(index)) => Ok(options[index].action.clone()),
            _ => Ok(ModelMenuAction::Back),
        },
        Ok(_) => {
            println!(
                "{}",
                warn("use ↑ / ↓ and Enter to choose a model action, or q to go back")
            );
            Ok(ModelMenuAction::Back)
        }
        Err(ReadlineError::Interrupted | ReadlineError::Eof) => Ok(ModelMenuAction::Back),
        Err(error) => Err(error.into()),
    }
}

pub async fn run_tui(
    effective_config: Config,
    config_path: PathBuf,
    mut catalog_config: Config,
    selection_label: String,
    history_path: PathBuf,
    security_mode: SecurityMode,
) -> anyhow::Result<()> {
    let mut agent = None;
    let mut current_config = effective_config;
    let mut current_label = selection_label;
    print_logo();

    let palette = Arc::new(Mutex::new(PaletteState::default()));
    let mut editor = TuiEditor::new()?;
    editor.set_helper(Some(PaletteHelper {
        state: Arc::clone(&palette),
    }));
    bind_palette_keys(&mut editor, &palette);
    load_history(&mut editor, &history_path);

    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        if onboarding::has_configured_key(&current_config, &catalog_config, &config_path)? {
            onboarding::attach_saved_key(&mut current_config, &config_path)?;
        } else if !run_deepseek_onboarding(
            &mut editor,
            &mut current_config,
            &mut catalog_config,
            &mut current_label,
            &config_path,
        )
        .await?
        {
            return Ok(());
        }
    }

    loop {
        {
            let mut state = palette.lock();
            state.active = true;
            state.filter.clear();
            state.model_label = current_config
                .provider
                .model
                .as_deref()
                .unwrap_or("gpt-4o-mini")
                .to_owned();
            state.selected = 0;
        }
        match readline_with_resources(&mut editor, PROMPT, &palette) {
            Ok(line) => {
                let accepted = palette.lock().accepted.take();
                palette.lock().active = false;
                let line = match accepted {
                    Some(PaletteSelection::Slash(command)) => command,
                    _ => line.trim(),
                };
                if line.is_empty() {
                    continue;
                }

                let _ = editor.add_history_entry(line);
                save_history(&mut editor, &history_path);
                println!(
                    "  {}",
                    dim(current_config
                        .provider
                        .model
                        .as_deref()
                        .unwrap_or("gpt-4o-mini"))
                );

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
                    InlineCommandResult::Model(command) => {
                        let chosen = if matches!(command, ModelCommand::Menu) {
                            print_models(&catalog_config, &current_config, &current_label);
                            if !io::stdin().is_terminal() {
                                continue;
                            }
                            match choose_model_menu(
                                &mut editor,
                                &palette,
                                &catalog_config,
                                &current_config,
                            ) {
                                Ok(choice) => choice,
                                Err(error) => {
                                    eprintln!("[tui] model menu failed: {error:#}");
                                    continue;
                                }
                            }
                        } else {
                            ModelMenuAction::Back
                        };
                        let command = match &chosen {
                            ModelMenuAction::Switch(profile) => {
                                let save = match read_step(&mut editor, "Save as default? [y/N]: ")
                                {
                                    Ok(Some(answer))
                                        if answer.eq_ignore_ascii_case("y")
                                            || answer.eq_ignore_ascii_case("yes") =>
                                    {
                                        true
                                    }
                                    Ok(Some(answer))
                                        if answer.is_empty()
                                            || answer.eq_ignore_ascii_case("n")
                                            || answer.eq_ignore_ascii_case("no") =>
                                    {
                                        false
                                    }
                                    Ok(None) => continue,
                                    Ok(Some(_)) => {
                                        println!(
                                            "{}",
                                            warn(
                                                "enter y or n to choose whether to save as default"
                                            )
                                        );
                                        continue;
                                    }
                                    Err(error) => {
                                        eprintln!("[tui] model menu failed: {error:#}");
                                        continue;
                                    }
                                };
                                ModelCommand::Switch { profile, save }
                            }
                            ModelMenuAction::Add => ModelCommand::Add(None),
                            ModelMenuAction::Back => command,
                        };
                        match command {
                            ModelCommand::Menu => {}
                            ModelCommand::Switch { profile, save } => {
                                match switch_profile(
                                    &mut agent,
                                    &mut catalog_config,
                                    &mut current_config,
                                    &mut current_label,
                                    &config_path,
                                    security_mode,
                                    profile,
                                    save,
                                )
                                .await
                                {
                                    Ok(()) => println!(
                                        "{}",
                                        accent(&format!("Model switched to {current_label}"))
                                    ),
                                    Err(error) => eprintln!("[tui] model switch failed: {error:#}"),
                                }
                            }
                            ModelCommand::Add(provider) => {
                                match add_model_profile(
                                    &mut editor,
                                    &mut agent,
                                    &mut catalog_config,
                                    &mut current_config,
                                    &mut current_label,
                                    &config_path,
                                    security_mode,
                                    provider,
                                )
                                .await
                                {
                                    Ok(true) => println!(
                                        "{}",
                                        accent(&format!("Model switched to {current_label}"))
                                    ),
                                    Ok(false) => println!("{}", dim("Model setup canceled")),
                                    Err(error) => eprintln!("[tui] model setup failed: {error:#}"),
                                }
                            }
                            ModelCommand::Invalid => {
                                println!(
                                    "{}",
                                    warn(
                                        "usage: /model [profile [--save]] | /model add [provider]"
                                    )
                                )
                            }
                        }
                        continue;
                    }
                    InlineCommandResult::Prompt(prompt) => {
                        if agent.is_none() {
                            match crate::providers::build_model(&current_config, &config_path) {
                                Ok(model) => {
                                    activate_model(
                                        &mut agent,
                                        model,
                                        &current_config,
                                        &config_path,
                                        security_mode,
                                    )
                                    .await;
                                }
                                Err(error) => {
                                    eprintln!("[tui] model unavailable: {error:#}. Set the required API key or use /model add to configure a provider.");
                                    continue;
                                }
                            }
                        }
                        if let Err(error) = run_prompt(
                            agent.as_mut().expect("model just activated"),
                            prompt,
                            current_config.behavior.streaming,
                            &current_config,
                            &config_path,
                        )
                        .await
                        {
                            eprintln!("[tui] prompt failed: {error:#}");
                        }
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

async fn activate_model(
    agent: &mut Option<Agent>,
    model: ModelHandle,
    config: &Config,
    config_path: &Path,
    security_mode: SecurityMode,
) {
    if let Some(agent) = agent {
        agent.switch_model(model, config).await;
    } else {
        let system_info = crate::cli::commands::load_or_create_memory_md().await;
        *agent = Some(
            crate::cli::commands::build_agent(
                model,
                config,
                security_mode,
                None,
                system_info,
                config_path.to_path_buf(),
            )
            .await,
        );
    }
}

async fn switch_profile(
    agent: &mut Option<Agent>,
    catalog_config: &mut Config,
    current_config: &mut Config,
    current_label: &mut String,
    config_path: &Path,
    security_mode: SecurityMode,
    profile: &str,
    save: bool,
) -> anyhow::Result<()> {
    let (selected, label) = resolve_selection(catalog_config, Some(profile), None, None)?;
    let mut effective = current_config.clone();
    effective.provider = selected.provider;
    effective.active_profile = selected.active_profile;
    let model = crate::providers::build_model(&effective, config_path)?;
    if save {
        set_default_profile(config_path, profile)?;
        let default = (profile != "default").then(|| profile.to_string());
        effective.models.default = default.clone();
        catalog_config.models.default = default;
    }
    activate_model(agent, model, &effective, config_path, security_mode).await;
    *current_config = effective;
    *current_label = label;
    Ok(())
}

fn read_step(editor: &mut TuiEditor, prompt: &str) -> anyhow::Result<Option<String>> {
    editor.helper().unwrap().state.lock().active = false;
    match editor.readline(prompt) {
        Ok(answer) if matches!(answer.trim(), "/cancel" | "cancel" | "q") => Ok(None),
        Ok(answer) => Ok(Some(answer.trim().to_string())),
        Err(ReadlineError::Interrupted | ReadlineError::Eof) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn choose_model<'a>(choice: &str, models: &'a [String]) -> anyhow::Result<&'a str> {
    let index: usize = choice
        .parse()
        .map_err(|_| anyhow::anyhow!("enter a model number from 1 to {}", models.len()))?;
    index
        .checked_sub(1)
        .and_then(|index| models.get(index))
        .map(String::as_str)
        .ok_or_else(|| anyhow::anyhow!("enter a model number from 1 to {}", models.len()))
}

fn valid_key_env_name(name: &str) -> bool {
    let mut chars = name.bytes();
    matches!(chars.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && chars.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn choose_provider(choice: &str) -> anyhow::Result<&'static crate::providers::ProviderPreset> {
    let providers = crate::providers::builtin_providers();
    if let Ok(index) = choice.parse::<usize>() {
        return index
            .checked_sub(1)
            .and_then(|index| providers.get(index))
            .ok_or_else(|| {
                anyhow::anyhow!("choose a provider number from 1 to {}", providers.len())
            });
    }
    crate::providers::preset(choice).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown built-in provider '{choice}'; choose from deepseek, kimi, glm, mimo, qwen"
        )
    })
}

async fn add_model_profile(
    editor: &mut TuiEditor,
    agent: &mut Option<Agent>,
    catalog_config: &mut Config,
    current_config: &mut Config,
    current_label: &mut String,
    config_path: &Path,
    security_mode: SecurityMode,
    requested_provider: Option<&str>,
) -> anyhow::Result<bool> {
    let preset = if let Some(id) = requested_provider {
        choose_provider(id)?
    } else {
        println!("Built-in providers:");
        for (index, provider) in crate::providers::builtin_providers().iter().enumerate() {
            println!(
                "  {}) {} ({})",
                index + 1,
                provider.display_name,
                provider.id
            );
        }
        let Some(answer) = read_step(editor, "Provider (number or name, q to cancel): ")? else {
            return Ok(false);
        };
        choose_provider(&answer)?
    };

    let Some(key_input) = read_step(
        editor,
        &format!("API key environment variable [{}]: ", preset.api_key_env),
    )?
    else {
        return Ok(false);
    };
    let key_env = if key_input.is_empty() {
        preset.api_key_env.to_string()
    } else {
        key_input
    };
    if !valid_key_env_name(&key_env) {
        anyhow::bail!("enter an API key environment variable name, not the key itself");
    }
    if std::env::var_os(&key_env).is_none_or(|value| value.is_empty()) {
        anyhow::bail!("set environment variable {key_env} to your API key, then retry /model add; keys are never entered or saved here");
    }
    let Some(url_input) = read_step(
        editor,
        &format!("API URL [{}] (Enter to use default): ", preset.base_url),
    )?
    else {
        return Ok(false);
    };
    let api_url = (!url_input.is_empty()).then_some(url_input);
    println!("Fetching current {} models...", preset.display_name);
    let models =
        crate::providers::discover_models(preset.id, api_url.as_deref(), Some(&key_env)).await?;
    if models.is_empty() {
        anyhow::bail!(
            "the {} catalog returned no models; check your account and endpoint",
            preset.display_name
        );
    }
    if preset.id == "glm" && api_url.is_none() {
        println!("{}", dim("GLM may use its published catalog; listed models are not a guarantee of account access."));
    }
    for (index, model) in models.iter().enumerate() {
        println!("  {}) {model}", index + 1);
    }
    let Some(choice) = read_step(editor, "Model number: ")? else {
        return Ok(false);
    };
    let model_id = choose_model(&choice, &models)?.to_string();
    let Some(alias) = read_step(editor, &format!("Profile name [{}]: ", preset.id))? else {
        return Ok(false);
    };
    let name = if alias.is_empty() {
        preset.id.to_string()
    } else {
        alias
    };
    if catalog_config.models.profiles.contains_key(&name) {
        anyhow::bail!("model profile '{name}' already exists; choose another profile name");
    }
    let Some(save_choice) = read_step(editor, "Save as default? [y/N]: ")? else {
        return Ok(false);
    };
    let save = if save_choice.eq_ignore_ascii_case("y") || save_choice.eq_ignore_ascii_case("yes") {
        true
    } else if save_choice.is_empty()
        || save_choice.eq_ignore_ascii_case("n")
        || save_choice.eq_ignore_ascii_case("no")
    {
        false
    } else {
        anyhow::bail!("enter y or n to choose whether to save as default");
    };

    let profile = ModelProfile {
        provider: preset.id.to_string(),
        model: model_id,
        api_url,
        api_key_env: Some(key_env),
        temperature: None,
        timeout_secs: None,
    };
    let mut updated = catalog_config.clone();
    updated
        .models
        .profiles
        .insert(name.clone(), profile.clone());
    let (selected, _) = resolve_selection(&updated, Some(&name), None, None)?;
    let mut effective = current_config.clone();
    effective
        .models
        .profiles
        .insert(name.clone(), profile.clone());
    effective.provider = selected.provider;
    effective.active_profile = selected.active_profile;
    let model = crate::providers::build_model(&effective, config_path)?;
    add_profile(config_path, &name, profile)?;
    if save {
        if let Err(error) = set_default_profile(config_path, &name) {
            remove_profile(config_path, &name)?;
            return Err(error);
        }
        updated.models.default = Some(name.clone());
        effective.models.default = Some(name.clone());
    }
    activate_model(agent, model, &effective, config_path, security_mode).await;
    *catalog_config = updated;
    *current_config = effective;
    *current_label = name;
    Ok(true)
}

fn print_models(catalog: &Config, current: &Config, label: &str) {
    println!(
        "{}",
        accent(&format!(
            "Current model: {label} ({}/{})",
            current.provider.provider.as_deref().unwrap_or("openai"),
            current.provider.model.as_deref().unwrap_or("gpt-4o-mini")
        ))
    );
    let legacy = &catalog.provider;
    let marker = if label == "default" { "*" } else { " " };
    let default = if catalog.models.default.is_none() {
        " (default)"
    } else {
        ""
    };
    println!(
        " {marker} default: {}/{}{}",
        legacy.provider.as_deref().unwrap_or("openai"),
        legacy.model.as_deref().unwrap_or("gpt-4o-mini"),
        default
    );
    for (name, profile) in list_profiles(catalog) {
        let marker = if current.active_profile.as_deref() == Some(name) {
            "*"
        } else {
            " "
        };
        let default = if catalog.models.default.as_deref() == Some(name) {
            " (default)"
        } else {
            ""
        };
        println!(
            " {marker} {name}: {}/{}{}",
            profile.provider, profile.model, default
        );
    }
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
    Model(ModelCommand<'a>),
}

enum ModelCommand<'a> {
    Menu,
    Add(Option<&'a str>),
    Switch { profile: &'a str, save: bool },
    Invalid,
}

fn parse_model_command(line: &str) -> ModelCommand<'_> {
    let mut args = line.split_whitespace();
    let _ = args.next();
    match (args.next(), args.next(), args.next()) {
        (None, None, None) => ModelCommand::Menu,
        (Some("add"), None, None) => ModelCommand::Add(None),
        (Some("add"), Some(provider), None) if !provider.starts_with('-') => {
            ModelCommand::Add(Some(provider))
        }
        (Some(profile), None, None) if profile != "--save" => ModelCommand::Switch {
            profile,
            save: false,
        },
        (Some(profile), Some("--save"), None) if profile != "--save" && profile != "add" => {
            ModelCommand::Switch {
                profile,
                save: true,
            }
        }
        _ => ModelCommand::Invalid,
    }
}

fn handle_inline_command<'a>(agent: &mut Option<Agent>, line: &'a str) -> InlineCommandResult<'a> {
    match line {
        "exit" | "quit" | "/exit" | "/quit" => InlineCommandResult::Quit,
        "clear" | "/clear" => {
            if let Some(agent) = agent {
                agent.clear_history();
            }
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
        cmd if cmd.split_whitespace().next() == Some("/model") => {
            InlineCommandResult::Model(parse_model_command(cmd))
        }
        cmd if cmd.starts_with('/') => {
            println!("{}", warn(&format!("unknown command: {cmd}")));
            println!("{}", dim("type /help to see available commands"));
            InlineCommandResult::Handled
        }
        prompt => InlineCommandResult::Prompt(prompt),
    }
}

fn load_history(editor: &mut TuiEditor, history_path: &Path) {
    if history_path.exists() {
        let _ = editor.load_history(history_path);
    }
}

fn save_history(editor: &mut TuiEditor, history_path: &Path) {
    if let Some(parent) = history_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = editor.save_history(history_path);
}

fn clear_terminal() {
    print!("\x1b[2J\x1b[H");
    let _ = io::stdout().flush();
}

fn print_logo() {
    println!("{}", mint("  ╱╲"));
    println!("{}", mint(" ╱╱╲╲  na"));
}

fn mint(text: &str) -> String {
    format!("\x1b[38;2;127;224;190m{text}\x1b[0m")
}

struct RawInputGuard;

impl RawInputGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(error) = execute!(io::stdout(), EnableBracketedPaste) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        Ok(Self)
    }
}

impl Drop for RawInputGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        let _ = disable_raw_mode();
    }
}

fn read_masked_key() -> anyhow::Result<Option<String>> {
    let _raw = RawInputGuard::enter()?;
    let mut stdout = io::stdout();
    print!("API Key > \x1b[2msk-***\x1b[0m\x1b[6D");
    stdout.flush()?;
    let mut key = String::new();
    let mut placeholder = true;
    loop {
        match event::read()? {
            TerminalEvent::Key(event) if event.kind == KeyEventKind::Press => {
                if event.code == TerminalKeyCode::Esc
                    || (event.code == TerminalKeyCode::Char('c')
                        && event.modifiers.contains(KeyModifiers::CONTROL))
                {
                    println!();
                    return Ok(None);
                }
                match event.code {
                    TerminalKeyCode::Enter => {
                        if !key.trim().is_empty() {
                            println!();
                            return Ok(Some(key.trim().to_owned()));
                        }
                    }
                    TerminalKeyCode::Backspace => {
                        if placeholder {
                            print!("\x1b[0K");
                            placeholder = false;
                        }
                        key.pop();
                        print!("\r\x1b[2KAPI Key > {}", "•".repeat(key.chars().count()));
                        stdout.flush()?;
                    }
                    TerminalKeyCode::Char(ch)
                        if !event.modifiers.contains(KeyModifiers::CONTROL)
                            && !event.modifiers.contains(KeyModifiers::ALT)
                            && !ch.is_control() =>
                    {
                        if placeholder {
                            print!("\x1b[0K");
                            placeholder = false;
                        }
                        key.push(ch);
                        print!("•");
                        stdout.flush()?;
                    }
                    _ => {}
                }
            }
            TerminalEvent::Paste(text) => {
                if placeholder {
                    print!("\x1b[0K");
                    placeholder = false;
                }
                let accepted: String = text
                    .chars()
                    .filter(|ch| !ch.is_control() && !matches!(ch, '\r' | '\n' | '\0'))
                    .collect();
                key.push_str(&accepted);
                print!("\r\x1b[2KAPI Key > {}", "•".repeat(key.chars().count()));
                stdout.flush()?;
            }
            _ => {}
        }
    }
}

fn readline_with_resources(
    editor: &mut TuiEditor,
    prompt: &str,
    palette: &Arc<Mutex<PaletteState>>,
) -> Result<String, ReadlineError> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return editor.readline(prompt);
    }
    let width = crossterm::terminal::size()
        .map(|(width, _)| width as usize)
        .unwrap_or(80);
    let mut sampler = resources::ResourceSampler::default();
    let initial = resources::format_line(sampler.sample(), width);
    println!("{}", mint(&initial));
    let _ = io::stdout().flush();
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let thread_palette = Arc::clone(palette);
    let updater = thread::spawn(move || loop {
        for _ in 0..10 {
            if thread_stop.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        if thread_stop.load(Ordering::Relaxed) {
            return;
        }
        let width = crossterm::terminal::size()
            .map(|(width, _)| width as usize)
            .unwrap_or(80);
        let snapshot = sampler.sample();
        let in_palette = {
            let state = thread_palette.lock();
            state.menu.is_some() || (state.active && state.filter.starts_with('/'))
        };
        if in_palette {
            continue;
        }
        let status = resources::format_line(snapshot, width);
        let output = format!("\x1b[s\x1b[1A\r\x1b[2K{}\x1b[u", mint(&status));
        let mut stdout = io::stdout().lock();
        if stdout.write_all(output.as_bytes()).is_err() || stdout.flush().is_err() {
            return;
        }
    });
    let result = editor.readline(prompt);
    stop.store(true, Ordering::Relaxed);
    let _ = updater.join();
    result
}

async fn run_deepseek_onboarding(
    editor: &mut TuiEditor,
    current_config: &mut Config,
    catalog_config: &mut Config,
    current_label: &mut String,
    config_path: &Path,
) -> anyhow::Result<bool> {
    println!();
    println!("{}", mint("Connect DeepSeek"));
    println!("  1. Open https://platform.deepseek.com/api_keys");
    println!("  2. Sign in or create an API key, then copy it.");
    println!("  3. Paste the key below. Input is masked.");

    let api_base = onboarding::api_base(current_config, catalog_config);
    loop {
        let Some(mut key) = read_masked_key()? else {
            return Ok(false);
        };
        loop {
            println!("  Testing connection...");
            match onboarding::validate_key(&api_base, &key).await {
                Ok(()) => {
                    if let Err(error) = save_deepseek_key(config_path, &key) {
                        key.clear();
                        return Err(error);
                    }
                    let profile_name = next_deepseek_profile_name(catalog_config);
                    let profile = ModelProfile {
                        provider: "deepseek".to_owned(),
                        model: onboarding::MODEL.to_owned(),
                        api_url: (api_base != onboarding::API_BASE).then_some(api_base.clone()),
                        api_key_env: None,
                        temperature: None,
                        timeout_secs: None,
                    };
                    if let Err(error) = add_profile(config_path, &profile_name, profile) {
                        let _ = std::fs::remove_file(deepseek_key_path(config_path));
                        key.clear();
                        return Err(error);
                    }
                    if let Err(error) = set_default_profile(config_path, &profile_name) {
                        let _ = remove_profile(config_path, &profile_name);
                        let _ = std::fs::remove_file(deepseek_key_path(config_path));
                        key.clear();
                        return Err(error);
                    }
                    catalog_config.models.profiles.insert(
                        profile_name.clone(),
                        ModelProfile {
                            provider: "deepseek".to_owned(),
                            model: onboarding::MODEL.to_owned(),
                            api_url: (api_base != onboarding::API_BASE).then_some(api_base),
                            api_key_env: None,
                            temperature: None,
                            timeout_secs: None,
                        },
                    );
                    catalog_config.models.default = Some(profile_name.clone());
                    let (mut selected, label) =
                        resolve_selection(catalog_config, Some(&profile_name), None, None)?;
                    selected.runtime_api_key = Some(RuntimeApiKey::new("deepseek", key.clone()));
                    *current_config = selected;
                    *current_label = label;
                    key.clear();
                    return Ok(true);
                }
                Err(_) => {
                    println!("  Connection failed. [R retry] [E edit]");
                    editor.helper().unwrap().state.lock().active = false;
                    match editor.readline("  > ") {
                        Ok(choice) if choice.trim().eq_ignore_ascii_case("r") => continue,
                        Ok(choice) if choice.trim().eq_ignore_ascii_case("e") => {
                            key.clear();
                            break;
                        }
                        Ok(choice) if choice.trim().eq_ignore_ascii_case("q") => {
                            key.clear();
                            return Ok(false);
                        }
                        Err(ReadlineError::Interrupted | ReadlineError::Eof) => {
                            key.clear();
                            return Ok(false);
                        }
                        Ok(_) => println!("  Choose R to retry, E to edit, or q to quit."),
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        }
    }
}

fn next_deepseek_profile_name(config: &Config) -> String {
    if !config.models.profiles.contains_key("deepseek") {
        return "deepseek".to_owned();
    }
    (2..)
        .map(|index| format!("deepseek-{index}"))
        .find(|name| !config.models.profiles.contains_key(name))
        .unwrap()
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
    println!(
        "  {}  browse models, profiles, and providers",
        accent("/model")
    );
    println!(
        "  {}  switch model for this conversation",
        accent("/model <profile>")
    );
    println!(
        "  {}  switch and set the default",
        accent("/model <profile> --save")
    );
    println!(
        "  {}  discover a built-in provider's models and add a profile",
        accent("/model add [provider]")
    );
    println!("     Set an API key environment variable first; enter q to cancel setup.");
    println!("  {}   show this help", accent("/help"));
    println!("  {}   quit interactive mode", accent("/exit"));
    println!("  {}   quit interactive mode", accent("/quit"));
    println!(
        "{}",
        dim("Type / to browse commands; ↑ / ↓ select, → completes, Enter runs. Outside menus, ↑ / ↓ browse saved prompt history.")
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

    #[tokio::test]
    async fn slash_quit_is_recognized() {
        let mut agent = None;
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
        let mut agent = None;
        assert!(matches!(
            handle_inline_command(&mut agent, "/wat"),
            InlineCommandResult::Handled
        ));
    }

    #[tokio::test]
    async fn plain_prompt_is_passed_through() {
        let mut agent = None;
        match handle_inline_command(&mut agent, "hello") {
            InlineCommandResult::Prompt(prompt) => assert_eq!(prompt, "hello"),
            _ => panic!("expected prompt passthrough"),
        }
    }

    #[tokio::test]
    async fn rescan_is_recognized() {
        let mut agent = None;
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
        let mut agent = None;
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
        let mut agent = None;
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
        let mut agent = None;
        assert!(matches!(
            handle_inline_command(&mut agent, "/help"),
            InlineCommandResult::Handled
        ));
    }

    #[tokio::test]
    async fn model_command_recognizes_menu_and_rejects_invalid_syntax() {
        let mut agent = None;
        assert!(matches!(
            handle_inline_command(&mut agent, "/model"),
            InlineCommandResult::Model(ModelCommand::Menu)
        ));
        for input in [
            "/model --save",
            "/model local extra",
            "/model local --save extra",
        ] {
            assert!(matches!(
                handle_inline_command(&mut agent, input),
                InlineCommandResult::Model(ModelCommand::Invalid)
            ));
        }
    }

    #[test]
    fn model_add_command_accepts_optional_builtin_and_rejects_extra_arguments() {
        assert!(matches!(
            parse_model_command("/model add"),
            ModelCommand::Add(None)
        ));
        assert!(matches!(
            parse_model_command("/model add deepseek"),
            ModelCommand::Add(Some("deepseek"))
        ));
        assert!(matches!(
            parse_model_command("/model add --save"),
            ModelCommand::Invalid
        ));
        assert!(matches!(
            parse_model_command("/model add deepseek extra"),
            ModelCommand::Invalid
        ));
    }

    #[test]
    fn model_choice_requires_a_number_in_catalog_range() {
        let models = vec!["first".to_string(), "second".to_string()];
        assert_eq!(choose_model("2", &models).unwrap(), "second");
        for invalid in ["", "0", "3", "first", "-1"] {
            assert!(choose_model(invalid, &models).is_err());
        }
    }

    #[test]
    fn provider_choice_resolves_id_or_number_without_falling_back_on_invalid_input() {
        assert_eq!(choose_provider("deepseek").unwrap().id, "deepseek");
        assert_eq!(choose_provider("2").unwrap().id, "kimi");
        assert!(choose_provider("0").is_err());
        assert!(choose_provider("missing").is_err());
    }

    #[test]
    fn key_environment_field_rejects_secret_values_and_invalid_names() {
        assert!(valid_key_env_name("DEEPSEEK_API_KEY"));
        assert!(valid_key_env_name("_NA_TEST_KEY"));
        for invalid in [
            "",
            "sk-live-secret",
            "NOT AN ENV NAME",
            "1_KEY",
            "KEY=secret",
        ] {
            assert!(!valid_key_env_name(invalid));
        }
    }
}

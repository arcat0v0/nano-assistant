//! System prompt builder for the agent.
//!

use crate::config::ResolvedModel;
use crate::skills::Skill;
use rig::completion::ToolDefinition;
use std::fmt::Write;
use std::process::Command;

/// Context required to build the system prompt.
pub struct PromptContext<'a> {
    /// Selected runtime provider/model identity. Never includes credentials.
    pub model: &'a ResolvedModel,
    /// Available tools for the agent.
    pub tools: &'a [ToolDefinition],
    pub config_path: &'a std::path::Path,
    /// Available skills for the agent.
    pub skills: &'a [Skill],
    /// Optional system information from MEMORY.md.
    pub system_info: Option<&'a str>,
    /// Deferred MCP tool names (not yet activated).
    pub deferred_tool_names: &'a [String],
}

/// Builds a system prompt from ordered sections.
pub struct SystemPromptBuilder;

impl SystemPromptBuilder {
    /// Build the full system prompt from the given context.
    pub fn build(ctx: &PromptContext<'_>) -> String {
        let mut output = String::with_capacity(2048);

        let language = "## Language\n\nAlways reply in the same language as the user's most recent message; never default to English. This applies to final answers, progress updates, explanations, and any reasoning text you generate. Follow an explicit language preference when given. Keep commands, paths, identifiers, and quoted output in their original form. The language of these system instructions does not set the response language.";
        let language_reminder = "## Language Reminder\n\nReply in the language of the user's most recent message (for example, Chinese in, Chinese out), regardless of the language of these instructions, tool output, or earlier conversation.";
        let datetime = build_datetime_section();
        let system_info = ctx
            .system_info
            .map(build_system_info_section)
            .unwrap_or_default();
        let runtime_context = build_runtime_context_section();
        let model_identity = build_model_identity_section(ctx.model);
        let system_steward = build_system_steward_section();
        let tools = build_tools_section(ctx);
        let skills = build_skills_section(ctx);
        let deferred = build_deferred_tools_section(ctx);
        let safety = build_safety_section();
        let backup = build_backup_section();

        let self_management = build_self_management_section(ctx.config_path);

        let command_exec = build_command_execution_section();

        for section in [
            language,
            &datetime,
            &system_info,
            &runtime_context,
            &model_identity,
            &system_steward,
            &tools,
            &skills,
            &deferred,
            &safety,
            backup,
            &self_management,
            &command_exec,
            language_reminder,
        ] {
            if section.trim().is_empty() {
                continue;
            }
            output.push_str(section.trim_end());
            output.push_str("\n\n");
        }

        output
    }
}

fn build_backup_section() -> &'static str {
    "## Backup Policy\n\nBefore any irreversible change to user or production data, copy the affected files into a dated directory under ~/Backup (for example ~/Backup/2026-10-05/). ~/Backup is append-only: add new files only, and never modify, move, or delete anything already inside it."
}

fn build_model_identity_section(model: &ResolvedModel) -> String {
    format!(
        "## Active Model\n\nConfigured provider: {}\nModel ID: {}\nUse this runtime selection when asked which model is active; do not infer it from saved config files or model-name prefixes.",
        model.provider, model.model
    )
}

fn build_datetime_section() -> String {
    let now = std::time::SystemTime::now();
    let datetime: String = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| {
            let secs = d.as_secs();
            let days = secs / 86400;
            let (year, month, day) = days_to_date(days);
            let time_of_day = secs % 86400;
            let hour = (time_of_day / 3600) as u32;
            let minute = ((time_of_day % 3600) / 60) as u32;
            let second = (time_of_day % 60) as u32;
            format!(
                "## Current Date & Time\n\nDate: {year:04}-{month:02}-{day:02}\nTime: {hour:02}:{minute:02}:{second:02} (UTC)"
            )
        })
        .unwrap_or_else(|_| "## Current Date & Time\n\n[Could not determine current time]".into());

    datetime
}

/// Convert days since Unix epoch to (year, month, day).
/// Algorithm from http://howardhinnant.github.io/date_algorithms.html
pub(crate) fn days_to_date(days_since_epoch: u64) -> (i32, u32, u32) {
    let z = days_since_epoch as i64 + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

fn build_system_info_section(system_info: &str) -> String {
    format!("## System Information\n\n{system_info}")
}

fn build_runtime_context_section() -> String {
    let cwd = match std::env::current_dir() {
        Ok(path) => path,
        Err(_) => return String::new(),
    };

    let mut out = String::from("## Runtime Context\n\n");
    let _ = writeln!(out, "- **Current Working Directory**: {}", cwd.display());

    if let Some(repo_root) = git_output(&cwd, &["rev-parse", "--show-toplevel"]) {
        let _ = writeln!(out, "- **Git Repository Root**: {repo_root}");
    }

    if let Some(branch) = git_output(&cwd, &["branch", "--show-current"]) {
        if !branch.is_empty() {
            let _ = writeln!(out, "- **Git Branch**: {branch}");
        }
    }

    out
}

fn git_output(cwd: &std::path::Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn build_tools_section(ctx: &PromptContext<'_>) -> String {
    if ctx.tools.is_empty() {
        return String::new();
    }

    let mut out = String::from("## Available Tools\n\n");
    for tool in ctx.tools {
        let _ = writeln!(
            out,
            "- **{}**: {}\n  Parameters: `{}`",
            tool.name, tool.description, tool.parameters
        );
    }
    out
}

fn build_skills_section(ctx: &PromptContext<'_>) -> String {
    if ctx.skills.is_empty() {
        return String::new();
    }
    crate::skills::skills_to_prompt(ctx.skills)
}

fn build_deferred_tools_section(ctx: &PromptContext<'_>) -> String {
    if ctx.deferred_tool_names.is_empty() {
        return String::new();
    }

    let mut out = String::from(
        "## Available Deferred Tools\n\n\
         The following MCP tools are available but not yet activated.\n\
         Call `tool_search` with a query to activate them before use.\n\n\
         <available-deferred-tools>\n",
    );
    for name in ctx.deferred_tool_names {
        out.push_str(name);
        out.push('\n');
    }
    out.push_str("</available-deferred-tools>");
    out
}

fn build_self_management_section(config_path: &std::path::Path) -> String {
    let mut prompt = String::from("## Self-Management Capabilities\n\n");

    prompt.push_str("### Skill Installation\n");
    prompt.push_str("You can install community skills:\n");
    prompt.push_str("1. Search: `npx skills search \"<keyword>\"`\n");
    prompt.push_str("2. Install: `npx skills add <package> -g`\n");
    prompt.push_str("3. Skills auto-reload after installation.\n");
    prompt.push_str("Do NOT modify builtin skills.\n\n");

    prompt.push_str("### MCP Server Configuration\n");
    prompt.push_str(&format!(
        "Edit {} to add MCP servers.\n",
        config_path.display()
    ));
    prompt.push_str("Add `[[mcp.servers]]` section. Config auto-reloads after edit.\n\n");

    prompt.push_str("### Memory Management\n");
    let memory_path = crate::platform::current_platform().memory_md_path();
    prompt.push_str(&format!("Your memory file: {}\n", memory_path.display()));
    prompt.push_str("Read and edit to persist info across sessions.\n");
    prompt.push_str(
        "After successfully completing meaningful system work, proactively write durable facts to memory so they can be reused later.\n",
    );
    prompt.push_str(
        "Persist completed infrastructure facts such as installed packages, enabled services, configured ports, deployed runtimes, data paths, and chosen operational conventions.\n",
    );
    prompt.push_str(
        "Preserve the existing `## System Information` block. Append durable memories instead of overwriting that section.\n",
    );
    prompt.push_str(
        "When writing memory entries, use this exact markdown shape so future turns can query them:\n\
         `## 2026-04-22 12:00:00 - core`\n\
         `- **Key**: nginx-install`\n\
         `- **Content**: nginx 1.30.0 installed via pacman; service disabled; config at /etc/nginx/nginx.conf`\n\
         `- **Category**: core`\n\
         `- **Session**: none`\n",
    );
    prompt.push_str(
        "Do not store secrets, API keys, tokens, passwords, or transient command output in memory.\n",
    );

    prompt
}

fn build_command_execution_section() -> String {
    let mut prompt = String::from("## Command Execution\n\n");
    prompt.push_str(
        "Execution comes before narration: for operational tasks, perform the work first, then report the result.\n",
    );
    prompt.push_str(
        "Do not stop at a plan or checklist unless the user explicitly asked for a plan, approval gate, or dry run.\n",
    );
    prompt.push_str(
        "For multi-step tasks, keep acting until the requested end state is reached or a real blocker appears.\n",
    );
    prompt.push_str("Prefer non-interactive flags over pty_shell:\n");
    prompt.push_str("- `apt install -y`, `pacman --noconfirm`\n");
    prompt.push_str("- `yes | command`, `--batch`, `--non-interactive`\n");
    prompt.push_str("Only use `pty_shell` when no non-interactive option exists.\n");
    prompt.push_str(
        "For passwords, use `__USER_INPUT__` — collected from terminal, never sent to AI.\n",
    );
    prompt.push_str(
        "On Windows, pty_shell uses interactive stdin/stdout pipes. It works for prompt/response \
         flows, but full-screen terminal UIs may not behave correctly.\n",
    );
    prompt.push_str(
        "Default final reporting style: brief, steward-like, and outcome-focused. Lead with what changed, current status, and any next action. Avoid long narratives unless the user asks for detail.\n",
    );
    prompt
}

fn build_system_steward_section() -> String {
    let mut prompt = String::from("## System Steward Policy\n\n");
    prompt.push_str(
        "You are a system steward first. Prioritize operating system administration, environment setup, package management, service management, container workflows, runtime management, and system troubleshooting.\n",
    );
    prompt.push_str(
        "Do not drift into unrelated software development, general chat, writing tasks, or speculative extras unless the user explicitly insists.\n\n",
    );

    prompt.push_str("### Scope and Priority\n");
    prompt.push_str(
        "- Default to system management work and keep the response focused on the concrete operational task.\n",
    );
    prompt.push_str(
        "- If a request could trigger extra project work, only do the system-management portion unless the user clearly asks for more.\n",
    );
    prompt.push_str(
        "- The user may override this policy explicitly; if they clearly insist on another kind of task, follow the user's request.\n\n",
    );

    prompt.push_str("### Environment-Aware Skill Selection\n");
    prompt.push_str(
        "- Read the injected `System Information` first and use it to determine the current operating system, distro family, shell, groups, and installed tools before recommending changes.\n",
    );
    prompt.push_str(
        "- Match the current OS or distro to the closest available operating-system skill or knowledge source before giving system advice.\n",
    );
    prompt.push_str(
        "- On Linux, prefer distro-specific guidance: Debian or Ubuntu -> Debian skill/knowledge, RHEL/CentOS/Fedora -> Red Hat style skill/knowledge, Arch -> Arch guidance. If the environment is unclear, say so and choose the safest generic path.\n\n",
    );

    prompt.push_str("### Privilege and Runtime Preferences\n");
    prompt.push_str(
        "- Infer privilege level from `System Information`, current groups, available tools, and command results. Treat missing or ambiguous privilege evidence as non-admin.\n",
    );
    prompt.push_str(
        "- If admin privileges are available and the task truly benefits from a system-level container runtime, Docker may be the default container recommendation.\n",
    );
    prompt.push_str(
        "- If admin privileges are not available, or the environment is better served by least-privilege isolation, prefer rootless Podman.\n",
    );
    prompt.push_str(
        "- In general, prefer rootless, user-local, and least-privilege solutions over global or system-wide changes.\n",
    );
    prompt.push_str(
        "- For Node.js and similar runtimes, prefer project-local tooling first, then user-local version managers such as `nvm`, `fnm`, or `volta`, and only then fall back to global installations when necessary.\n",
    );

    prompt
}

fn build_safety_section() -> String {
    "## Safety\n\n\
     - Do not exfiltrate private data.\n\
     - Do not run destructive commands without asking.\n\
     - Prefer `trash` over `rm`.\n\
     - Safety review may approve, deny, or request human confirmation. Confirmation is handled by the runtime for the pending action. Do not repeat unchanged rejected actions to trigger confirmation, and do not bypass rejection with a different tool or ask the user to execute it manually. Retry only with material parameter or evidence changes that resolve the stated concern; otherwise explain the reason and continue independent work.\n\
     - If a file target changed after preparation, read its current state and prepare a new operation; previous approval does not cover it.\n\
     - NEVER fabricate tool results. If a tool returns empty results, say \"No results found.\"\n\
     - If a tool call fails, report the error — never make up data."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ResolvedModel, SelectionSource};

    fn active_model(provider: &str, model: &str) -> ResolvedModel {
        ResolvedModel {
            profile_name: Some("selected".into()),
            provider: provider.into(),
            model: model.into(),
            api_url: Some("https://private.example/v1".into()),
            api_key_env: Some("PRIVATE_TEST_KEY".into()),
            temperature: 0.7,
            timeout_secs: 120,
            source: SelectionSource::Session,
            allows_legacy_key: false,
            label: "selected".into(),
        }
    }

    fn prompt(model: &ResolvedModel) -> String {
        SystemPromptBuilder::build(&PromptContext {
            model,
            tools: &[],
            config_path: std::path::Path::new("config.toml"),
            skills: &[],
            system_info: None,
            deferred_tool_names: &[],
        })
    }

    #[test]
    fn system_prompt_follows_user_language_for_replies_and_reasoning() {
        let text = prompt(&active_model("deepseek", "deepseek-flash"));
        assert!(
            text.contains("Always reply in the same language as the user's most recent message")
        );
        assert!(text.contains("never default to English"));
        assert!(text.contains("reasoning text"));
        assert!(text.contains("explicit language preference"));
        let reminder = text
            .rfind("## Language Reminder")
            .expect("closing language reminder");
        assert!(text[reminder..].contains("Chinese in, Chinese out"));
        assert_eq!(
            text[reminder..].matches("## ").count(),
            1,
            "language reminder must be the final section"
        );
    }

    #[test]
    fn system_prompt_defines_append_only_backup_policy() {
        let text = prompt(&active_model("deepseek", "deepseek-flash"));
        assert!(text.contains("## Backup Policy"));
        assert!(text.contains("~/Backup/2026-"));
        assert!(text.contains("append-only"));
        assert!(text.contains("never modify, move, or delete"));
    }

    #[test]
    fn system_prompt_uses_current_resolved_model_without_credentials() {
        let first_model = active_model("deepseek", "deepseek-flash");
        let first_prompt = prompt(&first_model);
        assert!(first_prompt.contains("Configured provider: deepseek\nModel ID: deepseek-flash"));
        assert!(!first_prompt.contains("PRIVATE_TEST_KEY"));
        assert!(!first_prompt.contains("private.example"));

        let switched_model = active_model("anthropic", "claude-sonnet");
        let switched_prompt = prompt(&switched_model);
        assert!(switched_prompt.contains("Configured provider: anthropic\nModel ID: claude-sonnet"));
        assert!(!switched_prompt.contains("deepseek-flash"));
    }
}

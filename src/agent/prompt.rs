//! System prompt builder for the agent.
//!
//! Simplified from ZeroClaw's `prompt.rs` — no AIEOS, no channel media.
//! Builds a system prompt with: datetime, tools list, skills, tool-usage protocol, safety.

use crate::skills::Skill;
use crate::tools::Tool;
use crate::tools::ToolSpec;
use std::fmt::Write;
use std::process::Command;

/// Context required to build the system prompt.
pub struct PromptContext<'a> {
    /// Available tools for the agent.
    pub tools: &'a [Box<dyn Tool>],
    /// Tool specs pre-computed from tools (avoids re-computation).
    pub tool_specs: &'a [ToolSpec],
    /// Whether the provider supports native function calling.
    pub native_tool_calling: bool,
    /// Dispatcher-specific instructions (XML protocol for non-native, empty for native).
    pub dispatcher_instructions: &'a str,
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

        let datetime = build_datetime_section();
        let system_info = ctx
            .system_info
            .map(build_system_info_section)
            .unwrap_or_default();
        let runtime_context = build_runtime_context_section();
        let system_steward = build_system_steward_section();
        let tools = build_tools_section(ctx);
        let skills = build_skills_section(ctx);
        let deferred = build_deferred_tools_section(ctx);
        let protocol = build_protocol_section(ctx);
        let safety = build_safety_section();

        let self_management = build_self_management_section();

        let command_exec = build_command_execution_section();

        for section in [
            &datetime,
            &system_info,
            &runtime_context,
            &system_steward,
            &tools,
            &skills,
            &deferred,
            &protocol,
            &safety,
            &self_management,
            &command_exec,
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
fn days_to_date(days_since_epoch: u64) -> (i32, u32, u32) {
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
            tool.name(),
            tool.description(),
            tool.parameters_schema()
        );
    }

    if !ctx.dispatcher_instructions.is_empty() {
        out.push('\n');
        out.push_str(ctx.dispatcher_instructions);
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

fn build_protocol_section(ctx: &PromptContext<'_>) -> String {
    if ctx.native_tool_calling {
        String::new()
    } else {
        "## Tool Use Protocol\n\n\
         To use a tool, wrap a JSON object in `antml:invoke name=\"tool_name\"` tags:\n\n\
         ```\n\
         antml:invoke name=\"tool_name\"\n\
         {\"param\": \"value\"}\n\
         antml:invoke\n\
         ```\n\n\
         Wait for tool results before responding. You may chain multiple tool calls \
         if they are independent."
            .to_string()
    }
}

fn build_self_management_section() -> String {
    let mut prompt = String::from("## Self-Management Capabilities\n\n");

    prompt.push_str("### Skill Installation\n");
    prompt.push_str("You can install community skills:\n");
    prompt.push_str("1. Search: `npx skills search \"<keyword>\"`\n");
    prompt.push_str("2. Install: `npx skills add <package> -g`\n");
    prompt.push_str("3. Skills auto-reload after installation.\n");
    prompt.push_str("Do NOT modify builtin skills.\n\n");

    prompt.push_str("### MCP Server Configuration\n");
    let config_path = crate::platform::current_platform().config_path();
    prompt.push_str(&format!(
        "Edit {} to add MCP servers.\n",
        config_path.display()
    ));
    prompt.push_str("Add `[[mcp.servers]]` section. Config auto-reloads after edit.\n\n");

    prompt.push_str("### Memory Management\n");
    let memory_path = crate::platform::current_platform().memory_md_path();
    prompt.push_str(&format!("Your memory file: {}\n", memory_path.display()));
    prompt.push_str("Read and edit to persist info across sessions.\n");

    prompt
}

fn build_command_execution_section() -> String {
    let mut prompt = String::from("## Command Execution\n\n");
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
     - NEVER fabricate tool results. If a tool returns empty results, say \"No results found.\"\n\
     - If a tool call fails, report the error — never make up data."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct DummyTool;

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            "dummy_tool"
        }
        fn description(&self) -> &str {
            "A test tool"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        async fn execute(
            &self,
            _args: serde_json::Value,
        ) -> anyhow::Result<crate::tools::ToolResult> {
            Ok(crate::tools::ToolResult {
                success: true,
                output: "ok".into(),
                error: None,
            })
        }
    }

    #[test]
    fn prompt_contains_tools_section() {
        let tool: Box<dyn Tool> = Box::new(DummyTool);
        let tools: Vec<Box<dyn Tool>> = vec![tool];
        let specs: Vec<ToolSpec> = tools.iter().map(|t| t.spec()).collect();
        let ctx = PromptContext {
            tools: &tools,
            tool_specs: &specs,
            native_tool_calling: false,
            dispatcher_instructions: "",
            skills: &[],
            system_info: None,
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        assert!(prompt.contains("## Available Tools"));
        assert!(prompt.contains("dummy_tool"));
    }

    #[test]
    fn prompt_contains_safety_section() {
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: false,
            dispatcher_instructions: "",
            skills: &[],
            system_info: None,
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        assert!(prompt.contains("## Safety"));
        assert!(prompt.contains("NEVER fabricate"));
    }

    #[test]
    fn prompt_includes_xml_protocol_for_non_native() {
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: false,
            dispatcher_instructions: "",
            skills: &[],
            system_info: None,
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        assert!(prompt.contains("## Tool Use Protocol"));
    }

    #[test]
    fn prompt_omits_protocol_for_native() {
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: true,
            dispatcher_instructions: "",
            skills: &[],
            system_info: None,
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        assert!(!prompt.contains("## Tool Use Protocol"));
    }

    #[test]
    fn days_to_date_epoch() {
        assert_eq!(days_to_date(0), (1970, 1, 1));
    }

    #[test]
    fn days_to_date_recent() {
        assert_eq!(days_to_date(19723), (2024, 1, 1));
    }

    #[test]
    fn prompt_contains_datetime() {
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: false,
            dispatcher_instructions: "",
            skills: &[],
            system_info: None,
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        assert!(prompt.contains("## Current Date & Time"));
    }

    #[test]
    fn prompt_empty_when_no_tools_and_native() {
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: true,
            dispatcher_instructions: "",
            skills: &[],
            system_info: None,
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        assert!(prompt.contains("## Current Date & Time"));
        assert!(prompt.contains("## Safety"));
    }

    #[test]
    fn prompt_includes_skills_section() {
        let skills = vec![crate::skills::Skill {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            version: "0.1.0".to_string(),
            author: None,
            tags: vec![],
            tools: vec![],
            prompts: vec!["Do the thing.".to_string()],
            location: None,
            is_builtin: false,
            source: None,
            raw_content: None,
        }];
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: false,
            dispatcher_instructions: "",
            skills: &skills,
            system_info: None,
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("<name>test-skill</name>"));
        assert!(prompt.contains("</available_skills>"));
    }

    #[test]
    fn prompt_includes_system_info_when_provided() {
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: false,
            dispatcher_instructions: "",
            skills: &[],
            system_info: Some("OS: Linux\nKernel: 5.15"),
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        assert!(prompt.contains("## System Information"));
        assert!(prompt.contains("OS: Linux"));
    }

    #[test]
    fn prompt_includes_runtime_context() {
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: false,
            dispatcher_instructions: "",
            skills: &[],
            system_info: None,
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        assert!(prompt.contains("## Runtime Context"));
        assert!(prompt.contains("Current Working Directory"));
    }

    #[test]
    fn prompt_omits_system_info_when_none() {
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: false,
            dispatcher_instructions: "",
            skills: &[],
            system_info: None,
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        assert!(!prompt.contains("## System Information"));
    }

    #[test]
    fn prompt_system_info_section_placed_after_datetime() {
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: false,
            dispatcher_instructions: "",
            skills: &[],
            system_info: Some("OS: TestLinux"),
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        let datetime_pos = prompt.find("## Current Date & Time").unwrap();
        let sysinfo_pos = prompt.find("## System Information").unwrap();
        assert!(
            sysinfo_pos > datetime_pos,
            "System info should appear after datetime"
        );
    }

    #[test]
    fn prompt_runtime_context_section_placed_after_system_info() {
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: false,
            dispatcher_instructions: "",
            skills: &[],
            system_info: Some("OS: TestLinux"),
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        let sysinfo_pos = prompt.find("## System Information").unwrap();
        let runtime_pos = prompt.find("## Runtime Context").unwrap();
        assert!(
            runtime_pos > sysinfo_pos,
            "Runtime context should appear after system info"
        );
    }

    #[test]
    fn prompt_contains_self_management_section() {
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: false,
            dispatcher_instructions: "",
            skills: &[],
            system_info: None,
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        assert!(prompt.contains("## Self-Management Capabilities"));
        assert!(prompt.contains("### Skill Installation"));
        assert!(prompt.contains("### MCP Server Configuration"));
        assert!(prompt.contains("### Memory Management"));
        assert!(prompt.contains("config.toml"));
        assert!(prompt.contains("MEMORY.md"));
    }

    #[test]
    fn prompt_contains_system_steward_policy_section() {
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: false,
            dispatcher_instructions: "",
            skills: &[],
            system_info: None,
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        assert!(prompt.contains("## System Steward Policy"));
        assert!(prompt.contains("You are a system steward first."));
        assert!(prompt.contains("The user may override this policy explicitly"));
    }

    #[test]
    fn prompt_system_steward_policy_includes_environment_rules() {
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: false,
            dispatcher_instructions: "",
            skills: &[],
            system_info: None,
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        assert!(prompt.contains("Read the injected `System Information` first"));
        assert!(prompt.contains("Debian or Ubuntu -> Debian skill/knowledge"));
        assert!(prompt.contains("RHEL/CentOS/Fedora -> Red Hat style skill/knowledge"));
        assert!(prompt.contains("Arch -> Arch guidance"));
    }

    #[test]
    fn prompt_system_steward_policy_includes_privilege_preferences() {
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: false,
            dispatcher_instructions: "",
            skills: &[],
            system_info: None,
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        assert!(prompt.contains("Treat missing or ambiguous privilege evidence as non-admin"));
        assert!(prompt.contains("Docker may be the default container recommendation"));
        assert!(prompt.contains("prefer rootless Podman"));
        assert!(prompt.contains("prefer project-local tooling first"));
    }

    #[test]
    fn prompt_mentions_windows_interactive_limit() {
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: false,
            dispatcher_instructions: "",
            skills: &[],
            system_info: None,
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        assert!(prompt.contains("On Windows, pty_shell uses interactive stdin/stdout pipes"));
        assert!(prompt.contains("full-screen terminal UIs may not behave correctly"));
    }

    #[test]
    fn prompt_system_info_preserves_multiline_content() {
        let multiline = "OS: Linux\nKernel: 6.1.0\nArch: x86_64\nUser: test";
        let ctx = PromptContext {
            tools: &[],
            tool_specs: &[],
            native_tool_calling: false,
            dispatcher_instructions: "",
            skills: &[],
            system_info: Some(multiline),
            deferred_tool_names: &[],
        };
        let prompt = SystemPromptBuilder::build(&ctx);
        assert!(prompt.contains("OS: Linux"));
        assert!(prompt.contains("Kernel: 6.1.0"));
        assert!(prompt.contains("Arch: x86_64"));
    }
}

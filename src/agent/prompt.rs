//! System prompt builder for the agent.
//!
//! Simplified from ZeroClaw's `prompt.rs` — no AIEOS, no channel media.
//! Builds a system prompt with: datetime, tools list, skills, tool-usage protocol, safety.

use crate::skills::Skill;
use crate::tools::Tool;
use crate::tools::ToolSpec;
use std::fmt::Write;

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
        let system_info = ctx.system_info.map(build_system_info_section).unwrap_or_default();
        let tools = build_tools_section(ctx);
        let skills = build_skills_section(ctx);
        let deferred = build_deferred_tools_section(ctx);
        let protocol = build_protocol_section(ctx);
        let safety = build_safety_section();

        for section in [&datetime, &system_info, &tools, &skills, &deferred, &protocol, &safety] {
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
         <available-deferred-tools>\n"
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
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<crate::tools::ToolResult> {
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
        assert!(sysinfo_pos > datetime_pos, "System info should appear after datetime");
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

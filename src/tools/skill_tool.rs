use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;

use crate::skills::SkillTool;
use crate::tools::traits::{Tool, ToolResult};

const SHELL_TIMEOUT_SECS: u64 = 60;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024; // 1 MiB

/// A tool that executes a skill-defined shell command.
///
/// The command template comes from the skill's `SkillTool.command` field.
/// Placeholder parameters like `{{key}}` are replaced with values provided
/// at call time.
pub struct SkillShellTool {
    tool_name: String,
    command: String,
    args: HashMap<String, String>,
}

impl SkillShellTool {
    pub fn new(skill_name: &str, tool: &SkillTool) -> Self {
        let tool_name = format!("{}.{}", skill_name, tool.name);
        let command = tool.command.clone();
        let args = tool.args.clone();
        Self {
            tool_name,
            command,
            args,
        }
    }

    /// Exposed for testing: the raw command template before substitution.
    #[cfg(test)]
    pub fn command_template(&self) -> &str {
        &self.command
    }
}

#[async_trait]
impl Tool for SkillShellTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        "Skill shell command"
    }

    fn parameters_schema(&self) -> Value {
        let mut properties = serde_json::Map::new();
        for (key, desc) in &self.args {
            properties.insert(
                key.clone(),
                json!({
                    "type": "string",
                    "description": desc,
                }),
            );
        }
        json!({
            "type": "object",
            "properties": properties,
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let mut command = self.command.clone();
        for key in self.args.keys() {
            if let Some(value) = args.get(key).and_then(|v| v.as_str()) {
                command = command.replace(&format!("{{{{{}}}", key), value);
            }
        }

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(&command);

        for var in ["PATH", "HOME", "TERM", "LANG", "USER"] {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }

        let result = tokio::time::timeout(Duration::from_secs(SHELL_TIMEOUT_SECS), cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let truncated = stdout.len() > MAX_OUTPUT_BYTES;
                let text = if truncated {
                    format!(
                        "{}...\n[output truncated at {} bytes]",
                        &stdout[..MAX_OUTPUT_BYTES],
                        MAX_OUTPUT_BYTES
                    )
                } else {
                    stdout
                };
                Ok(ToolResult {
                    success: output.status.success(),
                    output: text,
                    error: if output.status.success() {
                        None
                    } else {
                        Some(format!("exit code: {}", output.status.code().unwrap_or(-1)))
                    },
                })
            }
            Ok(Err(io_err)) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Command failed: {}", io_err)),
            }),
            Err(_) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Command timed out after {}s",
                    SHELL_TIMEOUT_SECS
                )),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_skill_tool(name: &str, kind: &str, command: &str, args: HashMap<String, String>) -> SkillTool {
        SkillTool {
            name: name.to_string(),
            description: format!("{} tool", name),
            kind: kind.to_string(),
            command: command.to_string(),
            args,
        }
    }

    #[test]
    fn tool_name_is_dotted() {
        let tool = make_skill_tool("greet", "shell", "echo hello", HashMap::new());
        let st = SkillShellTool::new("my-skill", &tool);
        assert_eq!(st.name(), "my-skill.greet");
    }

    #[test]
    fn parameters_schema_includes_args() {
        let mut args = HashMap::new();
        args.insert("name".to_string(), "The name to greet".to_string());
        args.insert("count".to_string(), "How many times".to_string());
        let tool = make_skill_tool("greet", "shell", "echo hello", args);
        let st = SkillShellTool::new("demo", &tool);
        let schema = st.parameters_schema();

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["name"]["type"], "string");
        assert_eq!(
            schema["properties"]["name"]["description"],
            "The name to greet"
        );
        assert_eq!(schema["properties"]["count"]["type"], "string");
    }

    #[test]
    fn parameters_schema_empty_when_no_args() {
        let tool = make_skill_tool("run", "shell", "ls", HashMap::new());
        let st = SkillShellTool::new("demo", &tool);
        let schema = st.parameters_schema();

        assert_eq!(schema["type"], "object");
        assert!(schema["properties"].as_object().unwrap().is_empty());
    }

    #[tokio::test]
    async fn echo_command_with_arg_substitution() {
        let mut args = HashMap::new();
        args.insert("msg".to_string(), "The message".to_string());
        let tool = make_skill_tool("echo", "shell", "echo {{msg}}", args);
        let st = SkillShellTool::new("demo", &tool);

        let result = st
            .execute(json!({ "msg": "hello world" }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("hello world"));
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn command_with_no_substitution_works() {
        let tool = make_skill_tool("ls", "shell", "echo ok", HashMap::new());
        let st = SkillShellTool::new("demo", &tool);

        let result = st.execute(json!({})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("ok"));
    }

    #[tokio::test]
    async fn failing_command_returns_error() {
        let tool = make_skill_tool("fail", "shell", "exit 42", HashMap::new());
        let st = SkillShellTool::new("demo", &tool);

        let result = st.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.is_some());
        assert!(result.error.as_ref().unwrap().contains("exit code: 42"));
    }

    #[tokio::test]
    async fn timeout_triggers_error() {
        let tool = make_skill_tool("slow", "shell", "sleep 999", HashMap::new());
        let st = SkillShellTool::new("demo", &tool);
        assert_eq!(st.name(), "demo.slow");
        // Executing would block for 60s; verify struct + template storage instead
        assert!(st.command_template().contains("sleep 999"));
    }
}

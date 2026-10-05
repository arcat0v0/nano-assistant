use crate::security::{ResolvedAction, SecurityManager, SecurityMode, ToolAction};
use rig::tool::{DynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use super::provider_name::{dynamic_tool_name, ToolNamespace};
use crate::skills::SkillTool;

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
    description: String,
}

impl SkillShellTool {
    pub fn new(skill_name: &str, tool: &SkillTool) -> Self {
        let tool_name = dynamic_tool_name(ToolNamespace::Skill, skill_name, &tool.name);
        let command = tool.command.clone();
        let args = tool.args.clone();
        Self {
            tool_name,
            command,
            args,
            description: tool.description.clone(),
        }
    }

    pub fn into_dynamic(self, security: Arc<SecurityManager>) -> DynamicTool {
        let name = self.tool_name.clone();
        security.register_prepared_tool(&name);
        let schema = self.parameters();
        let tool = std::sync::Arc::new(self);
        DynamicTool::new(
            name,
            "Skill shell command",
            schema,
            move |_context, args| {
                let tool = std::sync::Arc::clone(&tool);
                let security = Arc::clone(&security);
                Box::pin(async move { tool.run(args, &security).await.map(ToolOutput::text) })
            },
        )
    }

    fn parameters(&self) -> Value {
        let mut properties = serde_json::Map::new();
        for (key, desc) in &self.args {
            properties.insert(key.clone(), json!({"type": "string", "description": desc}));
        }
        json!({"type": "object", "properties": properties})
    }

    async fn run(
        &self,
        args: Value,
        security: &SecurityManager,
    ) -> Result<String, ToolExecutionError> {
        let mut command = self.command.clone();
        for key in self.args.keys() {
            if let Some(value) = args.get(key).and_then(|v| v.as_str()) {
                command = command.replace(&format!("{{{{{}}}", key), value);
            }
        }
        if security.mode() == SecurityMode::Auto {
            security
                .authorize(&ToolAction {
                    tool_name: &self.tool_name,
                    args: &args,
                    description: Some(&self.description),
                    resolved: Some(ResolvedAction::Shell {
                        command: &command,
                        shell: "sh",
                        flag: "-c",
                    }),
                })
                .await
                .map_err(ToolExecutionError::other)?;
        }

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(&command).kill_on_drop(true);

        for var in ["PATH", "HOME", "TERM", "LANG", "USER"] {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }

        let result =
            tokio::time::timeout(Duration::from_secs(SHELL_TIMEOUT_SECS), cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let text = if stdout.len() > MAX_OUTPUT_BYTES {
                    let mut boundary = MAX_OUTPUT_BYTES;
                    while !stdout.is_char_boundary(boundary) {
                        boundary -= 1;
                    }
                    format!(
                        "{}...\n[output truncated at {} bytes]",
                        &stdout[..boundary],
                        MAX_OUTPUT_BYTES
                    )
                } else {
                    stdout.into_owned()
                };
                if output.status.success() {
                    Ok(text)
                } else {
                    Err(ToolExecutionError::other(format!(
                        "Command exited with code {}: {text}\n{}",
                        output.status.code().unwrap_or(-1),
                        String::from_utf8_lossy(&output.stderr)
                    )))
                }
            }
            Ok(Err(io_err)) => Err(ToolExecutionError::other(format!(
                "Command failed: {io_err}"
            ))),
            Err(_) => Err(ToolExecutionError::timeout(format!(
                "Command timed out after {SHELL_TIMEOUT_SECS}s"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_skill_tool(
        name: &str,
        kind: &str,
        command: &str,
        args: HashMap<String, String>,
    ) -> SkillTool {
        SkillTool {
            name: name.to_string(),
            description: format!("{} tool", name),
            kind: kind.to_string(),
            command: command.to_string(),
            args,
        }
    }

    #[tokio::test]
    async fn shell_skill_dispatches_through_rig() {
        let mut args = HashMap::new();
        args.insert("msg".to_string(), "The message".to_string());
        let tool = make_skill_tool("echo", "shell", "echo {{msg}}", args);
        let dynamic = SkillShellTool::new("demo", &tool)
            .into_dynamic(Arc::new(SecurityManager::new(SecurityMode::Direct)));
        let tools = rig::tool::ToolSet::from_dynamic_tools(vec![dynamic]);
        let result = tools
            .execute(
                "skill__demo__echo",
                json!({"msg": "hello world"}).to_string(),
                &mut rig::tool::ToolContext::default(),
            )
            .await;
        assert!(result.error().is_none());
        assert!(result.output().as_text().unwrap().contains("hello world"));
    }

    #[tokio::test]
    async fn failing_command_returns_error() {
        let tool = make_skill_tool("fail", "shell", "exit 42", HashMap::new());
        let st = SkillShellTool::new("demo", &tool);

        let error = st
            .run(json!({}), &SecurityManager::new(SecurityMode::Direct))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("code 42"));
    }
}

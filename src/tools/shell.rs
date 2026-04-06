use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;

const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_OUTPUT_BYTES: usize = 1_048_576;

/// Execute shell commands.
pub struct ShellTool {
    timeout_secs: u64,
}

impl ShellTool {
    pub fn new() -> Self {
        Self {
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }

    pub fn with_timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

fn truncate_output(s: &mut String) {
    if s.len() > MAX_OUTPUT_BYTES {
        let mut b = MAX_OUTPUT_BYTES.min(s.len());
        while b > 0 && !s.is_char_boundary(b) {
            b -= 1;
        }
        s.truncate(b);
        s.push_str("\n... [output truncated at 1MB]");
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return stdout/stderr. \
         Use for running builds, tests, git operations, and other CLI tasks."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional timeout in seconds (default: 60)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'command' parameter"))?;

        let timeout = args
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.timeout_secs);

        let result = tokio::time::timeout(
            Duration::from_secs(timeout),
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();

                truncate_output(&mut stdout);
                truncate_output(&mut stderr);

                Ok(ToolResult {
                    success: output.status.success(),
                    output: stdout,
                    error: if stderr.is_empty() {
                        None
                    } else {
                        Some(stderr)
                    },
                })
            }
            Ok(Err(e)) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to execute command: {e}")),
            }),
            Err(_) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Command timed out after {timeout}s and was killed")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn executes_simple_command() {
        let tool = ShellTool::new();
        let result = tool
            .execute(json!({"command": "echo hello"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn captures_exit_code() {
        let tool = ShellTool::new();
        let result = tool
            .execute(json!({"command": "false"}))
            .await
            .unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn captures_stderr() {
        let tool = ShellTool::new();
        let result = tool
            .execute(json!({"command": "echo err >&2"}))
            .await
            .unwrap();
        assert!(result.error.as_deref().unwrap_or("").contains("err"));
    }

    #[tokio::test]
    async fn missing_command_param() {
        let tool = ShellTool::new();
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn timeout_kills_command() {
        let tool = ShellTool::new().with_timeout_secs(1);
        let result = tool
            .execute(json!({"command": "sleep 10"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("timed out"));
    }
}

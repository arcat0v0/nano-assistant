use rig::tool::{Tool, ToolContext, ToolExecutionError};
use serde_json::{json, Value};
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

impl Tool for ShellTool {
    const NAME: &'static str = "shell";
    type Args = Value;
    type Output = String;
    type Error = ToolExecutionError;

    fn description(&self) -> String {
        "Execute a shell command and return stdout/stderr. \
         Use for running builds, tests, git operations, and other CLI tasks."
            .into()
    }

    fn parameters(&self) -> Value {
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

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Value,
    ) -> Result<String, ToolExecutionError> {
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolExecutionError::invalid_args("Missing 'command' parameter"))?;

        let timeout = args
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.timeout_secs);

        let (shell, flag) = crate::platform::current_platform().shell_command();
        let result = tokio::time::timeout(
            Duration::from_secs(timeout),
            tokio::process::Command::new(shell)
                .arg(flag)
                .arg(command)
                .kill_on_drop(true)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
                truncate_output(&mut stdout);
                truncate_output(&mut stderr);
                if output.status.success() {
                    if !stderr.is_empty() {
                        if !stdout.is_empty() {
                            stdout.push('\n');
                        }
                        stdout.push_str(&stderr);
                    }
                    Ok(stdout)
                } else {
                    Err(ToolExecutionError::other(format!(
                        "Command exited with {}. stdout: {stdout}\nstderr: {stderr}",
                        output.status
                    )))
                }
            }
            Ok(Err(e)) => Err(ToolExecutionError::other(format!(
                "Failed to execute command: {e}"
            ))),
            Err(_) => Err(ToolExecutionError::timeout(format!(
                "Command timed out after {timeout}s and was killed"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn command_output_and_nonzero_exit_are_visible() {
        let mut context = ToolContext::default();
        let output = ShellTool::new()
            .call(&mut context, json!({"command": "echo hello"}))
            .await
            .unwrap();
        assert!(output.contains("hello"));
        let error = ShellTool::new()
            .call(&mut context, json!({"command": "echo failure >&2; exit 7"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("failure"));
        assert!(error.to_string().contains("exit status: 7"));
    }
}

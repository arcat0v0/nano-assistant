//! PTY-based interactive shell tool.
//!
//! Executes commands in a pseudo-terminal, allowing scripted interaction
//! with programs that require user input (e.g. password prompts, confirmations).

use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_OUTPUT_BYTES: usize = 1_048_576; // 1 MB

/// Interactive PTY shell tool.
pub struct PtyShellTool;

impl PtyShellTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PtyShellTool {
    fn default() -> Self {
        Self::new()
    }
}

/// A single expect/respond interaction step.
struct Interaction {
    expect: regex::Regex,
    respond: String,
    timeout: Duration,
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
impl Tool for PtyShellTool {
    fn name(&self) -> &str {
        "pty_shell"
    }

    fn description(&self) -> &str {
        "Execute an interactive command in a PTY with scripted expect/respond interactions. \
         Use only when no non-interactive flag exists (e.g. -y, --noconfirm, --batch). \
         For password prompts, set respond to \"__USER_INPUT__\" to collect input securely."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to run in a PTY"
                },
                "interactions": {
                    "type": "array",
                    "description": "Ordered expect/respond pairs",
                    "items": {
                        "type": "object",
                        "properties": {
                            "expect": {
                                "type": "string",
                                "description": "Regex pattern to match in output"
                            },
                            "respond": {
                                "type": "string",
                                "description": "Text to send when pattern matches. Use \"__USER_INPUT__\" for secure terminal passthrough."
                            },
                            "timeout_secs": {
                                "type": "integer",
                                "description": "Per-interaction timeout in seconds (default: 30)"
                            }
                        },
                        "required": ["expect", "respond"]
                    }
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Overall timeout in seconds (default: 120)"
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

        let overall_timeout = Duration::from_secs(
            args.get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(DEFAULT_TIMEOUT_SECS),
        );

        // Parse interactions
        let mut interactions: Vec<Interaction> = Vec::new();
        if let Some(arr) = args.get("interactions").and_then(|v| v.as_array()) {
            for item in arr {
                let pattern = item
                    .get("expect")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("interaction missing 'expect'"))?;
                let re = regex::Regex::new(pattern)
                    .map_err(|e| anyhow::anyhow!("invalid regex '{}': {}", pattern, e))?;
                let respond = item
                    .get("respond")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("interaction missing 'respond'"))?
                    .to_string();
                let timeout_secs = item
                    .get("timeout_secs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(30);
                interactions.push(Interaction {
                    expect: re,
                    respond,
                    timeout: Duration::from_secs(timeout_secs),
                });
            }
        }

        // Spawn PTY
        let platform = crate::platform::current_platform();
        let mut pty = platform
            .spawn_pty(command)
            .map_err(|e| anyhow::anyhow!("Failed to spawn PTY: {}", e))?;

        let deadline = Instant::now() + overall_timeout;
        let mut collected = String::new();
        let mut interaction_idx = 0;
        let mut interaction_start = Instant::now();

        loop {
            // Check overall timeout
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                collected.push_str("\n[TIMED OUT]");
                break;
            }

            // Check per-interaction timeout
            if interaction_idx < interactions.len() {
                let int_elapsed = Instant::now().duration_since(interaction_start);
                if int_elapsed > interactions[interaction_idx].timeout {
                    collected.push_str(&format!(
                        "\n[Interaction {} timed out waiting for: {}]",
                        interaction_idx,
                        interactions[interaction_idx].expect.as_str()
                    ));
                    break;
                }
            }

            // Read with a short timeout so we can re-check deadlines
            let read_timeout = remaining.min(Duration::from_millis(200));
            let read_result = tokio::time::timeout(read_timeout, pty.read()).await;

            match read_result {
                Ok(Ok(data)) => {
                    if data.is_empty() {
                        // EOF — process exited
                        break;
                    }
                    collected.push_str(&data);

                    // Check if current interaction pattern matches accumulated output
                    if interaction_idx < interactions.len() {
                        let interaction = &interactions[interaction_idx];
                        if interaction.expect.is_match(&collected) {
                            if interaction.respond == "__USER_INPUT__" {
                                // Secure passthrough — don't log the input
                                if let Err(e) = pty.passthrough_stdin().await {
                                    collected.push_str(&format!("\n[passthrough error: {}]", e));
                                    break;
                                }
                                collected.push_str("[REDACTED]");
                            } else {
                                let response = format!("{}\n", interaction.respond);
                                if let Err(e) = pty.write(&response).await {
                                    collected.push_str(&format!("\n[write error: {}]", e));
                                    break;
                                }
                            }
                            interaction_idx += 1;
                            interaction_start = Instant::now();
                        }
                    }
                }
                Ok(Err(e)) => {
                    collected.push_str(&format!("\n[read error: {}]", e));
                    break;
                }
                Err(_) => {
                    // Read timeout — loop back and check deadlines
                    continue;
                }
            }

            // Enforce output size limit
            if collected.len() > MAX_OUTPUT_BYTES {
                truncate_output(&mut collected);
                break;
            }
        }

        // Wait for process to exit (short grace period)
        let exit_code = match pty.wait(Duration::from_secs(5)).await {
            Ok(code) => code,
            Err(_) => None,
        };

        let success = exit_code == Some(0);
        truncate_output(&mut collected);

        if let Some(code) = exit_code {
            collected.push_str(&format!("\n[exit code: {}]", code));
        }

        Ok(ToolResult {
            success,
            output: collected,
            error: if success {
                None
            } else {
                Some(format!(
                    "Process exited with code: {}",
                    exit_code.map_or("unknown".to_string(), |c| c.to_string())
                ))
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let tool = PtyShellTool::new();
        assert_eq!(tool.name(), "pty_shell");
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["command"].is_object());
        assert!(schema["properties"]["interactions"].is_object());
    }

    #[tokio::test]
    async fn executes_simple_echo() {
        let tool = PtyShellTool::new();
        let result = tool
            .execute(json!({
                "command": "echo hello_pty",
                "timeout_secs": 10
            }))
            .await
            .unwrap();
        assert!(
            result.output.contains("hello_pty"),
            "output: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn handles_interaction() {
        let tool = PtyShellTool::new();
        // Use a shell script that prompts then echoes the response
        let result = tool
            .execute(json!({
                "command": "printf 'Name: ' && read name && echo \"Hello, $name\"",
                "interactions": [
                    { "expect": "Name:", "respond": "World" }
                ],
                "timeout_secs": 10
            }))
            .await
            .unwrap();
        assert!(
            result.output.contains("Hello, World"),
            "output: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn missing_command_param() {
        let tool = PtyShellTool::new();
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn timeout_kills_process() {
        let tool = PtyShellTool::new();
        let result = tool
            .execute(json!({
                "command": "sleep 60",
                "timeout_secs": 2
            }))
            .await
            .unwrap();
        assert!(
            result.output.contains("[TIMED OUT]"),
            "output: {}",
            result.output
        );
    }
}

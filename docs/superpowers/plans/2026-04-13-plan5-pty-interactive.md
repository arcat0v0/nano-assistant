# PTY Interactive Command Control Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `pty_shell` tool that can drive interactive CLI programs via pseudo-terminal, with expect/respond patterns and secure password passthrough that never exposes credentials to the LLM.

**Architecture:** A new `src/tools/pty_shell.rs` implements the `Tool` trait. It spawns commands in a PTY (via `nix::pty` or `rustix`), matches output against regex patterns, and sends responses. The special `__USER_INPUT__` sentinel triggers a passthrough mode that connects the PTY directly to the user's terminal. A `PlatformPty` trait in `src/platform/` abstracts the OS-specific PTY creation. System prompt guides the LLM to prefer non-interactive flags before falling back to PTY.

**Tech Stack:** Rust, `nix` crate (PTY), `tokio` (async I/O), `regex`

**Depends on:** Plan 1 (Platform Abstraction) — uses `Platform` for PTY trait

---

### Task 1: Add PTY dependencies and platform trait

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/platform/mod.rs`
- Modify: `src/platform/unix.rs`

- [ ] **Step 1: Add nix dependency**

In `Cargo.toml`:

```toml
nix = { version = "0.29", features = ["pty", "signal", "process"] }
```

- [ ] **Step 2: Add PtyProcess trait to platform/mod.rs**

```rust
/// A spawned PTY process.
#[async_trait::async_trait]
pub trait PtyProcess: Send {
    /// Read available output from the PTY. Returns empty string on EOF.
    async fn read(&mut self) -> Result<String, std::io::Error>;

    /// Write data to the PTY's stdin.
    async fn write(&mut self, data: &str) -> Result<(), std::io::Error>;

    /// Connect PTY stdin/stdout directly to the user's terminal.
    /// Blocks until the user finishes input (detected by next output line).
    async fn passthrough_stdin(&mut self) -> Result<(), std::io::Error>;

    /// Get exit status, or None if still running.
    fn exit_status(&self) -> Option<i32>;

    /// Wait for the process to finish, with timeout.
    async fn wait(&mut self, timeout: std::time::Duration) -> Result<Option<i32>, std::io::Error>;
}
```

Add a method to the `Platform` trait:

```rust
/// Spawn an interactive command in a PTY.
fn spawn_pty(&self, command: &str) -> Result<Box<dyn PtyProcess>, std::io::Error>;
```

- [ ] **Step 3: Implement spawn_pty for UnixPlatform**

In `src/platform/unix.rs`:

```rust
use nix::pty::{openpty, OpenptyResult};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub struct UnixPtyProcess {
    master_read: tokio::io::BufReader<tokio::fs::File>,
    master_write: tokio::fs::File,
    child: tokio::process::Child,
}

impl Platform for UnixPlatform {
    // ... existing methods ...

    fn spawn_pty(&self, command: &str) -> Result<Box<dyn super::PtyProcess>, std::io::Error> {
        let OpenptyResult { master, slave } = openpty(None, None)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let (shell, flag) = self.shell_command();

        let slave_fd = slave.as_raw_fd();
        let child = std::process::Command::new(shell)
            .arg(flag)
            .arg(command)
            .stdin(unsafe { std::process::Stdio::from_raw_fd(slave_fd) })
            .stdout(unsafe { std::process::Stdio::from_raw_fd(slave_fd) })
            .stderr(unsafe { std::process::Stdio::from_raw_fd(slave_fd) })
            .spawn()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        // Drop slave fd in parent (child has its own copy)
        drop(slave);

        let master_fd = master.as_raw_fd();
        let master_file = unsafe { std::fs::File::from_raw_fd(master_fd) };
        // Prevent the OwnedFd from closing the fd since we transferred ownership
        std::mem::forget(master);

        let master_tokio = tokio::fs::File::from_std(master_file.try_clone()?);
        let master_write = tokio::fs::File::from_std(master_file);
        let master_read = tokio::io::BufReader::new(master_tokio);

        let child = tokio::process::Child::from(child);

        Ok(Box::new(UnixPtyProcess {
            master_read,
            master_write,
            child,
        }))
    }
}

#[async_trait::async_trait]
impl super::PtyProcess for UnixPtyProcess {
    async fn read(&mut self) -> Result<String, std::io::Error> {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 4096];
        let n = self.master_read.read(&mut buf).await?;
        if n == 0 {
            return Ok(String::new());
        }
        Ok(String::from_utf8_lossy(&buf[..n]).to_string())
    }

    async fn write(&mut self, data: &str) -> Result<(), std::io::Error> {
        use tokio::io::AsyncWriteExt;
        self.master_write.write_all(data.as_bytes()).await?;
        self.master_write.flush().await
    }

    async fn passthrough_stdin(&mut self) -> Result<(), std::io::Error> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // Read from real stdin, write to PTY master
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 1];

        // Set terminal to raw mode for passthrough
        // Read one line (until \n or \r)
        loop {
            let n = stdin.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            self.master_write.write_all(&buf[..n]).await?;
            self.master_write.flush().await?;
            if buf[0] == b'\n' || buf[0] == b'\r' {
                break;
            }
        }

        Ok(())
    }

    fn exit_status(&self) -> Option<i32> {
        // Check without blocking
        None // Will be set after wait()
    }

    async fn wait(&mut self, timeout: std::time::Duration) -> Result<Option<i32>, std::io::Error> {
        match tokio::time::timeout(timeout, self.child.wait()).await {
            Ok(Ok(status)) => Ok(status.code()),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                // Timeout — kill the child
                let _ = self.child.kill().await;
                Ok(None)
            }
        }
    }
}
```

- [ ] **Step 4: Add stub for WindowsPlatform**

```rust
fn spawn_pty(&self, _command: &str) -> Result<Box<dyn super::PtyProcess>, std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "PTY not yet supported on Windows",
    ))
}
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check 2>&1 | head -20`

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml src/platform/mod.rs src/platform/unix.rs src/platform/windows.rs
git commit -m "feat(platform): add PtyProcess trait with Unix PTY implementation"
```

---

### Task 2: Create PtyShell tool

**Files:**
- Create: `src/tools/pty_shell.rs`
- Modify: `src/tools/mod.rs`

- [ ] **Step 1: Create pty_shell.rs**

```rust
use crate::tools::{Tool, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;

const DEFAULT_EXPECT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_OVERALL_TIMEOUT_SECS: u64 = 120;
const USER_INPUT_SENTINEL: &str = "__USER_INPUT__";

#[derive(Debug, Deserialize)]
struct Interaction {
    expect: String,
    respond: String,
    #[serde(default = "default_expect_timeout")]
    timeout_secs: u64,
}

fn default_expect_timeout() -> u64 {
    DEFAULT_EXPECT_TIMEOUT_SECS
}

pub struct PtyShellTool;

impl PtyShellTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for PtyShellTool {
    fn name(&self) -> &str {
        "pty_shell"
    }

    fn description(&self) -> &str {
        "Execute interactive commands via pseudo-terminal. Use for commands that require \
         user interaction (menus, selections, confirmations). Use __USER_INPUT__ as respond \
         value for password prompts — input will be collected directly from the user's terminal \
         and never sent to the AI."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute in a PTY"
                },
                "interactions": {
                    "type": "array",
                    "description": "Expected output patterns and responses",
                    "items": {
                        "type": "object",
                        "properties": {
                            "expect": {
                                "type": "string",
                                "description": "Regex pattern to match in command output"
                            },
                            "respond": {
                                "type": "string",
                                "description": "Text to send. Use __USER_INPUT__ for secure password passthrough"
                            },
                            "timeout_secs": {
                                "type": "integer",
                                "description": "Max wait for this pattern (default: 30)"
                            }
                        },
                        "required": ["expect", "respond"]
                    }
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Overall command timeout (default: 120)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let command = args["command"].as_str().unwrap_or_default();
        if command.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("command is required".to_string()),
            });
        }

        let interactions: Vec<Interaction> = args
            .get("interactions")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let overall_timeout = args["timeout_secs"]
            .as_u64()
            .unwrap_or(DEFAULT_OVERALL_TIMEOUT_SECS);

        let platform = crate::platform::current_platform();

        let mut pty = match platform.spawn_pty(command) {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to spawn PTY: {e}")),
                });
            }
        };

        let mut full_output = String::new();
        let mut interaction_idx = 0;
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_secs(overall_timeout);

        // Main interaction loop
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Ok(ToolResult {
                    success: false,
                    output: full_output,
                    error: Some("Overall timeout exceeded".to_string()),
                });
            }

            // Read output with timeout
            let read_timeout = if interaction_idx < interactions.len() {
                std::time::Duration::from_secs(interactions[interaction_idx].timeout_secs)
            } else {
                std::time::Duration::from_secs(5)
            };

            match tokio::time::timeout(read_timeout, pty.read()).await {
                Ok(Ok(chunk)) => {
                    if chunk.is_empty() {
                        // EOF — process finished
                        break;
                    }
                    full_output.push_str(&chunk);

                    // Check if current interaction pattern matches
                    if interaction_idx < interactions.len() {
                        let interaction = &interactions[interaction_idx];
                        let pattern = regex::Regex::new(&interaction.expect)
                            .unwrap_or_else(|_| {
                                regex::Regex::new(&regex::escape(&interaction.expect)).unwrap()
                            });

                        if pattern.is_match(&full_output) {
                            if interaction.respond == USER_INPUT_SENTINEL {
                                // Password passthrough — direct terminal connection
                                // DO NOT log or record the input
                                eprintln!("[nano-assistant] Waiting for your input (not sent to AI)...");
                                if let Err(e) = pty.passthrough_stdin().await {
                                    tracing::warn!("Passthrough error: {e}");
                                }
                                // Replace the password prompt area in output
                                full_output.push_str("[REDACTED user input]\n");
                            } else {
                                // Normal automated response
                                let response = format!("{}\n", interaction.respond);
                                if let Err(e) = pty.write(&response).await {
                                    return Ok(ToolResult {
                                        success: false,
                                        output: full_output,
                                        error: Some(format!("Write failed: {e}")),
                                    });
                                }
                            }
                            interaction_idx += 1;
                        }
                    }
                }
                Ok(Err(e)) => {
                    // Read error — usually means process exited
                    if !full_output.is_empty() {
                        break;
                    }
                    return Ok(ToolResult {
                        success: false,
                        output: full_output,
                        error: Some(format!("PTY read error: {e}")),
                    });
                }
                Err(_) => {
                    // Timeout on read
                    if interaction_idx >= interactions.len() {
                        // No more interactions expected, process may be done
                        break;
                    }
                    return Ok(ToolResult {
                        success: false,
                        output: full_output,
                        error: Some(format!(
                            "Timeout waiting for pattern: {}",
                            interactions[interaction_idx].expect
                        )),
                    });
                }
            }
        }

        // Wait for process to finish
        let exit_code = pty
            .wait(std::time::Duration::from_secs(10))
            .await
            .ok()
            .flatten();

        let success = exit_code.map_or(true, |c| c == 0);

        // Truncate output if too large
        const MAX_OUTPUT: usize = 1_048_576;
        if full_output.len() > MAX_OUTPUT {
            full_output.truncate(MAX_OUTPUT);
            full_output.push_str("\n[... output truncated]");
        }

        Ok(ToolResult {
            success,
            output: full_output,
            error: if success {
                None
            } else {
                Some(format!("Process exited with code: {:?}", exit_code))
            },
        })
    }
}
```

- [ ] **Step 2: Register PtyShellTool in tools/mod.rs**

Add to the module declarations:

```rust
pub mod pty_shell;
```

Add to `default_tools()`:

```rust
Box::new(pty_shell::PtyShellTool::new()),
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check 2>&1 | head -20`

- [ ] **Step 4: Commit**

```bash
git add src/tools/pty_shell.rs src/tools/mod.rs
git commit -m "feat(tools): add pty_shell tool for interactive command control"
```

---

### Task 3: Add non-interactive priority guidance to system prompt

**Files:**
- Modify: `src/agent/prompt.rs`

- [ ] **Step 1: Add command execution guidance**

In the `build` function, add after the self-management section:

```rust
prompt.push_str("## Command Execution\n\n");
prompt.push_str("Always prefer non-interactive command flags over pty_shell:\n");
prompt.push_str("- `apt install -y`, `yum install -y`, `pacman --noconfirm`\n");
prompt.push_str("- `yes | command`, `echo \"choice\" | command`\n");
prompt.push_str("- `--batch`, `--non-interactive`, `--yes`, `--no-confirm` flags\n");
prompt.push_str("Only use `pty_shell` when no non-interactive option exists.\n");
prompt.push_str("For password prompts, use `__USER_INPUT__` — it collects input directly ");
prompt.push_str("from the user's terminal without exposing it to the AI.\n\n");
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`

- [ ] **Step 3: Commit**

```bash
git add src/agent/prompt.rs
git commit -m "feat(prompt): add non-interactive priority guidance for command execution"
```

---

### Task 4: End-to-end verification

**Files:** None

- [ ] **Step 1: Build**

Run: `cargo build 2>&1 | tail -5`

- [ ] **Step 2: Test basic PTY execution**

In interactive mode, ask nana to run a simple interactive command like:

```
Use pty_shell to run "read -p 'Enter name: ' name && echo Hello $name" with interaction: expect "Enter name" respond "World"
```

Expected: PTY executes, matches pattern, sends "World", output contains "Hello World".

- [ ] **Step 3: Test non-interactive preference**

Ask nana to install a package. Verify it uses `apt install -y` (or equivalent) rather than `pty_shell`.

- [ ] **Step 4: Verify __USER_INPUT__ passthrough works**

Test with a sudo command. Verify the tool prompts for direct terminal input and doesn't send the password to the LLM conversation.

- [ ] **Step 5: Full test suite**

Run: `cargo test 2>&1 | tail -10`
Expected: all tests pass

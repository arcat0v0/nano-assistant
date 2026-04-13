# Self-Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable nana to install skills (via skills.sh), configure MCP servers (by editing config.toml), and manage its own MEMORY.md — with hot-reload for skills and MCP after changes.

**Architecture:** No new tools. The agent loop gains post-tool-execution hooks that detect when shell commands install skills or file_edit modifies MCP config, then triggers `rescan_skills()` or `reload_mcp()` on the Agent. System prompt is extended with self-management instructions so the LLM knows how to use these capabilities.

**Tech Stack:** Rust, regex for command detection, existing tool infrastructure

**Depends on:** Plan 2 (Skill Versioning) — builtin protection during rescan

---

### Task 1: Add rescan_skills method to Agent

**Files:**
- Modify: `src/agent/loop_.rs`

- [ ] **Step 1: Add rescan_skills method**

Add to the `impl Agent` block:

```rust
/// Rescan skill directories and load newly installed skills.
/// Returns names of newly discovered skills.
/// Respects builtin protection — new skills with builtin names are rejected.
pub fn rescan_skills(&mut self, config: &crate::config::SkillsConfig) -> Vec<String> {
    let fresh = crate::skills::load_skills(config);
    let existing_names: std::collections::HashSet<&str> =
        self.skills.iter().map(|s| s.name.as_str()).collect();

    let mut new_names = Vec::new();

    for skill in fresh {
        if !existing_names.contains(skill.name.as_str()) {
            // Convert skill to tools and add them
            let skill_tools = crate::skills::skills_to_tools(&[skill.clone()]);
            for tool in skill_tools {
                let spec = tool.spec();
                self.tools.push(tool);
                self.tool_specs.push(spec);
            }
            new_names.push(skill.name.clone());
            self.skills.push(skill);
        }
    }

    new_names
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check 2>&1 | head -10`

- [ ] **Step 3: Commit**

```bash
git add src/agent/loop_.rs
git commit -m "feat(agent): add rescan_skills for hot-reloading new skills"
```

---

### Task 2: Add reload_mcp method to Agent

**Files:**
- Modify: `src/agent/loop_.rs`

- [ ] **Step 1: Add ReloadResult type and reload_mcp method**

Add the type near the top of the file:

```rust
/// Result of an MCP reload attempt.
#[derive(Debug)]
pub enum McpReloadResult {
    Success { new_servers: Vec<String> },
    PartialFailure {
        connected: Vec<String>,
        failed: Vec<(String, String)>,
    },
    Disabled,
}
```

Add the method to `impl Agent`:

```rust
/// Reload MCP configuration. Reads config.toml, connects to any new servers.
/// Existing connections are preserved. Only new servers are connected.
pub async fn reload_mcp(&mut self) -> McpReloadResult {
    let config_path = crate::config::default_config_path();
    let config = match std::fs::read_to_string(&config_path) {
        Ok(content) => match toml::from_str::<crate::config::Config>(&content) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Failed to parse config for MCP reload: {e}");
                return McpReloadResult::PartialFailure {
                    connected: vec![],
                    failed: vec![("config".to_string(), e.to_string())],
                };
            }
        },
        Err(e) => {
            tracing::error!("Failed to read config for MCP reload: {e}");
            return McpReloadResult::PartialFailure {
                connected: vec![],
                failed: vec![("config".to_string(), e.to_string())],
            };
        }
    };

    if !config.mcp.enabled {
        return McpReloadResult::Disabled;
    }

    // Find servers not already connected (by name)
    let existing_tool_names: std::collections::HashSet<&str> =
        self.tools.iter().map(|t| t.name()).collect();

    let new_servers: Vec<_> = config
        .mcp
        .servers
        .iter()
        .filter(|s| {
            // Check if any tool with this server's prefix exists
            !existing_tool_names.iter().any(|name| {
                name.starts_with(&format!("{}__", s.name))
            }) && !self.deferred_tool_names.iter().any(|name| {
                name.starts_with(&format!("{}__", s.name))
            })
        })
        .cloned()
        .collect();

    if new_servers.is_empty() {
        return McpReloadResult::Success {
            new_servers: vec![],
        };
    }

    let mut connected = Vec::new();
    let mut failed = Vec::new();

    for server_config in &new_servers {
        match crate::mcp::McpRegistry::connect_all(&[server_config.clone()]).await {
            Ok(registry) => {
                let registry = std::sync::Arc::new(registry);
                let names = registry.tool_names();
                for name in &names {
                    if let Some(def) = registry.get_tool_def(name).await {
                        let tool = Box::new(crate::mcp::McpToolWrapper::new(
                            name.clone(),
                            def,
                            std::sync::Arc::clone(&registry),
                        ));
                        let spec = tool.spec();
                        self.tools.push(tool);
                        self.tool_specs.push(spec);
                    }
                }
                connected.push(server_config.name.clone());
            }
            Err(e) => {
                failed.push((server_config.name.clone(), e.to_string()));
            }
        }
    }

    if failed.is_empty() {
        McpReloadResult::Success {
            new_servers: connected,
        }
    } else {
        McpReloadResult::PartialFailure { connected, failed }
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check 2>&1 | head -10`

- [ ] **Step 3: Commit**

```bash
git add src/agent/loop_.rs
git commit -m "feat(agent): add reload_mcp for hot-reloading MCP servers"
```

---

### Task 3: Add post-tool-execution hooks in agent loop

**Files:**
- Modify: `src/agent/loop_.rs`

- [ ] **Step 1: Add command detection helpers**

```rust
/// Check if a shell command is a skill install command.
fn is_skill_install_command(command: &str) -> bool {
    let pattern = regex::Regex::new(r"(?:npx\s+)?skills\s+(add|install)\b").unwrap();
    pattern.is_match(command)
}

/// Check if a file_edit targets MCP config.
fn is_mcp_config_edit(tool_name: &str, args: &serde_json::Value) -> bool {
    if tool_name != "file_edit" {
        return false;
    }
    let path = args["file_path"].as_str().unwrap_or_default();
    if !path.ends_with("config.toml") {
        return false;
    }
    let old_str = args["old_string"].as_str().unwrap_or_default();
    let new_str = args["new_string"].as_str().unwrap_or_default();
    old_str.contains("[mcp")
        || old_str.contains("[[mcp.servers]]")
        || new_str.contains("[mcp")
        || new_str.contains("[[mcp.servers]]")
}
```

- [ ] **Step 2: Add regex dependency if not already present**

Check Cargo.toml for `regex`. If not present, add:

```toml
regex = "1"
```

- [ ] **Step 3: Integrate hooks into execute_tools**

Modify the `execute_tools` method. After each tool execution result, check for reload triggers. Change the method to `&mut self` (it's currently `&self`):

```rust
async fn execute_tools(
    &mut self,
    calls: &[crate::agent::dispatcher::ParsedToolCall],
) -> Result<Vec<ToolExecutionResult>> {
    let mut results = Vec::with_capacity(calls.len());
    let mut needs_skill_rescan = false;
    let mut needs_mcp_reload = false;

    for call in calls {
        // ... existing tool dispatch logic (unchanged) ...

        // Post-execution hooks
        if result.success {
            if call.name == "shell" || call.name == "execute_command" {
                if let Some(cmd) = call.arguments["command"].as_str() {
                    if is_skill_install_command(cmd) {
                        needs_skill_rescan = true;
                    }
                }
            }
            if is_mcp_config_edit(&call.name, &call.arguments) {
                needs_mcp_reload = true;
            }
        }

        results.push(result);
    }

    // Apply hot reloads
    if needs_skill_rescan {
        let new_skills = self.rescan_skills(&self.config.skills.clone());
        if !new_skills.is_empty() {
            tracing::info!("Hot-reloaded skills: {:?}", new_skills);
            results.push(ToolExecutionResult {
                name: "_system".to_string(),
                output: format!(
                    "Skills auto-reloaded. New skills available: {}",
                    new_skills.join(", ")
                ),
                success: true,
                tool_call_id: None,
            });
        }
    }

    if needs_mcp_reload {
        let reload_result = self.reload_mcp().await;
        let msg = match &reload_result {
            McpReloadResult::Success { new_servers } if !new_servers.is_empty() => {
                format!(
                    "MCP config reloaded. New servers connected: {}",
                    new_servers.join(", ")
                )
            }
            McpReloadResult::Success { .. } => {
                "MCP config saved. No new servers to connect.".to_string()
            }
            McpReloadResult::PartialFailure { connected, failed } => {
                let mut msg = String::new();
                if !connected.is_empty() {
                    msg.push_str(&format!("Connected: {}. ", connected.join(", ")));
                }
                for (name, err) in failed {
                    msg.push_str(&format!("Failed to connect '{}': {}. ", name, err));
                }
                msg.push_str("Try restarting na if the server is not responding.");
                msg
            }
            McpReloadResult::Disabled => {
                "MCP config saved but MCP is disabled in config.".to_string()
            }
        };
        results.push(ToolExecutionResult {
            name: "_system".to_string(),
            output: msg,
            success: true,
            tool_call_id: None,
        });
    }

    Ok(results)
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check 2>&1 | head -20`

Note: changing `execute_tools` from `&self` to `&mut self` may require updating callers. The `turn` and `turn_streamed` methods call `self.execute_tools()` — since `self` is already `&mut self` in those methods, this should work.

- [ ] **Step 5: Commit**

```bash
git add src/agent/loop_.rs Cargo.toml
git commit -m "feat(agent): add post-tool-execution hooks for skill and MCP hot-reload"
```

---

### Task 4: Add self-management instructions to system prompt

**Files:**
- Modify: `src/agent/prompt.rs`

- [ ] **Step 1: Add self-management section to system prompt builder**

In the `build` function, after the safety section, add:

```rust
// Self-management instructions
prompt.push_str("\n## Self-Management Capabilities\n\n");
prompt.push_str("### Skill Installation\n");
prompt.push_str("You can install community skills from the skills.sh ecosystem:\n");
prompt.push_str("1. Search: execute `npx skills search \"<keyword>\"`\n");
prompt.push_str("2. Install: execute `npx skills add <package> -g`\n");
prompt.push_str("3. Skills auto-reload after installation — no restart needed.\n");
prompt.push_str("Do NOT modify or override builtin skills.\n\n");

prompt.push_str("### MCP Server Configuration\n");
let config_path = crate::platform::current_platform().config_path();
prompt.push_str(&format!(
    "You can add MCP servers by editing {}.\n",
    config_path.display()
));
prompt.push_str("Add a `[[mcp.servers]]` section with name, transport, command, and args.\n");
prompt.push_str("Config auto-reloads after edit. If reload fails, suggest user restart na.\n\n");

prompt.push_str("### Memory Management\n");
let memory_path = crate::platform::current_platform().memory_md_path();
prompt.push_str(&format!(
    "Your persistent memory file: {}\n",
    memory_path.display()
));
prompt.push_str("You can read and edit this file to persist information across sessions.\n");
prompt.push_str("Use Markdown with ## headers as categories.\n\n");
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check 2>&1 | head -10`

- [ ] **Step 3: Commit**

```bash
git add src/agent/prompt.rs
git commit -m "feat(prompt): add self-management instructions to system prompt"
```

---

### Task 5: End-to-end verification

**Files:** None

- [ ] **Step 1: Build**

Run: `cargo build 2>&1 | tail -5`

- [ ] **Step 2: Verify system prompt contains self-management section**

Run in interactive mode, ask nana to describe its self-management capabilities. It should mention skill installation, MCP configuration, and memory management.

- [ ] **Step 3: Test skill install hot-reload (if npx available)**

In interactive mode, ask nana to search and install a skill. After installation, verify the skill appears in the session without restart.

- [ ] **Step 4: Test MCP config edit**

Ask nana to add a new MCP server to config.toml. Verify hot-reload is attempted and status is reported.

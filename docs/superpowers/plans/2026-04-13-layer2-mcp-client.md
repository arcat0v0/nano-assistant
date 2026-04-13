# Layer 2: MCP Client Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port ZeroClaw's MCP client to nano-assistant, enabling connection to any MCP server (Exa, Context7, Grep.app, etc.) with deferred loading support.

**Architecture:** Copy 6 MCP modules from `../zeroclaw/src/tools/` into `src/mcp/`, adapting only ZeroClaw-specific references. The Tool trait and ToolResult are identical between both projects, so the bridge layer (`McpToolWrapper`) needs minimal changes. Agent loop integration adds `ActivatedToolSet` for deferred MCP tool resolution.

**Tech Stack:** Rust, reqwest (existing), tokio (existing), async-trait (existing), serde_json (existing). No new dependencies.

**Source reference:** ZeroClaw MCP modules at `/home/arcat/Develop/nano-assistant/zeroclaw/src/tools/`

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `src/mcp/mod.rs` | Create | Module declarations and re-exports |
| `src/mcp/protocol.rs` | Create (copy) | JSON-RPC 2.0 types, MCP protocol constants |
| `src/mcp/transport.rs` | Create (copy) | Stdio/HTTP/SSE transport implementations |
| `src/mcp/client.rs` | Create (copy+adapt) | McpServer + McpRegistry |
| `src/mcp/tool.rs` | Create (copy+adapt) | McpToolWrapper bridging MCP to Tool trait |
| `src/mcp/deferred.rs` | Create (copy) | Deferred loading + ActivatedToolSet |
| `src/mcp/tool_search.rs` | Create (copy) | ToolSearchTool for LLM-driven tool discovery |
| `src/config/schema.rs` | Modify | Add McpConfig, McpServerConfig, McpTransport |
| `src/config/mod.rs` | Modify | Re-export new config types |
| `src/lib.rs` | Modify | Add `pub mod mcp;` |
| `src/agent/loop_.rs` | Modify | MCP init, deferred prompt, tool execution fallback |
| `src/agent/prompt.rs` | Modify | Add deferred tools section to system prompt |

---

### Task 1: Create MCP module structure + protocol.rs

**Files:**
- Create: `src/mcp/mod.rs`
- Create: `src/mcp/protocol.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Create src/mcp/ directory and mod.rs**

Create `src/mcp/mod.rs`:

```rust
pub mod protocol;
pub mod transport;
pub mod client;
pub mod tool;
pub mod deferred;
pub mod tool_search;

pub use client::McpRegistry;
pub use deferred::{ActivatedToolSet, DeferredMcpToolSet};
pub use tool::McpToolWrapper;
pub use tool_search::ToolSearchTool;
```

- [ ] **Step 2: Copy protocol.rs from ZeroClaw**

```bash
cp ../zeroclaw/src/tools/mcp_protocol.rs src/mcp/protocol.rs
```

This file has **zero ZeroClaw-specific references** — it's pure JSON-RPC 2.0 types. No modifications needed.

Verify the file contains: `JsonRpcRequest`, `JsonRpcResponse`, `JsonRpcError`, `McpToolDef`, `McpToolsListResult`, and the constants `JSONRPC_VERSION`, `MCP_PROTOCOL_VERSION`.

- [ ] **Step 3: Fix imports in protocol.rs**

The file should only use `serde` and `serde_json`. If it references any ZeroClaw-specific crate paths, fix them. The imports should be:

```rust
use serde::{Deserialize, Serialize};
```

No other crate-level imports needed.

- [ ] **Step 4: Add mcp module to lib.rs**

In `src/lib.rs`, add after the existing module declarations:

```rust
pub mod mcp;
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check 2>&1 | tail -10`

This will fail with missing module errors for transport/client/tool/deferred/tool_search — that's expected. But protocol.rs itself should compile. To verify just protocol:

Run: `cargo check 2>&1 | grep "protocol" | head -5`
Expected: No errors from protocol.rs specifically. Errors should only be about missing sibling modules.

- [ ] **Step 6: Commit**

```bash
git add src/mcp/ src/lib.rs
git commit -m "feat(mcp): add protocol module with JSON-RPC 2.0 types"
```

---

### Task 2: Port transport.rs

**Files:**
- Create: `src/mcp/transport.rs`

- [ ] **Step 1: Copy transport.rs from ZeroClaw**

```bash
cp ../zeroclaw/src/tools/mcp_transport.rs src/mcp/transport.rs
```

- [ ] **Step 2: Fix imports**

Replace any ZeroClaw crate-path imports. The file should reference the protocol module as a sibling:

Change any occurrence of:
- `crate::tools::mcp_protocol::` → `super::protocol::`
- `crate::tools::McpServerConfig` or similar → `crate::config::McpServerConfig`
- `use crate::config::schema::McpTransport` → `use crate::config::McpTransport`

The key imports should be:
```rust
use super::protocol::{JsonRpcRequest, JsonRpcResponse, McpToolDef};
use crate::config::{McpServerConfig, McpTransport};
```

Note: `McpServerConfig` and `McpTransport` don't exist in nano-assistant's config yet (Task 5 adds them). For now, just fix the import paths — the file won't compile until Task 5.

- [ ] **Step 3: Verify no ZeroClaw-specific logic remains**

Search: `grep -n "zeroclaw\|ZeroClaw\|approved\|observer\|Observer\|CancellationToken" src/mcp/transport.rs`
Expected: No matches

- [ ] **Step 4: Commit**

```bash
git add src/mcp/transport.rs
git commit -m "feat(mcp): add Stdio/HTTP/SSE transport layer"
```

---

### Task 3: Port client.rs

**Files:**
- Create: `src/mcp/client.rs`

- [ ] **Step 1: Copy client.rs from ZeroClaw**

```bash
cp ../zeroclaw/src/tools/mcp_client.rs src/mcp/client.rs
```

- [ ] **Step 2: Fix imports**

Change:
- `crate::tools::mcp_protocol::` → `super::protocol::`
- `crate::tools::mcp_transport::` → `super::transport::`
- `crate::config::schema::McpServerConfig` → `crate::config::McpServerConfig`

- [ ] **Step 3: Change client identifier**

Find the `"initialize"` JSON-RPC request (the `clientInfo` block) and change:

```rust
// Before:
"name": "zeroclaw",

// After:
"name": "nano-assistant",
```

- [ ] **Step 4: Verify changes**

Run: `grep -n "zeroclaw\|ZeroClaw" src/mcp/client.rs`
Expected: No matches

- [ ] **Step 5: Commit**

```bash
git add src/mcp/client.rs
git commit -m "feat(mcp): add MCP server connection and registry"
```

---

### Task 4: Port tool.rs (McpToolWrapper)

**Files:**
- Create: `src/mcp/tool.rs`

- [ ] **Step 1: Copy tool.rs from ZeroClaw**

```bash
cp ../zeroclaw/src/tools/mcp_tool.rs src/mcp/tool.rs
```

- [ ] **Step 2: Fix imports**

Change:
- `crate::tools::mcp_protocol::McpToolDef` → `super::protocol::McpToolDef`
- `crate::tools::mcp_client::McpRegistry` → `super::client::McpRegistry`
- `crate::tools::traits::{Tool, ToolResult}` → `crate::tools::{Tool, ToolResult}`

- [ ] **Step 3: Remove "approved" field stripping**

In the `execute()` method, find and **remove** this block:

```rust
// Remove this entire block:
let args = match args {
    serde_json::Value::Object(mut map) => {
        map.remove("approved");
        serde_json::Value::Object(map)
    }
    other => other,
};
```

Replace with just passing `args` directly to `call_tool`. The execute method should be:

```rust
async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
    match self.registry.call_tool(&self.prefixed_name, args).await {
        Ok(output) => Ok(ToolResult {
            success: true,
            output,
            error: None,
        }),
        Err(e) => Ok(ToolResult {
            success: false,
            output: String::new(),
            error: Some(e.to_string()),
        }),
    }
}
```

- [ ] **Step 4: Commit**

```bash
git add src/mcp/tool.rs
git commit -m "feat(mcp): add McpToolWrapper bridging MCP to Tool trait"
```

---

### Task 5: Add MCP config types

**Files:**
- Modify: `src/config/schema.rs`
- Modify: `src/config/mod.rs`

- [ ] **Step 1: Write failing tests**

Add to the `mod tests` block in `src/config/schema.rs`:

```rust
#[test]
fn mcp_config_default() {
    let m = McpConfig::default();
    assert!(!m.enabled);
    assert!(m.deferred_loading);
    assert!(m.servers.is_empty());
}

#[test]
fn mcp_transport_default_is_stdio() {
    let t = McpTransport::default();
    assert_eq!(t, McpTransport::Stdio);
}

#[test]
fn toml_deserialization_mcp_config() {
    let toml_str = r#"
        [mcp]
        enabled = true

        [[mcp.servers]]
        name = "context7"
        command = "npx"
        args = ["-y", "@upstash/context7-mcp@latest"]
    "#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(config.mcp.enabled);
    assert_eq!(config.mcp.servers.len(), 1);
    assert_eq!(config.mcp.servers[0].name, "context7");
    assert_eq!(config.mcp.servers[0].transport, McpTransport::Stdio);
}

#[test]
fn toml_deserialization_mcp_server_with_env() {
    let toml_str = r#"
        [mcp]
        enabled = true

        [[mcp.servers]]
        name = "exa"
        command = "npx"
        args = ["-y", "exa-mcp-server"]

        [mcp.servers.env]
        EXA_API_KEY = "test-key"
    "#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.mcp.servers[0].env.get("EXA_API_KEY").unwrap(), "test-key");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib config::schema::tests::mcp_config_default 2>&1 | tail -5`
Expected: FAIL — types don't exist yet

- [ ] **Step 3: Add MCP config types**

Add these types in `src/config/schema.rs` after `SkillsConfig`:

```rust
/// MCP transport protocol.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    #[default]
    Stdio,
    Http,
    Sse,
}

/// Configuration for a single MCP server.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct McpServerConfig {
    /// Server name, used as tool name prefix (e.g., "context7" → "context7__query_docs").
    pub name: String,

    /// Transport protocol. Default: Stdio.
    #[serde(default)]
    pub transport: McpTransport,

    /// URL for HTTP/SSE transports.
    #[serde(default)]
    pub url: Option<String>,

    /// Command to spawn for Stdio transport.
    #[serde(default)]
    pub command: String,

    /// Arguments for the Stdio command.
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment variables for Stdio command.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,

    /// HTTP headers for HTTP/SSE transports.
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,

    /// Per-tool call timeout in seconds. Default: 180, max: 600.
    #[serde(default)]
    pub tool_timeout_secs: Option<u64>,
}

/// MCP (Model Context Protocol) configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct McpConfig {
    /// Enable MCP server connections. Default: false.
    #[serde(default)]
    pub enabled: bool,

    /// Use deferred loading (register tool_search instead of all tools). Default: true.
    #[serde(default = "default_true")]
    pub deferred_loading: bool,

    /// MCP server definitions.
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            deferred_loading: true,
            servers: Vec::new(),
        }
    }
}
```

Add `mcp` field to the `Config` struct:

```rust
pub struct Config {
    // ... existing fields ...

    /// MCP server configuration.
    #[serde(default)]
    pub mcp: McpConfig,
}
```

- [ ] **Step 4: Update config/mod.rs re-exports**

In `src/config/mod.rs`, add to the `pub use` line:

```rust
pub use schema::{
    BehaviorConfig, Config, McpConfig, McpServerConfig, McpTransport,
    MemoryConfig, ProviderConfig, SecurityConfig, SkillsConfig,
};
```

- [ ] **Step 5: Run tests**

Run: `cargo test --lib config::schema::tests 2>&1 | tail -10`
Expected: All tests pass (existing + 4 new)

- [ ] **Step 6: Commit**

```bash
git add src/config/schema.rs src/config/mod.rs
git commit -m "feat(config): add MCP server configuration types"
```

---

### Task 6: Port deferred.rs and tool_search.rs

**Files:**
- Create: `src/mcp/deferred.rs`
- Create: `src/mcp/tool_search.rs`

- [ ] **Step 1: Copy deferred.rs from ZeroClaw**

```bash
cp ../zeroclaw/src/tools/mcp_deferred.rs src/mcp/deferred.rs
```

Fix imports:
- `crate::tools::mcp_protocol::McpToolDef` → `super::protocol::McpToolDef`
- `crate::tools::mcp_client::McpRegistry` → `super::client::McpRegistry`
- `crate::tools::mcp_tool::McpToolWrapper` → `super::tool::McpToolWrapper`
- `crate::tools::traits::{Tool, ToolResult, ToolSpec}` → `crate::tools::{Tool, ToolResult, ToolSpec}`

- [ ] **Step 2: Copy tool_search.rs from ZeroClaw**

```bash
cp ../zeroclaw/src/tools/tool_search.rs src/mcp/tool_search.rs
```

Fix imports:
- `super::mcp_deferred::` → `super::deferred::`
- `super::traits::{Tool, ToolResult, ToolSpec}` → `crate::tools::{Tool, ToolResult, ToolSpec}`
- Any other `crate::tools::` references → appropriate `super::` or `crate::` paths

- [ ] **Step 3: Verify no ZeroClaw-specific references remain**

Run:
```bash
grep -rn "zeroclaw\|ZeroClaw\|approved\|observer\|Observer\|CancellationToken" src/mcp/
```
Expected: No matches (except possibly in comments, which are fine)

- [ ] **Step 4: Verify compilation**

Run: `cargo check 2>&1 | tail -10`
Expected: `Finished` — all 6 MCP modules should now compile together

- [ ] **Step 5: Run any existing tests in the ported modules**

Run: `cargo test --lib mcp 2>&1 | tail -10`
Check: any tests ported from ZeroClaw should pass

- [ ] **Step 6: Commit**

```bash
git add src/mcp/deferred.rs src/mcp/tool_search.rs
git commit -m "feat(mcp): add deferred loading and tool_search"
```

---

### Task 7: Agent loop integration

**Files:**
- Modify: `src/agent/loop_.rs`
- Modify: `src/agent/prompt.rs`

This is the most complex task — connecting the MCP client to the agent's execution pipeline.

- [ ] **Step 1: Add MCP fields to Agent struct**

In `src/agent/loop_.rs`, add to the `Agent` struct:

```rust
pub struct Agent {
    provider: Arc<dyn Provider>,
    tools: Vec<Box<dyn Tool>>,
    tool_specs: Vec<ToolSpec>,
    memory: Option<Arc<dyn Memory>>,
    config: Config,
    history: ConversationHistory,
    dispatcher: Box<dyn ToolDispatcher>,
    last_visible_len: usize,
    skills: Vec<Skill>,
    system_info: Option<String>,
    // MCP support
    activated_tools: Option<Arc<std::sync::Mutex<crate::mcp::ActivatedToolSet>>>,
    deferred_tool_names: Vec<String>,
}
```

- [ ] **Step 2: Add MCP initialization to Agent constructors**

Add a new async constructor that handles MCP setup. Add this method to `impl Agent`:

```rust
/// Create an agent with MCP server support.
pub async fn with_mcp(
    provider: Arc<dyn Provider>,
    mut tools: Vec<Box<dyn Tool>>,
    memory: Option<Arc<dyn Memory>>,
    config: Config,
    skills: Vec<Skill>,
    system_info: Option<String>,
) -> Self {
    let mut activated_tools = None;
    let mut deferred_tool_names = Vec::new();

    if config.mcp.enabled && !config.mcp.servers.is_empty() {
        match crate::mcp::McpRegistry::connect_all(&config.mcp.servers).await {
            Ok(registry) => {
                let registry = Arc::new(registry);

                if config.mcp.deferred_loading {
                    let deferred = crate::mcp::DeferredMcpToolSet::from_registry(
                        Arc::clone(&registry),
                    ).await;
                    deferred_tool_names = deferred.stubs.iter()
                        .map(|s| s.prefixed_name.clone())
                        .collect();

                    let activated = Arc::new(std::sync::Mutex::new(
                        crate::mcp::ActivatedToolSet::new(),
                    ));

                    tools.push(Box::new(crate::mcp::ToolSearchTool::new(
                        deferred,
                        Arc::clone(&activated),
                    )));

                    activated_tools = Some(activated);
                } else {
                    // Eager: register all MCP tools directly
                    let names = registry.tool_names();
                    for name in names {
                        if let Some(def) = registry.get_tool_def(&name).await {
                            tools.push(Box::new(
                                crate::mcp::McpToolWrapper::new(
                                    name, def, Arc::clone(&registry),
                                ),
                            ));
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!("MCP registry connection failed: {e:#}");
            }
        }
    }

    let tool_specs: Vec<ToolSpec> = tools.iter().map(|t| t.spec()).collect();
    let dispatcher = create_dispatcher(provider.supports_native_tools());

    Self {
        provider,
        tools,
        tool_specs,
        memory,
        config,
        history: ConversationHistory::new(),
        dispatcher,
        last_visible_len: 0,
        skills,
        system_info,
        activated_tools,
        deferred_tool_names,
    }
}
```

Update existing constructors to initialize the new fields:

```rust
activated_tools: None,
deferred_tool_names: Vec::new(),
```

- [ ] **Step 3: Update execute_tools to check ActivatedToolSet**

Replace the `execute_tools` method:

```rust
async fn execute_tools(
    &self,
    calls: &[crate::agent::dispatcher::ParsedToolCall],
) -> Result<Vec<ToolExecutionResult>> {
    let mut results = Vec::with_capacity(calls.len());

    for call in calls {
        // 1. Try static tool registry
        let static_tool = self.tools.iter().find(|t| t.name() == call.name);

        // 2. Try activated MCP tools (deferred loading)
        let activated_arc = if static_tool.is_none() {
            self.activated_tools.as_ref().and_then(|at| {
                at.lock().unwrap().get_resolved(&call.name)
            })
        } else {
            None
        };

        let tool: Option<&dyn Tool> = static_tool
            .map(|t| t.as_ref())
            .or(activated_arc.as_deref());

        let result = match tool {
            Some(t) => match t.execute(call.arguments.clone()).await {
                Ok(tool_result) => ToolExecutionResult {
                    name: call.name.clone(),
                    output: tool_result.output,
                    success: tool_result.success,
                    tool_call_id: call.tool_call_id.clone(),
                },
                Err(e) => ToolExecutionResult {
                    name: call.name.clone(),
                    output: format!("Tool execution error: {e}"),
                    success: false,
                    tool_call_id: call.tool_call_id.clone(),
                },
            },
            None => ToolExecutionResult {
                name: call.name.clone(),
                output: format!("Unknown tool: {}", call.name),
                success: false,
                tool_call_id: call.tool_call_id.clone(),
            },
        };

        results.push(result);
    }

    Ok(results)
}
```

- [ ] **Step 4: Rebuild tool_specs each turn to include activated tools**

In the `turn()` and `turn_streamed()` methods, after tool execution and before the next LLM call, rebuild tool_specs to include newly activated tools:

Add a helper method:

```rust
fn current_tool_specs(&self) -> Vec<ToolSpec> {
    let mut specs: Vec<ToolSpec> = self.tools.iter().map(|t| t.spec()).collect();
    if let Some(at) = &self.activated_tools {
        for spec in at.lock().unwrap().tool_specs() {
            if !specs.iter().any(|s| s.name == spec.name) {
                specs.push(spec);
            }
        }
    }
    specs
}
```

In `call_llm`, use `self.current_tool_specs()` instead of `self.tool_specs`:

```rust
async fn call_llm(&self, messages: &[ChatMessage], model: &str, temperature: f64) -> Result<ChatResponse> {
    let specs = self.current_tool_specs();
    let tools = if self.dispatcher.should_send_tool_specs() {
        Some(specs.as_slice())
    } else {
        None
    };
    self.provider
        .chat(ChatRequest { messages, tools }, model, temperature)
        .await
}
```

- [ ] **Step 5: Add deferred tools section to system prompt**

In `src/agent/prompt.rs`, add a new field to `PromptContext`:

```rust
pub struct PromptContext<'a> {
    // ... existing fields ...
    /// Deferred MCP tool names (not yet activated).
    pub deferred_tool_names: &'a [String],
}
```

Add a new section builder:

```rust
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
```

Add it to the `build()` method's section list, between skills and protocol:

```rust
let deferred = build_deferred_tools_section(ctx);

for section in [&datetime, &system_info, &tools, &skills, &deferred, &protocol, &safety] {
```

- [ ] **Step 6: Update build_system_prompt in loop_.rs**

Update the `build_system_prompt` method in `Agent` to pass `deferred_tool_names`:

```rust
fn build_system_prompt(&self) -> String {
    let ctx = PromptContext {
        tools: &self.tools,
        tool_specs: &self.tool_specs,
        native_tool_calling: self.provider.supports_native_tools(),
        dispatcher_instructions: self.dispatcher.instructions(),
        skills: &self.skills,
        system_info: self.system_info.as_deref(),
        deferred_tool_names: &self.deferred_tool_names,
    };
    SystemPromptBuilder::build(&ctx)
}
```

- [ ] **Step 7: Verify compilation**

Run: `cargo check 2>&1 | tail -10`
Expected: `Finished` with no errors

- [ ] **Step 8: Run full test suite**

Run: `cargo test 2>&1 | tail -15`
Expected: All tests pass. Existing agent tests should work because `activated_tools: None` and `deferred_tool_names: Vec::new()` means MCP is inactive by default.

- [ ] **Step 9: Commit**

```bash
git add src/agent/loop_.rs src/agent/prompt.rs
git commit -m "feat(agent): integrate MCP client with deferred loading support"
```

---

### Task 8: Integration smoke test

**Files:**
- No permanent changes

- [ ] **Step 1: Verify full compilation**

Run: `cargo check 2>&1 | tail -5`
Expected: `Finished`

- [ ] **Step 2: Run full test suite**

Run: `cargo test 2>&1 | tail -15`
Expected: All tests pass

- [ ] **Step 3: Verify binary runs**

Run: `cargo run -- --help 2>&1 | head -5`
Expected: Shows help output

- [ ] **Step 4: Commit any fixups**

If any fixes were needed:
```bash
git add -u
git commit -m "fix: layer 2 integration fixups"
```

---

## Summary

| Task | What | Complexity |
|------|------|-----------|
| 1 | Module structure + protocol.rs | Simple copy |
| 2 | transport.rs | Copy + fix imports |
| 3 | client.rs | Copy + fix imports + rename |
| 4 | tool.rs | Copy + fix imports + remove approved |
| 5 | MCP config types | New code |
| 6 | deferred.rs + tool_search.rs | Copy + fix imports |
| 7 | Agent loop integration | New code (most complex) |
| 8 | Integration smoke test | Verification |

**Key adaptation points:**
- Import paths: `crate::tools::mcp_*` → `super::` or `crate::mcp::`
- Client name: `"zeroclaw"` → `"nano-assistant"`
- Remove `approved` field stripping from McpToolWrapper
- Tool trait is identical — no bridging changes needed

# nano-assistant v0.3.0 Design Spec

> Knowledge Sources, Skill Versioning, Self-Management, Interactive Commands, Platform Abstraction

**Date:** 2026-04-13  
**Status:** Draft  
**Target Version:** 0.3.0

---

## Table of Contents

1. [Knowledge Source System](#1-knowledge-source-system)
2. [Skill Version System & Builtin Protection](#2-skill-version-system--builtin-protection)
3. [Self-Management](#3-self-management)
4. [Interactive Command Control](#4-interactive-command-control)
5. [Platform Abstraction Layer](#5-platform-abstraction-layer)

---

## 1. Knowledge Source System

### Concept

Knowledge Source is a new skill type. Instead of providing instructions + tools like a regular skill, it represents a structured connection to an external knowledge base (wiki, documentation site, etc.).

### Skill Definition Format

```toml
[skill]
name = "arch-wiki"
type = "knowledge-source"
description = "ArchLinux official wiki"
version = "0.3.0"

[source]
engine = "mediawiki"
base_url = "https://wiki.archlinux.org"
language = "en"

[routing]
triggers = ["arch", "archlinux", "pacman", "makepkg", "AUR", "PKGBUILD"]
priority = 10
```

### Engine Adapter Hierarchy

```
KnowledgeSource trait
├── MediaWikiAdapter    ← ArchWiki, Gentoo Wiki, Wikipedia
├── MoinMoinAdapter     ← Debian Wiki, Fedora Wiki
└── WebAdapter          ← Generic fallback, uses web_fetch + smart extraction
```

### Auto-Registered Tools

Each knowledge source automatically registers two tools:

- `{name}.search(query: String, limit?: u32)` — Returns `Vec<{title, snippet, page_id, url}>`
- `{name}.read(page_id: String, section?: String)` — Returns clean Markdown content, optional section-level extraction

### MediaWiki Adapter

- **Search:** `/api.php?action=query&list=search&srsearch=...&format=json`
- **Read:** `/api.php?action=parse&pageid=...&prop=wikitext&format=json`
- **Section read:** `&section=N` parameter for targeted extraction
- **Content conversion:** wikitext → Markdown (reuse/extend existing html2text logic)

### MoinMoin Adapter

- **Search:** `/wiki/?action=fullsearch&value=...` (parse HTML search results using `scraper` crate, same as web_fetch enhancement)
- **Read:** `/wiki/PageName?action=raw` (raw wiki text)
- **Degradation:** Falls back to WebAdapter if MoinMoin API returns errors or is unavailable

### Web Adapter (Fallback)

- **Search:** Calls `web_search` with `site:{base_url} {query}`
- **Read:** Calls `web_fetch` with enhanced HTML → Markdown conversion

### Routing

No automatic routing. Knowledge sources register as normal tools. The LLM decides which to call based on system prompt hints:

```
Available knowledge sources: arch-wiki (ArchLinux), debian-wiki (Debian), ...
```

### web_fetch Enhancement (Accompanying Improvement)

- HTML → Markdown conversion replaces plain text (preserves code blocks, tables, heading hierarchy)
- New optional parameter `selector: Option<String>` for CSS selector extraction
- Output truncation respects paragraph boundaries

### KnowledgeSource Trait

```rust
#[async_trait]
pub trait KnowledgeSource: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn engine(&self) -> &str;
    fn triggers(&self) -> &[String];

    async fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchResult>>;
    async fn read(&self, page_id: &str, section: Option<&str>) -> Result<PageContent>;
}

pub struct SearchResult {
    pub title: String,
    pub snippet: String,
    pub page_id: String,
    pub url: String,
}

pub struct PageContent {
    pub title: String,
    pub content: String,       // Markdown formatted
    pub sections: Vec<String>, // Available section names
    pub url: String,
}
```

---

## 2. Skill Version System & Builtin Protection

### Compile-Time Embedding

System skills are compiled into the binary via `include_str!()`:

```rust
const BUILTIN_SKILLS: &[(&str, &str)] = &[
    ("database-admin", include_str!("../../skills/database-admin/SKILL.md")),
    ("server-security", include_str!("../../skills/server-security/SKILL.md")),
    ("container-orchestration", include_str!("../../skills/container-orchestration/SKILL.md")),
];
```

### Version Synchronization

System skills do not have independent version fields. The nano-assistant binary version is injected at load time:

```rust
const NA_VERSION: &str = env!("CARGO_PKG_VERSION");

// When loading system skills:
skill.version = NA_VERSION.to_string();
skill.is_builtin = true;
```

The `version` field in SKILL.md is ignored for system skills; the binary version always takes precedence.

### Builtin Protection

**Loading order:**

1. Compiled builtin skills (`is_builtin = true`)
2. User skill directory `~/.config/nano-assistant/skills/`
3. skills.sh directory `~/.agents/skills/`
4. `config.extra_paths[]`

**Conflict resolution:**

- If a user skill has the same name as a system skill → **skip user skill, print warning**
- `[WARN] Skill 'database-admin' is builtin, user version ignored`
- System skills cannot be uninstalled, overridden, or modified

### SkillMetadata Changes

```rust
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub version: String,
    pub is_builtin: bool,        // NEW
    pub source: SkillSource,     // NEW
    // ...existing fields
}

pub enum SkillSource {
    Builtin,
    UserDir(PathBuf),
    SkillsSh,
    ExtraPath(PathBuf),
}
```

### CLI Output

`na skills list` example:

```
NAME                      VERSION  SOURCE
database-admin            0.3.0    builtin
server-security           0.3.0    builtin
container-orchestration   0.3.0    builtin
arch-wiki                 0.3.0    builtin
my-custom-skill           1.2.0    ~/.config/nano-assistant/skills/
some-community-skill      0.5.0    ~/.agents/skills/
```

---

## 3. Self-Management

Three subsystems: skill installation, MCP configuration, memory editing.

### 3a. Skill Self-Installation

nana uses the existing shell tool to call skills.sh CLI:

```bash
# Search
npx skills search "kubernetes"

# Install
npx skills add <package-name> -g
```

**Post-install hot reload:**

```rust
impl Agent {
    /// Rescan skill directories, load newly installed skills
    pub async fn rescan_skills(&mut self) -> Result<Vec<String>>
}
```

**Trigger mechanism:** After shell tool executes a command, check if the command matches the pattern `r"npx\s+skills\s+(add|install)\b"` or `r"skills\s+(add|install)\b"`. If matched and exit code is 0, agent loop automatically calls `rescan_skills`. No new tool needed.

**Builtin protection applies during rescan** — newly installed skills with the same name as builtin skills are skipped with a warning.

**System prompt injection:**

```
## Self-Management
You can install community skills:
1. Search: execute `npx skills search "<keyword>"`
2. Install: execute `npx skills add <package> -g`
3. Skills auto-reload after installation.
Do NOT modify builtin skills.
```

### 3b. MCP Self-Configuration

nana uses existing `file_read` + `file_edit` tools to modify `config.toml`.

**Hot reload mechanism:**

```rust
impl Agent {
    /// Re-read MCP config, connect to new servers
    pub async fn reload_mcp(&mut self) -> Result<ReloadResult>
}

pub enum ReloadResult {
    Success { new_servers: Vec<String> },
    PartialFailure { connected: Vec<String>, failed: Vec<(String, String)> },
}
```

**Trigger:** Agent loop detects when `file_edit` targets a path ending in `config.toml` and the `old_string` or `new_string` contains `[[mcp.servers]]` or `[mcp` → automatically attempts `reload_mcp`.

**Failure degradation:**

- Hot reload succeeds → inform user that new MCP server is connected
- Hot reload fails (connection timeout, etc.) → preserve config change, prompt user to restart na

**System prompt injection:**

```
## MCP Self-Configuration
You can add MCP servers by editing ~/.config/nano-assistant/config.toml.
Add a [[mcp.servers]] section. Config auto-reloads after edit.
If reload fails, suggest user restart na.
```

### 3c. MEMORY.md Self-Modification

nana already has `file_read` and `file_edit` tools. No new mechanism needed.

**Constraints:**

- MEMORY.md path is explicitly provided in system prompt
- System prompt guides content organization (format conventions)
- nana can only modify its own MEMORY.md, not system skills or other protected files

**System prompt injection:**

```
## Memory Management
Your memory file: ~/.config/nano-assistant/MEMORY.md
You can read and edit this file to persist information across sessions.
Format: Markdown with ## headers as categories.
```

### Shared Design Principles (3a/3b/3c)

- **No new tools** — reuse shell, file_read, file_edit
- **Auto-detect + hot reload** — agent loop intelligently detects changes after tool execution
- **System prompt driven** — LLM learns its capabilities and boundaries through prompt injection
- **Clear protection boundaries** — system skills immutable, config mutable, memory mutable

---

## 4. Interactive Command Control

### Two-Layer Strategy

```
Command execution request
    ↓
Non-interactive mode available? ──yes──→ Direct execution (existing shell tool)
    ↓ no
PTY mode execution
```

### 4a. Non-Interactive Priority

System prompt guides the LLM to prefer non-interactive flags:

```
## Command Execution
Always prefer non-interactive command flags:
- apt install -y, yum install -y
- yes | command
- echo "selection" | command
- command --batch, --non-interactive, --yes, --no-confirm
Only use PTY mode when no non-interactive option exists.
```

No code changes needed — pure LLM behavior guidance.

### 4b. PTY Mode

New `PtyShell` tool, parallel to existing `Shell`:

```rust
pub struct PtyShell {
    platform: Arc<dyn PlatformPty>,
}
```

**Tool definition:**

```json
{
    "name": "pty_shell",
    "description": "Execute interactive commands via pseudo-terminal",
    "parameters": {
        "command": "string - command to execute",
        "interactions": [{
            "expect": "string - regex pattern to match in output",
            "respond": "string - text to send (or __USER_INPUT__ for password passthrough)",
            "timeout_secs": "number - max wait time, default 30"
        }],
        "timeout_secs": "number - overall timeout, default 120"
    }
}
```

**Example — menu selection:**

```json
{
    "command": "nmtui",
    "interactions": [
        {"expect": "Activate a connection", "respond": "\n"},
        {"expect": "eth0", "respond": "\n"}
    ]
}
```

**Example — sudo password:**

```json
{
    "command": "sudo apt update",
    "interactions": [
        {"expect": "[Pp]assword", "respond": "__USER_INPUT__"}
    ]
}
```

### 4c. Password PTY Passthrough

When `respond` is `__USER_INPUT__`, special handling:

1. PTY detects expect pattern match (e.g., "Password:")
2. Pause LLM stream, stop recording PTY output
3. Connect PTY stdin/stdout directly to user terminal
4. User types password directly in terminal
5. Password sent to PTY — never passes through LLM, never enters conversation history
6. Detect password prompt completion (next line of output), resume normal mode

**Security constraints:**

- Content marked `__USER_INPUT__` is NEVER written to ConversationHistory
- Not recorded in memory
- Not sent to LLM API
- Replaced with `[REDACTED]` in logs

### 4d. sudo NOPASSWD Suggestion

System prompt guidance for frequent sudo scenarios:

```
If user frequently needs sudo, you may suggest configuring
passwordless sudo for specific commands via /etc/sudoers.d/
Only suggest, never auto-configure.
```

### 4e. PTY Platform Abstraction

```rust
#[async_trait]
trait PlatformPty: Send + Sync {
    async fn spawn(&self, command: &str) -> Result<Box<dyn PtyProcess>>;
}

#[async_trait]
trait PtyProcess: Send {
    async fn read(&mut self) -> Result<String>;
    async fn write(&mut self, data: &str) -> Result<()>;
    async fn passthrough_stdin(&mut self) -> Result<()>;
    fn exit_status(&self) -> Option<i32>;
}

// Linux implementation
struct UnixPty;  // openpty + tokio::process

// Windows placeholder
// struct ConPty;  // Windows ConPTY API
```

Linux uses `nix::pty::openpty` or `rustix`. Windows ConPTY interface reserved, not implemented.

---

## 5. Platform Abstraction Layer

### Goal

All new features must not hardcode Linux assumptions. Platform differences are isolated via trait abstraction.

### Platform Trait

```rust
pub trait Platform: Send + Sync {
    // Filesystem
    fn config_dir(&self) -> PathBuf;         // Linux: ~/.config/nano-assistant
                                              // Windows: %APPDATA%/nano-assistant
    fn skills_dir(&self) -> PathBuf;          // Linux: ~/.config/nano-assistant/skills
    fn memory_path(&self) -> PathBuf;         // Linux: ~/.config/nano-assistant/MEMORY.md

    // Shell execution
    fn shell_command(&self) -> (&str, &str);  // Linux: ("sh", "-c")
                                               // Windows: ("cmd", "/c")

    // PTY
    fn create_pty(&self) -> Box<dyn PlatformPty>;  // Linux: UnixPty
                                                     // Windows: ConPty

    // System info
    fn detect_system_info(&self) -> SystemInfo;

    // Capabilities
    fn supports_pty_passthrough(&self) -> bool;
}
```

### Conditional Compilation

```rust
#[cfg(unix)]
mod unix;

#[cfg(windows)]
mod windows;

pub fn current_platform() -> Arc<dyn Platform> {
    #[cfg(unix)]
    { Arc::new(unix::UnixPlatform) }

    #[cfg(windows)]
    { Arc::new(windows::WindowsPlatform) }
}
```

### Impact Scope

| Existing Module | Current Linux Assumption | Required Abstraction |
|----------------|------------------------|---------------------|
| `system_info.rs` | `/etc/os-release`, `systemctl` | `Platform::detect_system_info()` |
| `tools/shell.rs` | `sh -c` | `Platform::shell_command()` |
| `config/schema.rs` | `~/.config/` hardcoded | `Platform::config_dir()` |
| `skills/mod.rs` | `~/.agents/skills/` path | `Platform::skills_dir()` |
| `memory/markdown.rs` | Path separators | `Platform::memory_path()` |
| New `pty_shell.rs` | `openpty` | `Platform::create_pty()` |

### Implementation Scope

**This version implements:**

- Define `Platform` trait with all method signatures
- Implement `UnixPlatform` with full functionality
- Refactor existing code to use `Platform` trait instead of hardcoded paths
- `WindowsPlatform` as stub: `unimplemented!("Windows support planned")`

**Not implemented:**

- Windows-specific logic
- macOS special handling (macOS uses Unix branch, generally compatible)

---

## Cross-Cutting Concerns

### Dependency Additions

| Crate | Purpose | Estimated Size Impact |
|-------|---------|----------------------|
| `nix` or `rustix` | PTY operations (openpty) | ~50KB |
| (optional) `cssselect` or `scraper` | CSS selector for web_fetch | ~100KB |

### System Prompt Budget

New prompt injections add approximately:

- Knowledge source list: ~100 tokens
- Self-management instructions: ~150 tokens
- Command execution guidance: ~100 tokens
- **Total: ~350 tokens additional system prompt**

### Security Considerations

- Password passthrough never touches LLM API or conversation history
- System skills are immutable (compiled into binary)
- Skill installation goes through existing security mode (confirm/whitelist)
- MCP config changes go through file_edit (subject to security mode)
- PTY mode inherits the current security mode for command approval

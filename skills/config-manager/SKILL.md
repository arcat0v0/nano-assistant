---
name: config-manager
description: Modify nano-assistant's own config.toml — switch LLM provider/model, rotate API keys, change security mode, tune behavior flags, add/remove MCP servers and skill paths. Use when user asks to change provider, switch model, update key, adjust safety, or edit nano-assistant settings.
version: 0.1.0
author: nano-assistant
tags: [config, provider, model, security, mcp, self-modify]
---

# nano-assistant Config Manager

Edit `nano-assistant` own config file safely. File uses TOML.

## 1. Locate Config

Order of precedence: CLI `--config-path` > default.

Default path:

- Linux/macOS: `~/.config/nano-assistant/config.toml`
- Windows: `%APPDATA%\nano-assistant\config.toml`

Check existence first:

```bash
test -f ~/.config/nano-assistant/config.toml && echo ok || echo missing
```

Missing? Create dir + write minimal config (see Section 7).

## 2. Schema (authoritative field names)

Matches `src/config/schema.rs`. Unknown keys silently ignored — typos = silent drift. Validate names against this list.

```toml
[provider]
provider      = "openai" | "anthropic" | "gemini" | "glm" | "ollama"
model         = "gpt-4o-mini"      # provider-specific
api_key       = "sk-..."           # optional; env NA_API_KEY wins
api_url       = ""                 # optional; set for OpenAI-compat (DeepSeek/Kimi/Qwen)
timeout_secs  = 120
temperature   = 0.7

[memory]
enabled            = true
max_messages       = 100
embeddings_enabled = false

[security]
autonomy_level = "standard"
mode           = "direct" | "confirm" | "whitelist"
whitelist      = ["ls", "cat", "docker *"]
allowed_tools  = []
blocked_tools  = []

[behavior]
max_iterations  = 10
debug           = false
verbose_errors  = true
explain_tools   = true
streaming       = true

[skills]
enabled       = true
allow_scripts = false
skills_dir    = ""           # optional override
extra_paths   = []

[mcp]
enabled          = true
deferred_loading = true

[[mcp.servers]]
name      = "context7"
transport = "stdio"          # "stdio" | "http" | "sse"
command   = "npx"
args      = ["-y", "@upstash/context7-mcp@latest"]
env       = { KEY = "val" }
# for http/sse:
# url     = "https://..."
# headers = { Authorization = "Bearer ..." }
# tool_timeout_secs = 30
```

## 3. Workflow — ALWAYS

1. **Read** current file with `file_read` before changing. Never blind-write.
2. **Backup** once per session: `cp config.toml config.toml.bak.$(date +%s)`.
3. **Edit** via `file_edit` (targeted replace) — not `file_write` full overwrite, unless creating from scratch.
4. **Validate** TOML parses: `na --config-path <path> --help` exits 0, or `python3 -c "import tomllib,sys; tomllib.load(open(sys.argv[1],'rb'))" config.toml`.
5. **Confirm** change to user with diff summary. Do not silently rotate secrets.

## 4. Common Tasks

### Switch provider (e.g. OpenAI → Anthropic)

Edit `[provider]` block. Update **all three**: `provider`, `model`, `api_key` (or instruct user to set env). Clear stale `api_url` if switching away from ollama-compat.

```toml
[provider]
provider = "anthropic"
model    = "claude-sonnet-4-5"
api_key  = "sk-ant-..."
api_url  = ""                 # clear
```

### Switch to OpenAI-compatible (DeepSeek / Kimi / Qwen)

Provider stays `ollama` (acts as compat base), set `api_url`:

```toml
[provider]
provider = "ollama"
api_url  = "https://api.deepseek.com/v1"
model    = "deepseek-chat"
api_key  = "sk-..."
```

### Rotate API key

Prefer env var over file when possible — safer:

```bash
export NA_API_KEY="sk-new..."
```

If user insists on file: edit `api_key` in `[provider]`. Warn if file world-readable: `chmod 600 config.toml`.

### Change security mode

```toml
[security]
mode = "confirm"             # or "direct" | "whitelist"
whitelist = ["ls", "git *", "docker ps"]   # only when mode = "whitelist"
```

Whitelist pattern: literal or `*`-suffix glob. Each list entry matched against full command line.

### Add MCP server

Append `[[mcp.servers]]` table. Don't duplicate `name`.

```toml
[[mcp.servers]]
name    = "exa"
command = "npx"
args    = ["-y", "exa-mcp-server"]
env     = { EXA_API_KEY = "..." }
```

### Remove MCP server

Delete the matching `[[mcp.servers]]` block entirely. Leaving empty name breaks load.

### Toggle streaming / debug / explain_tools

Flip bool in `[behavior]`.

## 5. Safety Rules

- **Never** commit `config.toml` to git; it holds secrets. If cwd is repo root, verify `.gitignore` covers `config.toml`.
- **Never** log or echo `api_key` value back to user. Refer to it as `sk-...` (first 4 chars + `...`).
- **Never** change both provider and model without knowing the provider's valid model list. Ask user if unsure.
- **Warn** user when switching `mode` from `confirm`/`whitelist` to `direct` — it removes all per-call approval.
- **Warn** when `skills.allow_scripts = true` — enables executable skill hooks.

## 6. Failure Recovery

TOML parse error after edit? Restore from `.bak` file made in step 2. Run `na --help` to verify config loads.

## 7. Bootstrap — no config exists

```bash
mkdir -p ~/.config/nano-assistant
cat > ~/.config/nano-assistant/config.toml <<'EOF'
[provider]
provider = "openai"
model    = "gpt-4o-mini"
api_key  = ""

[security]
mode = "confirm"
EOF
chmod 600 ~/.config/nano-assistant/config.toml
```

Then tell user to set `api_key` or `export NA_API_KEY=...`.

## 8. Quick Inspect (no edit)

```bash
na -v "noop"            # prints provider/model/mode/debug summary to stderr
na --debug "noop"       # fuller runtime context
```

Or read directly with `file_read` on config path.

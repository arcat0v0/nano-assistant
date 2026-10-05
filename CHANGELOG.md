# Changelog

## Unreleased

### Added

- Local model profiles, `na model` management commands, one-shot model overrides, and interactive `/model` switching with optional saved default. Switching retains conversation and tool state without inheriting credentials from another provider.
- Built-in DeepSeek, Kimi, GLM, MiMo, and Qwen providers with authenticated live model discovery; `na model discover` and the interactive `/model add` wizard select online models and save local profiles without storing API keys. The wizard is available even when the previous default model cannot initialize.
- Interactive `/` command palette with arrow-key selection and Right-arrow completion, plus a `/model` submenu for profile switching, provider addition, and optional default selection.
- `na chat --mode auto` reviews every operation except the actual built-in file reader. Valid safe decisions auto-approve; failed or timed-out reviews require fresh human confirmation. Skills review their expanded execution plans, and PTY/MCP operations use the same gate.
- The system prompt now requires dated, append-only backups under ~/Backup before irreversible changes to user or production data.
- A session-fixed, isolated safety reviewer uses `security.review_profile` when explicitly configured, or the startup main model otherwise; it has no tools or shared conversation history.
- MIT and Apache-2.0 license texts, included in newly built release archives, with licensing and safety disclaimers in the README.
- Conversation sessions persist after every completed turn under the configuration directory; `/resume` in interactive mode lists past sessions (newest first, with model, timestamp, and message preview) and continues the selected one, while `/clear` starts a fresh session.

### Changed

- Rig 0.42 is now the sole model and Agent runtime: native structured tool calls/results, provider clients, streaming, and multi-turn execution replace the custom provider protocol, XML/native dispatcher, and turn loop.
- Builtin, skill, knowledge, and MCP tools now use Rig's tool registry; direct/confirm/whitelist policy is applied to all execution paths. MCP discovery and reload, skill rescans, Markdown memory, CLI and TUI remain available.
- Hub `free/*` models send the unprefixed model slug through the existing signed JSON transport; native tools and results continue across both streamed and nonstreamed Hub responses.
- Source builds require Rust 1.88 or newer; the musl vendored OpenSSL release configuration remains in place.
- GLM chat now uses Z.AI's documented Bearer API key rather than JWT. If its default model-list endpoint is unavailable, discovery reads current text-model IDs from Z.AI's published pricing catalog, which does not guarantee account access.
- Streamed tool calls and results again show progress on stderr; confirmation prompts name the tool and arguments instead of displaying an unknown command for non-shell tools.
- The Linux installer no longer creates configuration directories or embeds a default configuration; normal `na` startup creates missing configuration with explicit DeepSeek defaults and leaves existing files untouched. The redundant `--config` editor entry point is removed; `--help` and `--version` remain read-only.
- Chat startup now stops on unreadable or malformed existing configuration, invalid selected security modes, and unusable explicitly configured safety models instead of silently falling back to direct execution.
- Auto safety review is now the default for new and unspecified security configuration; explicitly configured modes and CLI overrides remain effective.
- Default Auto preserves first-run DeepSeek and model-add onboarding when no usable main-model credential exists. Its implicit reviewer initializes from the first successfully configured main model before chat execution, without requiring a separate review profile.
- Auto review now returns risky/unknown feedback to the main model for revised proposals, carrying up to two previous rejected actions and reasons into review. Only three consecutive non-approvals prompt for human confirmation; approval, a new user request, or human intervention resets the dialogue.
- Auto safety review retries malformed reviewer replies up to twice before involving the user, returning the rejection reason and a snippet of the invalid reply so the reviewer can correct its output. Human confirmation is now required only after three consecutive unusable responses; request failures and timeouts still confirm immediately.
- Auto safety review now scores actions on a 1-100 risk scale: 1-9 auto-approves, 10-49 returns to the main model for revision, 50-69 auto-approves only when the latest user message explicitly confirms the destructive scope, 70-89 always requires live human confirmation, and 90-100 is prohibited outright, even with human approval. Commands that modify or delete existing files under ~/Backup are always prohibited.
- Review context sections are wrapped in per-request random boundary tokens so forged confirmations embedded in tool arguments or prior rejections cannot impersonate the genuine user_request section.
- Tool calls render as bordered blocks (⚒ header, 🛡 review verdict, ✓/✗ result) with color-coded risk words, and confirmation prompts show a readable action summary behind a ⚠ [y/N] flag instead of a raw JSON dump.

### Fixed

- Tool-edit failures preserve actionable error feedback without modifying files; duplicate matches still require a unique replacement.
- Conversation history is trimmed by complete user turns so a retained tool result keeps its matching call.
- Dynamic skill, knowledge, and MCP tool names now meet provider function-name rules without losing their original backend routing, preventing DeepSeek from rejecting requests containing dotted tool names.
- The installer accepts release archives containing optional license files while rejecting unexpected paths and duplicate entries before replacing an existing installation.
- The system prompt now states the reply-language rule as an absolute requirement and repeats it as the closing section, so Chinese requests no longer draw English final answers from smaller models after English-heavy tool output.

## v0.3.1 - 2026-04-13

### Added

- Stronger runtime self-management guidance in the system prompt for execution flow, durable memory writes, and concise final reporting
- Richer system bootstrap context including CPU model, GPU, virtualization, and detected tool paths
- A regression test that blocks Unix path handling from falling back to a literal `~/` directory

### Changed

- Unix config and skills path resolution now falls back to the real user home discovery path before any temporary fallback
- Local workspace artifacts such as `.sisyphus/`, `AGENTS.md`, `nvim.log`, and accidental literal `~/` directories are now ignored by git
- `HOME`-dependent tests now restore environment state and run serially where needed

### Fixed

- `~/.config/nano-assistant` is now resolved as the executing user's home config directory instead of creating a literal `~/` tree inside the repository
- Full `cargo test` runs are stable again without cross-test `HOME` pollution

## v0.3.0 - 2026-04-13

### Added

- Knowledge source support for builtin wiki/documentation skills
- Builtin `arch-wiki`, `debian-wiki`, and `redhat-wiki` knowledge sources
- Skill self-management guidance, including skill install, MCP config edits, and `MEMORY.md` management
- Automatic skill rescan after `skills add/install`
- Automatic MCP reload after `config.toml` MCP edits
- `pty_shell` interactive command tool for expect/respond flows
- Windows platform path and shell support
- Windows pipe-backed interactive command support for prompt/response flows

### Changed

- Builtin skill versions now follow the binary version
- Builtin skills are protected from filesystem override collisions
- README now documents Windows installation and usage constraints
- System prompt now explains Windows interactive command limitations

### Fixed

- `file_edit` MCP auto-reload hook now reads the correct `path` argument
- `file_edit` now refuses to modify builtin skill source files

### Notes

- Windows interactive command support is currently pipe-based, not native ConPTY
- Full-screen terminal UIs on Windows may still behave incompletely

# Repository Guidelines

## Project Overview

`nano-assistant` (binary `na`) — terminal-based Rust LLM agent. One-shot mode (`na "prompt"`) and interactive REPL (`na` or `na chat`). Targets Linux/macOS; Windows has partial support (stdin/stdout pipe shell, no native ConPTY).

## Project Structure & Module Organization

Core Rust code lives in `src/`. Entry: `src/bin/na.rs` → `src/cli/` parses CLI → constructs the Rig-backed agent session.

Core module map (`src/`):

- `agent/` — thin session orchestration around Rig 0.42, which owns provider message handling, native tool calls/results, streaming, and multi-turn execution. Application code builds prompts, retains complete-turn history/Markdown memory, registers dynamic tools, and applies `max_iterations`.
- `providers/` — Rig model clients for OpenAI, Anthropic, Gemini, GLM (JWT auth via HMAC+SHA2), Ollama, and compatible APIs (DeepSeek/Kimi/Qwen via custom `api_url`); Hub requests use the existing signed transport.
- `tools/` — 9 built-in tools implementing Rig's native `Tool` API: `shell`, `pty_shell`, `file_read`, `file_write`, `file_edit`, `glob_search`, `content_search`, `web_fetch`, `web_search`. Skills, knowledge sources, and MCP tools use Rig `DynamicTool` registrations.
- `security/` — Rig tool-call hook applies 3 modes before tool execution: `direct` (run), `confirm` (prompt per call), `whitelist` (raw command pattern match with `*` globbing).
- `platform/` — OS abstraction behind a `Platform` trait. Unix uses `nix`/`libc` for PTY; Windows uses piped `cmd /C`. Add per-OS divergence here.
- `mcp/` — MCP (Model Context Protocol) client. Three transports: stdio, http, sse. Supports `deferred_loading`: instead of registering all MCP tools upfront, a synthetic `tool_search` tool activates them on demand to keep prompts small.
- `memory/` — Markdown-backed persistent conversation memory (`MEMORY.md` in config dir). Capped by `max_messages`.
- `skills/` — skill loader. Scans (in order, first-wins): `~/.config/nano-assistant/skills/`, `~/.agents/skills/` (skills.sh ecosystem), then `skills.extra_paths` from config. Built-in skills live in the repo `skills/` dir.
- `knowledge/` — knowledge source adapters (URL-encoded queries to external docs).
- `cli/`, `tui/`, `config/` — CLI parsing, terminal rendering (rustyline + termimad + crossterm), config loading.

Integration tests live in `tests/integration.rs`; actual CLI/provider/tool continuation contracts live in `tests/runtime_cli.rs`; unit tests are co-located with modules. Distribution integration tests in `tests/test_distribution.py` exercise the Bash installer, publishing client, and temporary Git mirrors through controlled local services. Runtime ownership is documented in `docs/runtime.md`, with release notes and design docs under `docs/releases/` and `docs/superpowers/`. Built-in skill content is stored in `skills/`.

## Build, Test, and Development Commands

- `cargo build` — compile the project in debug mode.
- `cargo build --release` — build the production binary at `target/release/na`.
- `cargo run -- "list current directory"` — run the CLI with a one-shot prompt.
- `cargo run -- chat` — start interactive mode.
- `cargo test` — run the full test suite.
- `cargo test pty_shell -- --nocapture` — run a focused test subset while debugging interactive command support.
- `cargo fmt` — format (required before committing).
- `cargo clippy` — lint.
- `python3 -m unittest discover -s tests -p 'test_distribution.py' -v` — test installation and publishing with isolated local services; requires Python 3.11+, Bash, jq, curl/wget, and coreutils.
- `shellcheck install.sh` — lint the installer.

Run commands from the repository root where `Cargo.toml` is located.

## Coding Style & Naming Conventions

Rust 2021, MSRV 1.88. 4-space indentation, `snake_case` for functions/modules, `CamelCase` for types, and focused modules with one clear responsibility.

Extension seams are Rig `Tool`/`DynamicTool` for tools and model clients/transports for providers, plus application `Platform` and `Memory`. Add capabilities through these boundaries, not a second provider or agent loop.

Format before submitting with `cargo fmt`. Check for obvious issues with `cargo clippy` when practical. Keep comments short and only where behavior is non-obvious.

`--debug` writes structured diagnostics to **stderr only**, never stdout (preserves pipe-ability of assistant output).

## Testing Guidelines

Use Rust's built-in test framework with `#[test]` and `#[tokio::test]`. Add unit tests close to the implementation, cross-module behavior in `tests/integration.rs`, and real CLI tool/model continuation in `tests/runtime_cli.rs`. Name tests descriptively, for example `handles_interaction` or `cli_streamed_file_edit_reports_tool_progress_only_on_stderr`.

New features should include success-path coverage and at least one failure or timeout case where relevant.

## Commit & Pull Request Guidelines

Recent history favors Conventional Commit-style subjects, often with gitmoji prefixes, for example `✨ feat(debug): add runtime diagnostics context` or `fix(platform): ...`. Keep the type scoped and specific.

PRs should explain the user-visible change, list verification steps (`cargo test`, targeted commands), and link related issues or design docs. Include terminal output or screenshots only when they clarify CLI/TUI behavior.

## Release

Two release paths are supported, both executed by `.github/workflows/release.yml`; do not create GitHub Releases by other means:

- Automated (preferred): manually trigger the **Release** workflow from the GitHub Actions page (`workflow_dispatch`) and choose `patch` / `minor` / `major`. It bumps `version` in `Cargo.toml`, syncs `Cargo.lock`, commits as `🔖 chore(release): bump version to vX.Y.Z`, pushes the tag, then builds and publishes the release.
- Manual: bump `version` in `Cargo.toml` and commit it yourself, then `git tag vX.Y.Z && git push origin vX.Y.Z`. The workflow aborts if the tag does not match the `Cargo.toml` version.

Both paths run application/distribution checks, build `x86_64-linux-gnu`, `x86_64-linux-musl`, and `aarch64-linux-musl` tarballs plus `.sha256` checksums, and publish the same bundle to GitHub and Gitee, including `install.sh` and a readiness manifest. Gitee mirror updates never force push. Configure the `GITEE_TOKEN` repository Actions Secret before publishing. The musl targets depend on the vendored `openssl` entry in `Cargo.toml`; do not remove it.

Re-run failed jobs to reuse existing artifacts. To republish an existing version, dispatch Release with its `tag` input; it recovers original GitHub assets without bumping or rebuilding. See `docs/releases/distribution.md`. After publishing, verify the README bootstrap command on clean mainland/overseas Linux environments; static/local tests do not establish real Gitee or regional download success.

## Security & Configuration

Do not commit API keys or local config.

Config precedence: **CLI args > env vars > config file**. Config file: `~/.config/nano-assistant/config.toml` (Unix), `%APPDATA%\nano-assistant\config.toml` (Windows). API key env: `NA_API_KEY` (generic), plus `OPENAI_API_KEY`/`ANTHROPIC_API_KEY`/`GEMINI_API_KEY`/`GLM_API_KEY` fallbacks.

Runtime context (cwd, git root, git branch) is auto-injected into the system prompt each session — handled in agent prompt-building, no user config needed.

Prefer non-interactive command flags before using `pty_shell`, and document any security-mode implications when changing tool behavior.

# Changelog

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

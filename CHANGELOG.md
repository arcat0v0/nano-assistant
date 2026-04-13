# Changelog

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

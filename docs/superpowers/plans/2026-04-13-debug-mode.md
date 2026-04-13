# DEBUG Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a full DEBUG mode for nano-assistant that emits high-signal runtime diagnostics to stderr without changing normal stdout responses.

**Architecture:** Add a boolean debug flag in config and CLI, resolve it once in the CLI layer, then thread that flag into the Agent so the main loop, LLM call path, and tool execution path can emit concise stderr diagnostics. Keep the implementation local instead of introducing a global logging stack.

**Tech Stack:** Rust, clap, serde, tokio, existing nano-assistant CLI/agent modules.

---

### Task 1: Add config and CLI surface

**Files:**
- Modify: `src/config/schema.rs`
- Modify: `src/cli/mod.rs`
- Test: existing CLI/config unit tests in `src/cli/commands.rs`

- [ ] Step 1: Add `behavior.debug` with default `false`
- [ ] Step 2: Add `--debug` to chat CLI command
- [ ] Step 3: Add resolution logic for CLI-over-config precedence
- [ ] Step 4: Add/update tests for parsing and precedence

### Task 2: Thread debug flag into runtime

**Files:**
- Modify: `src/cli/commands.rs`
- Modify: `src/agent/loop_.rs`

- [ ] Step 1: Extend agent construction to accept resolved debug flag
- [ ] Step 2: Store debug flag on `Agent`
- [ ] Step 3: Add small helper for concise stderr debug output

### Task 3: Emit debug diagnostics from the agent loop

**Files:**
- Modify: `src/agent/loop_.rs`
- Test: existing agent tests in `src/agent/loop_.rs`

- [ ] Step 1: Log turn startup summary
- [ ] Step 2: Log iteration boundaries and LLM request/response summaries
- [ ] Step 3: Log tool call arguments and tool execution results
- [ ] Step 4: Log abnormal exits and iteration exhaustion

### Task 4: Document and verify

**Files:**
- Modify: `README.md`

- [ ] Step 1: Document `--debug` and `behavior.debug`
- [ ] Step 2: Run targeted tests
- [ ] Step 3: Run formatting

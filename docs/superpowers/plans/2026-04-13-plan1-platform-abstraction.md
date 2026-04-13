# Platform Abstraction Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract all platform-specific assumptions (paths, shell, system detection) behind a `Platform` trait so new features don't hardcode Linux and Windows support can be added later.

**Architecture:** A `src/platform/` module defines a `Platform` trait. `UnixPlatform` implements it for Linux/macOS. `WindowsPlatform` is a compile-time stub. All existing code that uses hardcoded paths (`~/.config/nano-assistant/`, `sh -c`, etc.) is refactored to call `Platform` methods via a global accessor `current_platform()`.

**Tech Stack:** Rust, `cfg` conditional compilation, `dirs` crate (optional, for robust home dir), existing `std::env`

---

### Task 1: Create Platform trait and UnixPlatform

**Files:**
- Create: `src/platform/mod.rs`
- Create: `src/platform/unix.rs`
- Create: `src/platform/windows.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Add platform module declaration to lib.rs**

In `src/lib.rs`, add `pub mod platform;` after the existing module declarations:

```rust
pub mod platform;
```

- [ ] **Step 2: Create src/platform/mod.rs with Platform trait**

```rust
use std::path::PathBuf;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::UnixPlatform;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::WindowsPlatform;

/// Platform-specific behavior abstraction.
pub trait Platform: Send + Sync {
    /// Config directory: ~/.config/nano-assistant/ on Linux
    fn config_dir(&self) -> PathBuf;

    /// Default skills directory inside config dir
    fn skills_dir(&self) -> PathBuf {
        self.config_dir().join("skills")
    }

    /// Path to MEMORY.md
    fn memory_md_path(&self) -> PathBuf {
        self.config_dir().join("MEMORY.md")
    }

    /// Path to config.toml
    fn config_path(&self) -> PathBuf {
        self.config_dir().join("config.toml")
    }

    /// Memory storage directory
    fn memory_dir(&self) -> PathBuf {
        self.config_dir().join("memory")
    }

    /// skills.sh ecosystem directory: ~/.agents/skills/
    fn agents_skills_dir(&self) -> PathBuf;

    /// Shell command and flag for executing commands.
    /// Returns (program, flag) e.g. ("sh", "-c") or ("cmd", "/c").
    fn shell_command(&self) -> (&'static str, &'static str);

    /// Expand `~` to home directory in a path string.
    fn expand_tilde(&self, path: &str) -> PathBuf;
}

/// Get the platform implementation for the current OS.
pub fn current_platform() -> &'static dyn Platform {
    #[cfg(unix)]
    {
        static INSTANCE: UnixPlatform = UnixPlatform;
        &INSTANCE
    }
    #[cfg(windows)]
    {
        static INSTANCE: WindowsPlatform = WindowsPlatform;
        &INSTANCE
    }
}
```

- [ ] **Step 3: Create src/platform/unix.rs**

```rust
use super::Platform;
use std::path::PathBuf;

pub struct UnixPlatform;

impl UnixPlatform {
    fn home_dir(&self) -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

impl Platform for UnixPlatform {
    fn config_dir(&self) -> PathBuf {
        if let Some(home) = self.home_dir() {
            home.join(".config").join("nano-assistant")
        } else {
            PathBuf::from(".config/nano-assistant")
        }
    }

    fn agents_skills_dir(&self) -> PathBuf {
        if let Some(home) = self.home_dir() {
            home.join(".agents").join("skills")
        } else {
            PathBuf::from(".agents/skills")
        }
    }

    fn shell_command(&self) -> (&'static str, &'static str) {
        ("sh", "-c")
    }

    fn expand_tilde(&self, path: &str) -> PathBuf {
        if let Some(rest) = path.strip_prefix("~/") {
            if let Some(home) = self.home_dir() {
                return home.join(rest);
            }
        }
        PathBuf::from(path)
    }
}
```

- [ ] **Step 4: Create src/platform/windows.rs (stub)**

```rust
use super::Platform;
use std::path::PathBuf;

pub struct WindowsPlatform;

impl Platform for WindowsPlatform {
    fn config_dir(&self) -> PathBuf {
        unimplemented!("Windows support planned for a future release")
    }

    fn agents_skills_dir(&self) -> PathBuf {
        unimplemented!("Windows support planned for a future release")
    }

    fn shell_command(&self) -> (&'static str, &'static str) {
        unimplemented!("Windows support planned for a future release")
    }

    fn expand_tilde(&self, _path: &str) -> PathBuf {
        unimplemented!("Windows support planned for a future release")
    }
}
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check 2>&1 | head -20`
Expected: no errors (warnings OK)

- [ ] **Step 6: Commit**

```bash
git add src/platform/ src/lib.rs
git commit -m "feat(platform): add Platform trait with Unix impl and Windows stub"
```

---

### Task 2: Refactor config paths to use Platform

**Files:**
- Modify: `src/config/schema.rs`
- Modify: `src/cli/commands.rs`

- [ ] **Step 1: Refactor default_config_path in config/schema.rs**

Replace the current `default_config_path()` function:

```rust
pub fn default_config_path() -> PathBuf {
    crate::platform::current_platform().config_path()
}
```

- [ ] **Step 2: Refactor memory_md_path in cli/commands.rs**

Replace the current `memory_md_path()` function:

```rust
pub(crate) fn memory_md_path() -> PathBuf {
    crate::platform::current_platform().memory_md_path()
}
```

- [ ] **Step 3: Verify it compiles and tests pass**

Run: `cargo check && cargo test 2>&1 | tail -5`
Expected: compiles, existing tests pass

- [ ] **Step 4: Commit**

```bash
git add src/config/schema.rs src/cli/commands.rs
git commit -m "refactor(config): use Platform trait for config and memory paths"
```

---

### Task 3: Refactor shell tool to use Platform

**Files:**
- Modify: `src/tools/shell.rs`

- [ ] **Step 1: Replace hardcoded sh -c with Platform call**

In `src/tools/shell.rs`, find the `Command::new("sh").arg("-c")` block (around line 85-88) and replace with:

```rust
let (shell, flag) = crate::platform::current_platform().shell_command();
let output = tokio::process::Command::new(shell)
    .arg(flag)
    .arg(&command)
    .output()
    .await?;
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: no errors

- [ ] **Step 3: Commit**

```bash
git add src/tools/shell.rs
git commit -m "refactor(shell): use Platform trait for shell command selection"
```

---

### Task 4: Refactor skills paths to use Platform

**Files:**
- Modify: `src/skills/mod.rs`

- [ ] **Step 1: Replace skills_dir() function**

Replace the existing `skills_dir()` function:

```rust
fn skills_dir() -> PathBuf {
    crate::platform::current_platform().skills_dir()
}
```

- [ ] **Step 2: Replace agents_skills_dir() function**

Replace the existing `agents_skills_dir()` function:

```rust
fn agents_skills_dir() -> PathBuf {
    crate::platform::current_platform().agents_skills_dir()
}
```

- [ ] **Step 3: Replace expand_tilde() function**

Replace the existing `expand_tilde()` function:

```rust
fn expand_tilde(path: &str) -> PathBuf {
    crate::platform::current_platform().expand_tilde(path)
}
```

- [ ] **Step 4: Verify it compiles and tests pass**

Run: `cargo check && cargo test 2>&1 | tail -5`
Expected: compiles, tests pass

- [ ] **Step 5: Commit**

```bash
git add src/skills/mod.rs
git commit -m "refactor(skills): use Platform trait for directory paths"
```

---

### Task 5: Verify full build and run

**Files:** None (verification only)

- [ ] **Step 1: Full build**

Run: `cargo build 2>&1 | tail -5`
Expected: successful build

- [ ] **Step 2: Run tests**

Run: `cargo test 2>&1 | tail -10`
Expected: all tests pass

- [ ] **Step 3: Smoke test CLI**

Run: `cargo run -- --version`
Expected: prints version number

- [ ] **Step 4: Final commit if any fixups needed**

Only if previous steps revealed issues that needed fixing.

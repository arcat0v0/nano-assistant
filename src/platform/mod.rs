//! Platform abstraction layer.
//!
//! Centralises all OS-specific path and shell logic behind a single trait so
//! the rest of the codebase never hardcodes platform assumptions.

mod unix;
mod windows;

use std::path::PathBuf;

/// A spawned PTY process.
#[async_trait::async_trait]
pub trait PtyProcess: Send {
    /// Read available output from PTY. Empty string = EOF.
    async fn read(&mut self) -> Result<String, std::io::Error>;
    /// Write data to PTY stdin.
    async fn write(&mut self, data: &str) -> Result<(), std::io::Error>;
    /// Connect PTY directly to user's terminal for secure input (passwords).
    async fn passthrough_stdin(&mut self) -> Result<(), std::io::Error>;
    /// Wait for process with timeout. Returns exit code.
    async fn wait(&mut self, timeout: std::time::Duration) -> Result<Option<i32>, std::io::Error>;
}

/// Platform-specific operations: config paths, shell commands, path expansion.
pub trait Platform: Send + Sync {
    /// Root configuration directory (e.g. `~/.config/nano-assistant/`).
    fn config_dir(&self) -> PathBuf;

    /// Default skills directory (`config_dir/skills`).
    fn skills_dir(&self) -> PathBuf {
        self.config_dir().join("skills")
    }

    /// Path to `MEMORY.md` (`config_dir/MEMORY.md`).
    fn memory_md_path(&self) -> PathBuf {
        self.config_dir().join("MEMORY.md")
    }

    /// Path to `config.toml` (`config_dir/config.toml`).
    fn config_path(&self) -> PathBuf {
        self.config_dir().join("config.toml")
    }

    /// Conversation memory storage directory (`config_dir/memory`).
    fn memory_dir(&self) -> PathBuf {
        self.config_dir().join("memory")
    }

    /// skills.sh default install location (`~/.agents/skills/`).
    fn agents_skills_dir(&self) -> PathBuf;

    /// Shell executable and flag for running commands.
    /// Returns `("sh", "-c")` on Unix.
    fn shell_command(&self) -> (&'static str, &'static str);

    /// Expand a leading `~` in a path to the user's home directory.
    fn expand_tilde(&self, path: &str) -> PathBuf;

    /// Spawn an interactive command in a PTY.
    fn spawn_pty(&self, command: &str) -> Result<Box<dyn PtyProcess>, std::io::Error>;
}

/// Return the platform implementation for the current OS.
pub fn current_platform() -> &'static dyn Platform {
    #[cfg(unix)]
    {
        static INSTANCE: unix::UnixPlatform = unix::UnixPlatform;
        &INSTANCE
    }
    #[cfg(windows)]
    {
        static INSTANCE: windows::WindowsPlatform = windows::WindowsPlatform;
        &INSTANCE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_platform_returns_consistent_reference() {
        let a = current_platform();
        let b = current_platform();
        assert_eq!(
            a as *const dyn Platform as *const u8,
            b as *const dyn Platform as *const u8,
        );
    }

    #[test]
    fn config_path_ends_with_config_toml() {
        let p = current_platform();
        let path = p.config_path();
        assert!(path.ends_with("config.toml"));
    }

    #[test]
    fn memory_md_path_ends_with_memory_md() {
        let p = current_platform();
        let path = p.memory_md_path();
        assert!(path.ends_with("MEMORY.md"));
    }

    #[test]
    fn skills_dir_ends_with_skills() {
        let p = current_platform();
        let path = p.skills_dir();
        assert!(path.ends_with("skills"));
    }

    #[test]
    fn memory_dir_ends_with_memory() {
        let p = current_platform();
        let path = p.memory_dir();
        assert!(path.ends_with("memory"));
    }

    #[test]
    fn expand_tilde_absolute_path_unchanged() {
        let p = current_platform();
        let result = p.expand_tilde("/usr/local/bin");
        assert_eq!(result, PathBuf::from("/usr/local/bin"));
    }
}

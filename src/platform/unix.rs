//! Unix (Linux / macOS) platform implementation.

use std::path::PathBuf;

use super::Platform;

pub struct UnixPlatform;

impl UnixPlatform {
    fn home_dir(&self) -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

impl Platform for UnixPlatform {
    fn config_dir(&self) -> PathBuf {
        if let Some(home) = self.home_dir() {
            return home.join(".config").join("nano-assistant");
        }
        PathBuf::from("~/.config/nano-assistant")
    }

    fn agents_skills_dir(&self) -> PathBuf {
        if let Some(home) = self.home_dir() {
            return home.join(".agents").join("skills");
        }
        PathBuf::from("~/.agents/skills")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_contains_nano_assistant() {
        let p = UnixPlatform;
        let dir = p.config_dir();
        let s = dir.to_string_lossy();
        assert!(
            s.contains(".config/nano-assistant"),
            "unexpected config_dir: {s}"
        );
    }

    #[test]
    fn agents_skills_dir_contains_agents_skills() {
        let p = UnixPlatform;
        let dir = p.agents_skills_dir();
        let s = dir.to_string_lossy();
        assert!(
            s.contains(".agents/skills"),
            "unexpected agents_skills_dir: {s}"
        );
    }

    #[test]
    fn shell_command_is_sh() {
        let p = UnixPlatform;
        assert_eq!(p.shell_command(), ("sh", "-c"));
    }

    #[test]
    fn expand_tilde_with_home_prefix() {
        let p = UnixPlatform;
        let expanded = p.expand_tilde("~/Documents/test");
        let s = expanded.to_string_lossy();
        assert!(
            s.contains("Documents/test"),
            "tilde not expanded: {s}"
        );
        assert!(
            !s.starts_with('~'),
            "tilde should be resolved: {s}"
        );
    }

    #[test]
    fn expand_tilde_absolute_path_unchanged() {
        let p = UnixPlatform;
        let result = p.expand_tilde("/usr/local/bin");
        assert_eq!(result, PathBuf::from("/usr/local/bin"));
    }

    #[test]
    fn expand_tilde_relative_path_unchanged() {
        let p = UnixPlatform;
        let result = p.expand_tilde("relative/path");
        assert_eq!(result, PathBuf::from("relative/path"));
    }

    #[test]
    fn default_methods_derive_from_config_dir() {
        let p = UnixPlatform;
        let config = p.config_dir();
        assert_eq!(p.config_path(), config.join("config.toml"));
        assert_eq!(p.memory_md_path(), config.join("MEMORY.md"));
        assert_eq!(p.skills_dir(), config.join("skills"));
        assert_eq!(p.memory_dir(), config.join("memory"));
    }
}

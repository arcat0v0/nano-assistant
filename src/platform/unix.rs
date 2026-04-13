//! Unix (Linux / macOS) platform implementation.

use std::path::{Path, PathBuf};

use super::{Platform, PtyProcess};

pub struct UnixPlatform;

impl UnixPlatform {
    fn home_dir(&self) -> Option<PathBuf> {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .or_else(|| directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()))
    }

    fn expand_home_path(home: Option<&Path>, path: &str) -> PathBuf {
        if let Some(rest) = path.strip_prefix("~/") {
            if let Some(home) = home {
                return home.join(rest);
            }
        }
        PathBuf::from(path)
    }

    fn config_dir_from_home(home: Option<&Path>) -> PathBuf {
        home.map(|path| path.join(".config").join("nano-assistant"))
            .unwrap_or_else(|| std::env::temp_dir().join("nano-assistant"))
    }

    fn agents_skills_dir_from_home(home: Option<&Path>) -> PathBuf {
        home.map(|path| path.join(".agents").join("skills"))
            .unwrap_or_else(|| {
                std::env::temp_dir()
                    .join("nano-assistant")
                    .join(".agents")
                    .join("skills")
            })
    }
}

impl Platform for UnixPlatform {
    fn config_dir(&self) -> PathBuf {
        Self::config_dir_from_home(self.home_dir().as_deref())
    }

    fn agents_skills_dir(&self) -> PathBuf {
        Self::agents_skills_dir_from_home(self.home_dir().as_deref())
    }

    fn shell_command(&self) -> (&'static str, &'static str) {
        ("sh", "-c")
    }

    fn expand_tilde(&self, path: &str) -> PathBuf {
        Self::expand_home_path(self.home_dir().as_deref(), path)
    }

    fn spawn_pty(&self, command: &str) -> Result<Box<dyn PtyProcess>, std::io::Error> {
        use nix::pty::openpty;
        use std::os::fd::IntoRawFd;
        use std::os::unix::io::FromRawFd;
        use std::process::{Command, Stdio};

        let pty = openpty(None, None).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::Other, format!("openpty failed: {e}"))
        })?;

        let master_raw = pty.master.into_raw_fd();
        let slave_raw = pty.slave.into_raw_fd();

        // dup the slave fd for stdout and stderr so each Stdio owns its own fd
        let slave_out = unsafe { libc::dup(slave_raw) };
        if slave_out < 0 {
            unsafe {
                libc::close(slave_raw);
                libc::close(master_raw);
            }
            return Err(std::io::Error::last_os_error());
        }
        let slave_err = unsafe { libc::dup(slave_raw) };
        if slave_err < 0 {
            unsafe {
                libc::close(slave_raw);
                libc::close(slave_out);
                libc::close(master_raw);
            }
            return Err(std::io::Error::last_os_error());
        }

        // Each from_raw_fd takes ownership of its fd; spawn will close them after fork
        let child = unsafe {
            Command::new("sh")
                .arg("-c")
                .arg(command)
                .stdin(Stdio::from_raw_fd(slave_raw))
                .stdout(Stdio::from_raw_fd(slave_out))
                .stderr(Stdio::from_raw_fd(slave_err))
                .spawn()
        }?;

        // Wrap master fd as a File for I/O
        let master_file = unsafe { std::fs::File::from_raw_fd(master_raw) };

        Ok(Box::new(UnixPtyProcess {
            master: Some(master_file),
            child,
        }))
    }
}

/// PTY process backed by a Unix master fd and child process.
struct UnixPtyProcess {
    master: Option<std::fs::File>,
    child: std::process::Child,
}

#[async_trait::async_trait]
impl PtyProcess for UnixPtyProcess {
    async fn read(&mut self) -> Result<String, std::io::Error> {
        use std::io::Read;

        let mut master = self
            .master
            .as_ref()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "PTY closed"))?
            .try_clone()?;

        tokio::task::spawn_blocking(move || {
            let mut buf = [0u8; 4096];
            match master.read(&mut buf) {
                Ok(0) => Ok(String::new()),
                Ok(n) => Ok(String::from_utf8_lossy(&buf[..n]).to_string()),
                Err(e) => {
                    // EIO is normal when the child exits — treat as EOF
                    if e.raw_os_error() == Some(libc::EIO) {
                        Ok(String::new())
                    } else {
                        Err(e)
                    }
                }
            }
        })
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
    }

    async fn write(&mut self, data: &str) -> Result<(), std::io::Error> {
        use std::io::Write;

        let mut master = self
            .master
            .as_ref()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "PTY closed"))?
            .try_clone()?;
        let data = data.to_string();

        tokio::task::spawn_blocking(move || master.write_all(data.as_bytes()))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
    }

    async fn passthrough_stdin(&mut self) -> Result<(), std::io::Error> {
        use std::io::{BufRead, Write};

        let mut master = self
            .master
            .as_ref()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "PTY closed"))?
            .try_clone()?;

        // Read one line from the real terminal stdin (for password entry)
        tokio::task::spawn_blocking(move || {
            let stdin = std::io::stdin();
            let mut line = String::new();
            stdin.lock().read_line(&mut line)?;
            master.write_all(line.as_bytes())?;
            Ok(())
        })
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
    }

    async fn wait(&mut self, timeout: std::time::Duration) -> Result<Option<i32>, std::io::Error> {
        // Drop master so the child sees EOF on its stdin side
        let child_id = self.child.id();

        let result = tokio::time::timeout(timeout, async {
            // Poll the child in a blocking task
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(50));
            loop {
                interval.tick().await;
                match self.child.try_wait()? {
                    Some(status) => return Ok(status.code()),
                    None => continue,
                }
            }
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => {
                // Timeout — kill the child
                let _ = unsafe { libc::kill(child_id as i32, libc::SIGKILL) };
                let _ = self.child.wait(); // reap
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "PTY process timed out",
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

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
    #[serial(home_env)]
    fn expand_tilde_with_home_prefix() {
        let saved_home = std::env::var_os("HOME");
        if saved_home.is_none() {
            std::env::set_var("HOME", "/tmp");
        }
        let p = UnixPlatform;
        let expanded = p.expand_tilde("~/Documents/test");
        let s = expanded.to_string_lossy();
        assert!(s.contains("Documents/test"), "tilde not expanded: {s}");
        assert!(!s.starts_with('~'), "tilde should be resolved: {s}");

        match saved_home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }
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
    #[serial(home_env)]
    fn default_methods_derive_from_config_dir() {
        let p = UnixPlatform;
        let config = p.config_dir();
        assert_eq!(p.config_path(), config.join("config.toml"));
        assert_eq!(p.memory_md_path(), config.join("MEMORY.md"));
        assert_eq!(p.skills_dir(), config.join("skills"));
        assert_eq!(p.memory_dir(), config.join("memory"));
    }

    #[test]
    #[serial(home_env)]
    fn unix_paths_do_not_fall_back_to_literal_tilde() {
        let saved_home = std::env::var_os("HOME");
        std::env::remove_var("HOME");

        let p = UnixPlatform;
        let config_dir = p.config_dir();
        let skills_dir = p.agents_skills_dir();
        let expanded = p.expand_tilde("~/Documents/test");

        match saved_home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }

        for path in [config_dir, skills_dir, expanded] {
            assert!(
                !path.starts_with("~"),
                "path should not use a literal tilde fallback: {}",
                path.display()
            );
        }
    }
}

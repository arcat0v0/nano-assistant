//! Windows platform implementation.
#![cfg_attr(not(windows), allow(dead_code))]

use std::io;
use std::path::{Path, PathBuf};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

use super::Platform;

pub struct WindowsPlatform;

impl WindowsPlatform {
    fn home_dir(&self) -> Option<PathBuf> {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
    }

    fn appdata_dir(&self) -> Option<PathBuf> {
        std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
            self.home_dir()
                .map(|home| home.join("AppData").join("Roaming"))
        })
    }

    fn expand_home_path(home: Option<&Path>, path: &str) -> PathBuf {
        if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
            if let Some(home) = home {
                return home.join(rest);
            }
        }
        PathBuf::from(path)
    }
}

impl Platform for WindowsPlatform {
    fn config_dir(&self) -> PathBuf {
        self.appdata_dir()
            .unwrap_or_else(|| PathBuf::from(r"~\AppData\Roaming"))
            .join("nano-assistant")
    }

    fn agents_skills_dir(&self) -> PathBuf {
        self.home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(".agents")
            .join("skills")
    }

    fn shell_command(&self) -> (&'static str, &'static str) {
        ("cmd", "/C")
    }

    fn expand_tilde(&self, path: &str) -> PathBuf {
        Self::expand_home_path(self.home_dir().as_deref(), path)
    }

    fn spawn_pty(&self, command: &str) -> Result<Box<dyn super::PtyProcess>, std::io::Error> {
        Ok(Box::new(WindowsPipeProcess::spawn("cmd", "/C", command)?))
    }
}

/// Windows interactive process backed by stdin/stdout/stderr pipes.
///
/// This is not a real console PTY, but it supports prompt/response style
/// interaction which is enough for many installers and confirmations.
struct WindowsPipeProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
}

impl WindowsPipeProcess {
    fn spawn(shell: &str, flag: &str, command: &str) -> Result<Self, io::Error> {
        let mut child = Command::new(shell)
            .arg(flag)
            .arg(command)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        Ok(Self {
            stdin: child.stdin.take(),
            stdout: child.stdout.take(),
            stderr: child.stderr.take(),
            child,
        })
    }
}

#[async_trait::async_trait]
impl super::PtyProcess for WindowsPipeProcess {
    async fn read(&mut self) -> Result<String, io::Error> {
        loop {
            if self.stdout.is_none() && self.stderr.is_none() {
                return Ok(String::new());
            }

            let stdout_fut = async {
                if let Some(stdout) = self.stdout.as_mut() {
                    let mut buf = [0u8; 4096];
                    let n = stdout.read(&mut buf).await?;
                    Ok::<_, io::Error>((0u8, n, buf))
                } else {
                    futures::future::pending::<Result<(u8, usize, [u8; 4096]), io::Error>>().await
                }
            };

            let stderr_fut = async {
                if let Some(stderr) = self.stderr.as_mut() {
                    let mut buf = [0u8; 4096];
                    let n = stderr.read(&mut buf).await?;
                    Ok::<_, io::Error>((1u8, n, buf))
                } else {
                    futures::future::pending::<Result<(u8, usize, [u8; 4096]), io::Error>>().await
                }
            };

            let (stream, n, buf) = tokio::select! {
                res = stdout_fut => res?,
                res = stderr_fut => res?,
            };

            if n == 0 {
                if stream == 0 {
                    self.stdout = None;
                } else {
                    self.stderr = None;
                }
                continue;
            }

            return Ok(String::from_utf8_lossy(&buf[..n]).to_string());
        }
    }

    async fn write(&mut self, data: &str) -> Result<(), io::Error> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "stdin closed"))?;
        stdin.write_all(data.as_bytes()).await?;
        stdin.flush().await
    }

    async fn passthrough_stdin(&mut self) -> Result<(), io::Error> {
        use std::io::BufRead;

        let stdin_pipe = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "stdin closed"))?;

        let line = tokio::task::spawn_blocking(move || {
            let stdin = std::io::stdin();
            let mut line = String::new();
            stdin.lock().read_line(&mut line)?;
            Ok::<String, io::Error>(line)
        })
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))??;

        stdin_pipe.write_all(line.as_bytes()).await?;
        stdin_pipe.flush().await
    }

    async fn wait(&mut self, timeout: std::time::Duration) -> Result<Option<i32>, io::Error> {
        match tokio::time::timeout(timeout, self.child.wait()).await {
            Ok(status) => status.map(|s| s.code()),
            Err(_) => {
                let _ = self.child.kill().await;
                let _ = self.child.wait().await;
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "interactive process timed out",
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::PtyProcess;

    #[test]
    fn config_dir_uses_roaming_appdata_layout() {
        let p = WindowsPlatform;
        let dir = p.config_dir();
        let s = dir.to_string_lossy();
        assert!(
            s.contains("AppData") && s.contains("Roaming") && s.ends_with("nano-assistant"),
            "unexpected config_dir: {s}"
        );
    }

    #[test]
    fn agents_skills_dir_uses_home_agents_layout() {
        let p = WindowsPlatform;
        let dir = p.agents_skills_dir();
        let s = dir.to_string_lossy();
        assert!(
            s.ends_with(".agents/skills") || s.ends_with(".agents\\skills"),
            "unexpected agents_skills_dir: {s}"
        );
    }

    #[test]
    fn shell_command_uses_cmd() {
        let p = WindowsPlatform;
        assert_eq!(p.shell_command(), ("cmd", "/C"));
    }

    #[test]
    fn expand_tilde_resolves_home_prefix() {
        let expanded = WindowsPlatform::expand_home_path(
            Some(Path::new(r"C:\Users\tester")),
            "~/Documents/test",
        );
        let s = expanded.to_string_lossy();
        assert!(s.contains("Documents"));
        assert!(!s.starts_with('~'));
    }

    #[tokio::test]
    async fn pipe_process_supports_prompt_response() {
        let mut proc = WindowsPipeProcess::spawn(
            "sh",
            "-c",
            "printf 'Name: '; read name; printf 'Hello %s\\n' \"$name\"",
        )
        .unwrap();

        let first = proc.read().await.unwrap();
        assert!(first.contains("Name:"));

        proc.write("nano\n").await.unwrap();

        let second = proc.read().await.unwrap();
        assert!(second.contains("Hello nano"));

        let status = proc.wait(std::time::Duration::from_secs(1)).await.unwrap();
        assert_eq!(status, Some(0));
    }
}

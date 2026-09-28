#![cfg(unix)]

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nix::pty::{openpty, Winsize};

struct Terminal {
    child: Child,
    master: File,
    output: Vec<u8>,
}

impl Terminal {
    fn start(home: &Path, config: &Path) -> Self {
        let pty = openpty(
            Some(&Winsize {
                ws_row: 40,
                ws_col: 120,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
            None,
        )
        .unwrap();
        let slave = File::from(pty.slave);
        let stdin = slave.try_clone().unwrap();
        let stdout = slave.try_clone().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_na"));
        command
            .args(["chat", "--config-path", config.to_str().unwrap()])
            .current_dir(home)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("TERM", "xterm")
            .env("NA_LOCAL_KEY", "local-test-key")
            .env_remove("NA_API_KEY")
            .env_remove("NA_MODEL")
            .env_remove("NA_PROVIDER")
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(slave));
        let child = command.spawn().unwrap();
        let master = File::from(pty.master);
        let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(
            unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
        Self {
            child,
            master,
            output: Vec::new(),
        }
    }

    fn send(&mut self, input: &str) {
        self.master.write_all(input.as_bytes()).unwrap();
    }

    fn until(&mut self, expected: &str) {
        self.until_from(expected, 0);
    }

    fn until_from(&mut self, expected: &str, start: usize) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !String::from_utf8_lossy(&self.output[start..]).contains(expected) {
            let mut buffer = [0u8; 8192];
            match self.master.read(&mut buffer) {
                Ok(0) => panic!(
                    "terminal closed before {expected}: {}",
                    String::from_utf8_lossy(&self.output)
                ),
                Ok(count) => self.output.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for {expected}: {:?}",
                        String::from_utf8_lossy(&self.output)
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!(
                    "terminal read failed: {error}: {}",
                    String::from_utf8_lossy(&self.output)
                ),
            }
        }
    }

    fn finish(mut self) {
        self.send("/exit\r");
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "terminal failed: {}",
                    String::from_utf8_lossy(&self.output)
                );
                return;
            }
            if Instant::now() >= deadline {
                self.child.kill().unwrap();
                panic!(
                    "terminal did not exit: {}",
                    String::from_utf8_lossy(&self.output)
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn config(home: &Path) -> std::path::PathBuf {
    let path = home.join("assistant.toml");
    std::fs::write(
        &path,
        "[provider]\nprovider='openai'\nmodel='gpt-4o-mini'\n\n[models.profiles.work]\nprovider='deepseek'\nmodel='local-model'\napi_url='http://127.0.0.1:9'\napi_key_env='NA_LOCAL_KEY'\n\n[skills]\nenabled=false\n\n[memory]\nenabled=false\n\n[hub]\nenabled=false\n",
    )
    .unwrap();
    path
}

#[test]
fn slash_arrows_select_model_without_recalling_history_and_back_preserves_config() {
    let temp = tempfile::tempdir().unwrap();
    let path = config(temp.path());
    let before = std::fs::read_to_string(&path).unwrap();
    let mut terminal = Terminal::start(temp.path(), &path);
    terminal.until("❯ ");
    terminal.send("/\x1b[B");
    terminal.until("> /memory");
    let checkpoint = terminal.output.len();
    terminal.send("\x1b[A");
    terminal.until_from("> /model", checkpoint);
    terminal.send("\r");
    terminal.until("Current model:");
    terminal.until("Add provider");
    let checkpoint = terminal.output.len();
    terminal.send("\x1b[B\x1b[B\x1b[B\r");
    terminal.until_from("\x1b[K❯ ", checkpoint);
    terminal.finish();
    assert_eq!(std::fs::read_to_string(path).unwrap(), before);
}

#[test]
fn right_completes_m_to_model_then_add_choice_invokes_existing_wizard() {
    let temp = tempfile::tempdir().unwrap();
    let path = config(temp.path());
    let before = std::fs::read_to_string(&path).unwrap();
    let mut terminal = Terminal::start(temp.path(), &path);
    terminal.until("❯ ");
    terminal.send("/m\x1b[C\r");
    terminal.until("Current model:");
    terminal.send("\x1b[B\x1b[B\r");
    terminal.until("Built-in providers:");
    terminal.send("q\r");
    terminal.until("Model setup canceled");
    terminal.finish();
    assert_eq!(std::fs::read_to_string(path).unwrap(), before);
}

#[test]
fn model_menu_switches_profile_without_saving_default() {
    let temp = tempfile::tempdir().unwrap();
    let path = config(temp.path());
    let before = std::fs::read_to_string(&path).unwrap();
    let mut terminal = Terminal::start(temp.path(), &path);
    terminal.until("❯ ");
    terminal.send("/model\r");
    terminal.until("Add provider");
    terminal.send("\x1b[B\r");
    terminal.until("Save as default? [y/N]:");
    terminal.send("n\r");
    terminal.until("Model switched to work");
    terminal.finish();
    assert_eq!(std::fs::read_to_string(path).unwrap(), before);
}

#[test]
fn model_menu_can_cancel_switch_without_changing_session_or_config() {
    let temp = tempfile::tempdir().unwrap();
    let path = config(temp.path());
    let before = std::fs::read_to_string(&path).unwrap();
    let mut terminal = Terminal::start(temp.path(), &path);
    terminal.until("❯ ");
    terminal.send("/model\r");
    terminal.until("Add provider");
    terminal.send("\x1b[B\r");
    terminal.until("Save as default? [y/N]:");
    let checkpoint = terminal.output.len();
    terminal.send("q\r");
    terminal.until_from("❯ ", checkpoint);
    let checkpoint = terminal.output.len();
    terminal.send("/model\r");
    terminal.until_from("Current model: default", checkpoint);
    terminal.until_from("Add provider", checkpoint);
    let checkpoint = terminal.output.len();
    terminal.send("q\r");
    terminal.until_from("\x1b[K❯ ", checkpoint);
    terminal.finish();
    assert_eq!(std::fs::read_to_string(path).unwrap(), before);
}

#[test]
fn model_menu_can_save_selected_profile_as_default() {
    let temp = tempfile::tempdir().unwrap();
    let path = config(temp.path());
    let mut terminal = Terminal::start(temp.path(), &path);
    terminal.until("❯ ");
    terminal.send("/model\r");
    terminal.until("Add provider");
    terminal.send("\x1b[B\r");
    terminal.until("Save as default? [y/N]:");
    terminal.send("y\r");
    terminal.until("Model switched to work");
    terminal.finish();
    let persisted: nano_assistant::config::Config =
        toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(persisted.models.default.as_deref(), Some("work"));
}

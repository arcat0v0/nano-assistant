//! Windows platform implementation (stub).

use std::path::PathBuf;

use super::Platform;

pub struct WindowsPlatform;

impl Platform for WindowsPlatform {
    fn config_dir(&self) -> PathBuf {
        unimplemented!("Windows support planned")
    }

    fn agents_skills_dir(&self) -> PathBuf {
        unimplemented!("Windows support planned")
    }

    fn shell_command(&self) -> (&'static str, &'static str) {
        unimplemented!("Windows support planned")
    }

    fn expand_tilde(&self, _path: &str) -> PathBuf {
        unimplemented!("Windows support planned")
    }

    fn spawn_pty(&self, _command: &str) -> Result<Box<dyn super::PtyProcess>, std::io::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "PTY not yet supported on Windows",
        ))
    }
}

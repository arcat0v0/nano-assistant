use crate::agent::loop_::{Agent, TurnResult};
use std::io::{self, Write};

pub enum StreamOutputEvent {
    Clear,
    Progress(String),
    Content(String),
}

/// Streams an agent turn to stdout/stderr with real-time output.
///
/// Wraps `Agent::turn_streamed()` to provide:
/// - Immediate flushing of each text delta
/// - Tool call progress on stderr and content on stdout
pub async fn turn_streamed_to_stdout(
    agent: &mut Agent,
    user_message: &str,
) -> anyhow::Result<TurnResult> {
    let mut loading = LoadingIndicator::new();
    loading.show();
    let result = run_streamed_turn(agent, user_message, &mut loading).await?;
    loading.finish();
    println!();
    Ok(result)
}

async fn run_streamed_turn(
    agent: &mut Agent,
    user_message: &str,
    loading: &mut LoadingIndicator,
) -> anyhow::Result<TurnResult> {
    let mut printer = StreamPrinter::new();

    agent
        .turn_streamed(user_message, |event| {
            loading.clear_for_output();
            printer.print_event(event);
        })
        .await
}

struct LoadingIndicator {
    stderr: io::Stderr,
    active: bool,
}

impl LoadingIndicator {
    fn new() -> Self {
        Self {
            stderr: io::stderr(),
            active: false,
        }
    }

    fn show(&mut self) {
        if self.active {
            return;
        }

        let _ = self
            .stderr
            .write_all(b"\r\x1b[2K\x1b[2m\xe2\x8f\xb3 thinking...\x1b[0m");
        let _ = self.stderr.flush();
        self.active = true;
    }

    fn clear_for_output(&mut self) {
        if !self.active {
            return;
        }

        let _ = self.stderr.write_all(b"\r\x1b[2K");
        let _ = self.stderr.flush();
        self.active = false;
    }

    fn finish(&mut self) {
        self.clear_for_output();
    }
}

struct StreamPrinter {
    stdout: io::Stdout,
    stderr: io::Stderr,
}

impl StreamPrinter {
    fn new() -> Self {
        Self {
            stdout: io::stdout(),
            stderr: io::stderr(),
        }
    }

    fn print_event(&mut self, event: StreamOutputEvent) {
        match event {
            StreamOutputEvent::Clear => {
                let _ = self.stderr.write_all(b"\n");
                let _ = self.stderr.flush();
            }
            StreamOutputEvent::Progress(text) => {
                let _ = self.stderr.write_all(text.as_bytes());
                let _ = self.stderr.flush();
            }
            StreamOutputEvent::Content(text) => {
                let _ = self.stdout.write_all(text.as_bytes());
                let _ = self.stdout.flush();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_printer_writes_content() {
        let mut printer = StreamPrinter::new();
        printer.print_event(StreamOutputEvent::Content("test".into()));
    }

    #[test]
    fn stream_printer_writes_progress() {
        let mut printer = StreamPrinter::new();
        printer.print_event(StreamOutputEvent::Progress("progress".into()));
    }

    #[test]
    fn loading_indicator_transitions_cleanly() {
        let mut loading = LoadingIndicator::new();
        loading.show();
        assert!(loading.active);
        loading.clear_for_output();
        assert!(!loading.active);
    }
}

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
    let mut printer = StreamPrinter::new();
    let result = run_streamed_turn(agent, user_message, &mut loading, &mut printer).await?;
    loading.finish();
    println!();

    // Redraw: replace raw streamed text with rendered markdown
    let accumulated = printer.take_accumulated();
    if !accumulated.is_empty() {
        let raw_rendered = crate::render::render_markdown_fallback(&accumulated, 0);
        let line_count = crate::render::count_rendered_lines(&raw_rendered);
        if line_count > 0 {
            print!("\x1b[{}A\x1b[J", line_count);
            let _ = std::io::stdout().flush();
            crate::render::render_markdown_to_stdout(&accumulated);
        }
    }

    Ok(result)
}

async fn run_streamed_turn(
    agent: &mut Agent,
    user_message: &str,
    loading: &mut LoadingIndicator,
    printer: &mut StreamPrinter,
) -> anyhow::Result<TurnResult> {
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
    accumulated: String,
}

impl StreamPrinter {
    fn new() -> Self {
        Self {
            stdout: io::stdout(),
            stderr: io::stderr(),
            accumulated: String::new(),
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
                self.accumulated.push_str(&text);
            }
        }
    }

    fn take_accumulated(&mut self) -> String {
        std::mem::take(&mut self.accumulated)
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

    #[test]
    fn stream_printer_accumulates_content() {
        let mut printer = StreamPrinter::new();
        printer.print_event(StreamOutputEvent::Content("hello ".into()));
        printer.print_event(StreamOutputEvent::Content("world".into()));
        assert_eq!(printer.take_accumulated(), "hello world");
    }

    #[test]
    fn stream_printer_take_accumulated_clears() {
        let mut printer = StreamPrinter::new();
        printer.print_event(StreamOutputEvent::Content("first".into()));
        assert_eq!(printer.take_accumulated(), "first");
        assert_eq!(printer.take_accumulated(), "");
    }

    #[test]
    fn stream_printer_accumulates_multiple_chunks() {
        let mut printer = StreamPrinter::new();
        let chunks = vec!["Hello ", "world", "! This ", "is ", "a ", "test."];
        for chunk in chunks {
            printer.print_event(StreamOutputEvent::Content(chunk.into()));
        }
        assert_eq!(printer.take_accumulated(), "Hello world! This is a test.");
    }

    #[test]
    fn stream_printer_ignores_progress_for_accumulation() {
        let mut printer = StreamPrinter::new();
        printer.print_event(StreamOutputEvent::Progress("tool: running...".into()));
        printer.print_event(StreamOutputEvent::Content("visible text".into()));
        printer.print_event(StreamOutputEvent::Progress("tool: done".into()));
        assert_eq!(printer.take_accumulated(), "visible text");
    }
}

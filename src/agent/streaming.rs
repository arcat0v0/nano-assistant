use crate::agent::{Agent, TurnResult};
use std::io::{self, IsTerminal, Write};
use unicode_width::UnicodeWidthChar;

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

    let accumulated = printer.take_accumulated();
    if printer.stdout_is_terminal {
        if accumulated.is_empty() {
            crate::render::render_markdown_to_stdout(&result.response);
        } else {
            println!();
            if let Ok((columns, rows)) = crossterm::terminal::size() {
                let occupied_rows = terminal_rows(&accumulated, columns as usize);
                if occupied_rows > 0 && occupied_rows <= rows as usize {
                    print!("\x1b[{}A\x1b[J", occupied_rows);
                    let _ = std::io::stdout().flush();
                    crate::render::render_markdown_to_stdout(&result.response);
                }
            }
        }
    } else {
        crate::render::render_markdown_to_stdout(&result.response);
    }

    Ok(result)
}

/// Count the terminal rows occupied by text printed from column zero.
/// Newline count alone misses soft wraps, leaving part of the streamed response
/// on screen when the final Markdown rendering replaces it.
fn terminal_rows(text: &str, columns: usize) -> usize {
    if text.is_empty() {
        return 0;
    }

    let columns = columns.max(1);
    let chars: Vec<char> = text.chars().collect();
    let mut row_count = 1;
    let mut column = 0;
    let mut wrap_pending = false;
    let mut index = 0;

    while index < chars.len() {
        let character = chars[index];
        if character == '\x1b' {
            index = skip_terminal_sequence(&chars, index);
            continue;
        }
        index += 1;

        match character {
            '\n' => {
                row_count += 1;
                column = 0;
                wrap_pending = false;
            }
            '\r' => {
                column = 0;
                wrap_pending = false;
            }
            '\t' => {
                let target = (column / 8 + 1) * 8;
                while column < target {
                    advance_terminal_column(
                        1,
                        columns,
                        &mut row_count,
                        &mut column,
                        &mut wrap_pending,
                    );
                }
            }
            control if control.is_control() => {}
            printable => {
                let width = UnicodeWidthChar::width(printable).unwrap_or(0);
                if width > 0 {
                    advance_terminal_column(
                        width,
                        columns,
                        &mut row_count,
                        &mut column,
                        &mut wrap_pending,
                    );
                }
            }
        }
    }

    row_count
}

fn advance_terminal_column(
    width: usize,
    columns: usize,
    row_count: &mut usize,
    column: &mut usize,
    wrap_pending: &mut bool,
) {
    if *wrap_pending || *column + width > columns {
        *row_count += 1;
        *column = 0;
        *wrap_pending = false;
    }
    *column += width;
    if *column >= columns {
        *column = columns;
        *wrap_pending = true;
    }
}

fn skip_terminal_sequence(chars: &[char], start: usize) -> usize {
    let Some(&kind) = chars.get(start + 1) else {
        return start + 1;
    };
    match kind {
        '[' => {
            let mut index = start + 2;
            while index < chars.len() {
                let character = chars[index];
                index += 1;
                if ('@'..='~').contains(&character) {
                    break;
                }
            }
            index
        }
        ']' => {
            let mut index = start + 2;
            while index < chars.len() {
                if chars[index] == '\x07' {
                    return index + 1;
                }
                if chars[index] == '\x1b' && chars.get(index + 1) == Some(&'\\') {
                    return index + 2;
                }
                index += 1;
            }
            index
        }
        _ => start + 2,
    }
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
    stdout_is_terminal: bool,
}

impl StreamPrinter {
    fn new() -> Self {
        let stdout = io::stdout();
        let stdout_is_terminal = stdout.is_terminal();
        Self {
            stdout,
            stderr: io::stderr(),
            accumulated: String::new(),
            stdout_is_terminal,
        }
    }

    fn print_event(&mut self, event: StreamOutputEvent) {
        match event {
            StreamOutputEvent::Clear => {
                self.accumulated.clear();
                let _ = self.stderr.write_all(b"\n");
                let _ = self.stderr.flush();
            }
            StreamOutputEvent::Progress(text) => {
                self.accumulated.clear();
                let _ = self.stderr.write_all(text.as_bytes());
                let _ = self.stderr.flush();
            }
            StreamOutputEvent::Content(text) => {
                if self.stdout_is_terminal {
                    let _ = self.stdout.write_all(text.as_bytes());
                    let _ = self.stdout.flush();
                }
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
    fn stream_printer_keeps_only_text_after_the_last_progress_boundary() {
        let mut printer = StreamPrinter::new();
        printer.print_event(StreamOutputEvent::Progress("tool: running...".into()));
        printer.print_event(StreamOutputEvent::Content("visible text".into()));
        printer.print_event(StreamOutputEvent::Progress("tool: done".into()));
        printer.print_event(StreamOutputEvent::Content("final response".into()));
        assert_eq!(printer.take_accumulated(), "final response");
    }

    #[test]
    fn terminal_rows_count_soft_wraps_and_newlines() {
        let response = "Hi! I'm your system steward on this Arch Linux box. What can I help you with today?\n\nA few things I can do right away:\n- help with system updates";
        assert_eq!(terminal_rows(response, 100), 4);
        assert_eq!(terminal_rows(response, 80), 5);
    }

    #[test]
    fn terminal_rows_count_wide_characters_and_ignore_ansi_sequences() {
        assert_eq!(terminal_rows("界界界\n\x1b[31mred\x1b[0m", 4), 3);
    }

    #[test]
    fn terminal_rows_handle_exact_width_and_empty_text() {
        assert_eq!(terminal_rows("1234", 4), 1);
        assert_eq!(terminal_rows("12345", 4), 2);
        assert_eq!(terminal_rows("", 4), 0);
    }
}

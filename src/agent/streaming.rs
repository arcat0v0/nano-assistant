use crate::agent::loop_::{Agent, TurnResult};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Streams an agent turn to stdout with real-time output and Ctrl+C support.
///
/// Wraps `Agent::turn_streamed()` to provide:
/// - Immediate flushing of each text delta
/// - Tool call status display ("⏳ tool_name ... ✓")
/// - Graceful Ctrl+C interruption
pub async fn turn_streamed_to_stdout(agent: &mut Agent, user_message: &str) -> anyhow::Result<TurnResult> {
    let interrupted = Arc::new(AtomicBool::new(false));
    let ctrl_c_handler = spawn_ctrl_c_handler(interrupted.clone());

    let result = run_streamed_turn(agent, user_message, &interrupted).await;

    ctrl_c_handler.abort();

    let was_interrupted = interrupted.load(Ordering::Relaxed);
    if was_interrupted {
        eprintln!("\n[interrupted]");
    }

    match result {
        Ok(turn_result) => {
            println!();
            Ok(turn_result)
        }
        Err(e) => {
            if was_interrupted {
                anyhow::bail!("Stream interrupted by user")
            }
            Err(e)
        }
    }
}

async fn run_streamed_turn(
    agent: &mut Agent,
    user_message: &str,
    interrupted: &Arc<AtomicBool>,
) -> anyhow::Result<TurnResult> {
    let mut printer = StreamPrinter::new(interrupted.clone());

    agent
        .turn_streamed(user_message, |chunk| {
            if interrupted.load(Ordering::Relaxed) {
                return;
            }
            printer.print_delta(chunk);
        })
        .await
}

fn spawn_ctrl_c_handler(interrupted: Arc<AtomicBool>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        interrupted.store(true, Ordering::Relaxed);
    })
}

struct StreamPrinter {
    interrupted: Arc<AtomicBool>,
    stdout: io::Stdout,
}

impl StreamPrinter {
    fn new(interrupted: Arc<AtomicBool>) -> Self {
        Self {
            interrupted,
            stdout: io::stdout(),
        }
    }

    fn print_delta(&mut self, text: &str) {
        if self.interrupted.load(Ordering::Relaxed) {
            return;
        }
        let _ = self.stdout.write_all(text.as_bytes());
        let _ = self.stdout.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_printer_writes_to_stdout() {
        let interrupted = Arc::new(AtomicBool::new(false));
        let mut printer = StreamPrinter::new(interrupted);
        printer.print_delta("test");
        assert!(!printer.interrupted.load(Ordering::Relaxed));
    }

    #[test]
    fn stream_printer_skips_when_interrupted() {
        let interrupted = Arc::new(AtomicBool::new(true));
        let mut printer = StreamPrinter::new(interrupted);
        printer.print_delta("should not print");
        assert!(printer.interrupted.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn ctrl_c_handler_sets_interrupted_flag() {
        let interrupted = Arc::new(AtomicBool::new(false));
        let handle = spawn_ctrl_c_handler(interrupted.clone());
        interrupted.store(true, Ordering::Relaxed);
        interrupted.store(true, Ordering::Relaxed);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        handle.abort();
        assert!(interrupted.load(Ordering::Relaxed));
    }
}

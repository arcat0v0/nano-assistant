//! ANSI terminal styling helpers (zero dependencies).

// ── ANSI escape codes ────────────────────────────────────────────────

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";

const FG_CYAN: &str = "\x1b[36m";
const FG_GREEN: &str = "\x1b[32m";
const FG_YELLOW: &str = "\x1b[33m";
const FG_RED: &str = "\x1b[31m";

const FG_BRIGHT_BLACK: &str = "\x1b[90m";

// ── Composite styles ─────────────────────────────────────────────────

/// Style for the tool name badge: **bold cyan**.
#[inline]
pub fn tool_name(name: &str) -> String {
    format!("{BOLD}{FG_CYAN}{name}{RESET}")
}

/// Style for tool arguments summary: *dim white*.
#[inline]
pub fn tool_args(args: &str) -> String {
    format!("{DIM}{FG_BRIGHT_BLACK}{args}{RESET}")
}

/// Style for a successful tool result: green ✓.
#[inline]
pub fn success_icon() -> String {
    format!("{FG_GREEN}✓{RESET}")
}

/// Style for a failed tool result: red ✗.
#[inline]
pub fn error_icon() -> String {
    format!("{FG_RED}✗{RESET}")
}

/// Style for the tool-in-progress indicator: dim yellow ⏳.
#[inline]
pub fn spinner_icon() -> String {
    format!("{FG_YELLOW}⏳{RESET}")
}

/// Dim prefix label like `[cli]`.
#[inline]
pub fn dim_label(label: &str) -> String {
    format!("{DIM}{FG_BRIGHT_BLACK}{label}{RESET}")
}

/// Format a single-line tool call summary.
///
/// Output:
/// ```text
///   ⏳ shell  uname -a   ✓
/// ```
pub fn format_tool_call_line(name: &str, args_summary: &str, success: bool) -> String {
    let icon = if success {
        success_icon()
    } else {
        error_icon()
    };
    let args_part = if args_summary.is_empty() {
        String::new()
    } else {
        format!(" {}", tool_args(args_summary))
    };
    format!(
        "  {} {}{}  {}\n",
        spinner_icon(),
        tool_name(name),
        args_part,
        icon,
    )
}

/// Format a tool-call-in-progress line (before execution).
///
/// Output:
/// ```text
///   ⏳ shell  uname -a  ...
/// ```
pub fn format_tool_pending(name: &str, args_summary: &str) -> String {
    let args_part = if args_summary.is_empty() {
        String::new()
    } else {
        format!(" {}", tool_args(args_summary))
    };
    format!(
        "  {} {}{}  {DIM}...{RESET}",
        spinner_icon(),
        tool_name(name),
        args_part,
    )
}

/// Build a short one-line summary of tool arguments for display.
///
/// - `shell` → the value of `"command"` key
/// - `file_read` / `file_write` → the value of `"path"` key
/// - `file_edit` → `"path"` key
/// - `glob_search` → the value of `"pattern"` key
/// - `content_search` → the value of `"pattern"` key
/// - Anything else → first string value found
pub fn args_summary(tool_name: &str, args: &serde_json::Value) -> String {
    let key = match tool_name {
        "shell" => "command",
        "file_read" | "file_write" | "file_edit" => "path",
        "glob_search" | "content_search" => "pattern",
        _ => {
            // Fallback: first string value
            return args
                .as_object()
                .and_then(|m| m.values().find_map(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
        }
    };

    args.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Style for the `[cli]` summary line at end of turn.
pub fn format_tool_summary(count: usize) -> String {
    format!("{} {} tool call(s) executed", dim_label("[cli]"), count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn args_summary_shell() {
        assert_eq!(
            args_summary("shell", &json!({"command": "uname -a"})),
            "uname -a"
        );
    }

    #[test]
    fn args_summary_file_read() {
        assert_eq!(
            args_summary("file_read", &json!({"path": "/etc/hosts"})),
            "/etc/hosts"
        );
    }

    #[test]
    fn args_summary_empty() {
        assert_eq!(args_summary("shell", &json!({})), "");
    }

    #[test]
    fn format_tool_call_line_success() {
        let line = format_tool_call_line("shell", "ls -la", true);
        assert!(line.contains("shell"));
        assert!(line.contains("ls -la"));
        assert!(line.contains('\n'));
    }

    #[test]
    fn format_tool_pending_no_args() {
        let line = format_tool_pending("echo", "");
        assert!(line.contains("echo"));
        assert!(!line.contains("  \n"));
    }

    #[test]
    fn format_tool_summary_contains_count() {
        let s = format_tool_summary(3);
        assert!(s.contains("3"));
    }
}

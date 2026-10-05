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

#[inline]
pub fn green(text: &str) -> String {
    format!("{FG_GREEN}{text}{RESET}")
}

#[inline]
pub fn yellow(text: &str) -> String {
    format!("{FG_YELLOW}{text}{RESET}")
}

#[inline]
pub fn red(text: &str) -> String {
    format!("{FG_RED}{text}{RESET}")
}

/// Dim prefix label like `[cli]`.
#[inline]
pub fn dim_label(label: &str) -> String {
    format!("{DIM}{FG_BRIGHT_BLACK}{label}{RESET}")
}

#[inline]
pub fn review_note(text: &str) -> String {
    format!("  {}  🛡 {text}", dim_label("│"))
}

#[inline]
pub fn confirm_prompt() -> String {
    format!("{BOLD}{FG_YELLOW}⚠ [y/N]{RESET}")
}

/// Format the closing line of a tool block.
///
/// Output:
/// ```text
///   ╰─ ✓ shell · result
/// ```
pub fn format_tool_call_line(name: &str, _args_summary: &str, success: bool) -> String {
    let icon = if success {
        success_icon()
    } else {
        error_icon()
    };
    format!(
        "  {} {icon} {} · result\n",
        dim_label("╰─"),
        dim_label(name)
    )
}

/// Format the opening line of a tool block (before execution).
///
/// Output:
/// ```text
///   ╭─ ⚒ shell · uname -a
/// ```
pub fn format_tool_pending(name: &str, args_summary: &str) -> String {
    let args_part = if args_summary.is_empty() {
        String::new()
    } else {
        format!(" · {}", tool_args(&compact_text(args_summary)))
    };
    format!(
        "  {} {}{args_part}",
        dim_label("╭─"),
        tool_name(&format!("⚒ {name}"))
    )
}

fn compact_text(text: &str) -> String {
    use unicode_width::UnicodeWidthChar;
    let mut output = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let part = if ch.is_control() {
            ch.escape_default().to_string()
        } else {
            ch.to_string()
        };
        let part_width: usize = part.chars().map(|ch| ch.width().unwrap_or(0)).sum();
        if width + part_width > 95 {
            output.push('…');
            break;
        }
        output.push_str(&part);
        width += part_width;
    }
    output
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
        "shell" | "pty_shell" => "command",
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
    format!("{} {} tool call(s) handled", dim_label("[cli]"), count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn progress_is_compact_and_escapes_terminal_controls() {
        let text = format_tool_pending("shell", &format!("echo\n\x1b[2J{}", "界".repeat(100)));
        assert_eq!(text.lines().count(), 1);
        assert!(!text.contains("\x1b[2J"));
        assert!(text.contains('…'));
        assert!(text.chars().count() < 150);
    }

    #[test]
    fn completed_progress_has_only_a_final_status() {
        let text = format_tool_call_line("shell", "", false);
        assert!(text.contains('✗'));
        assert!(!text.contains('⏳'));
        assert!(text.contains("result"));
    }

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
        assert!(line.contains("╰─"));
        assert!(line.contains("shell"));
        assert!(line.contains('\n'));
    }

    #[test]
    fn pending_block_opens_with_header() {
        let line = format_tool_pending("#1 shell", "ls -la");
        assert!(line.contains("╭─"));
        assert!(line.contains('⚒'));
        assert!(line.contains("#1 shell"));
        assert!(line.contains("ls -la"));
        assert_eq!(line.lines().count(), 1);
    }

    #[test]
    fn review_note_sits_on_block_gutter() {
        let line = review_note(&format!("{} · risk 4 · fine", green("safe")));
        assert!(line.contains('│'));
        assert!(line.contains('🛡'));
        assert!(line.contains("safe"));
    }

    #[test]
    fn verdict_words_carry_distinct_colors() {
        assert!(green("safe").contains("\x1b[32m"));
        assert!(yellow("uncertain").contains("\x1b[33m"));
        assert!(red("blocked").contains("\x1b[31m"));
    }

    #[test]
    fn confirm_prompt_is_flagged() {
        assert!(confirm_prompt().contains('⚠'));
        assert!(confirm_prompt().contains("[y/N]"));
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

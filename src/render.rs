//! Markdown-to-terminal rendering using [termimad].
//!
//! Provides styled rendering that matches the `console.rs` accent palette
//! (cyan `#1;36m` for headers, etc.).

use std::panic::{catch_unwind, AssertUnwindSafe};

use termimad::{crossterm::style::Color, MadSkin};

// ── Skin construction ─────────────────────────────────────────────────

/// Build a custom [`MadSkin`] whose palette matches the project's
/// `console.rs` accent colours (cyan headers, styled code blocks, …).
pub fn create_skin() -> MadSkin {
    let mut skin = MadSkin::default();

    skin.set_headers_fg(Color::Cyan);
    skin.headers[0]
        .compound_style
        .add_attr(termimad::crossterm::style::Attribute::Bold);

    skin.bold.set_fg(Color::White);
    skin.italic.set_fg(Color::DarkCyan);
    skin.inline_code.set_fgbg(Color::White, Color::DarkGrey);
    skin.code_block.set_fgbg(Color::Grey, Color::DarkGrey);

    skin
}

// ── Public API ────────────────────────────────────────────────────────

/// Render a Markdown string into a styled terminal string.
///
/// The `width` parameter controls the maximum line width (in terminal
/// columns).  Pass `0` to let termimad use the current terminal width.
pub fn render_markdown(md: &str, width: usize) -> String {
    if md.is_empty() {
        return String::new();
    }
    let skin = create_skin();
    let effective_width = if width == 0 { None } else { Some(width) };
    skin.text(md, effective_width).to_string()
}

/// Like [`render_markdown`] but catches panics and returns the raw input
/// text instead of propagating the panic.
pub fn render_markdown_fallback(md: &str, width: usize) -> String {
    catch_unwind(AssertUnwindSafe(|| render_markdown(md, width))).unwrap_or_else(|_| md.to_string())
}

/// Count how many terminal lines the *already-rendered* output occupies.
///
/// Simply counts `'\n'`-delimited lines (the rendered string already
/// contains hard newlines for wrapping).
pub fn count_rendered_lines(rendered: &str) -> usize {
    if rendered.is_empty() {
        return 0;
    }
    rendered.lines().count()
}

/// Render Markdown and print directly to stdout.
pub fn render_markdown_to_stdout(md: &str) {
    if md.is_empty() {
        return;
    }
    let skin = create_skin();
    skin.print_text(md);
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: usize = 80;

    #[test]
    fn test_render_markdown_basic() {
        let md = "# Hello\n\n**bold** and `code`";
        let result = render_markdown(md, WIDTH);
        assert!(result.contains("Hello"), "should contain the header text");
        assert!(result.contains("bold"), "should contain bold text");
        assert!(result.contains("code"), "should contain inline code");
    }

    #[test]
    fn test_render_markdown_lists() {
        let md = "- item one\n- item two\n- item three";
        let result = render_markdown(md, WIDTH);
        assert!(result.contains("item one"));
        assert!(result.contains("item two"));
        assert!(result.contains("item three"));
    }

    #[test]
    fn test_render_markdown_code_blocks() {
        let md = "```\nfn main() {}\n```";
        let result = render_markdown(md, WIDTH);
        assert!(result.contains("fn main()"));
    }

    #[test]
    fn test_render_markdown_tables() {
        let md = "| A | B |\n|---|---|\n| 1 | 2 |";
        let result = render_markdown(md, WIDTH);
        assert!(result.contains("1"));
        assert!(result.contains("2"));
    }

    #[test]
    fn test_render_empty_input() {
        let result = render_markdown("", WIDTH);
        assert!(result.is_empty(), "empty input must yield empty output");
    }

    #[test]
    fn test_render_contains_ansi() {
        let md = "# Hello";
        let result = render_markdown(md, WIDTH);
        assert!(
            result.contains('\x1b'),
            "rendered output must contain ANSI escape codes"
        );
    }

    // -- render_markdown_fallback ----------------------------------------

    #[test]
    fn test_render_markdown_fallback_no_panic() {
        let md = "**bold** text";
        assert_eq!(
            render_markdown_fallback(md, WIDTH),
            render_markdown(md, WIDTH),
        );
    }

    #[test]
    fn test_render_markdown_fallback_malformed() {
        let md = "```\nunclosed fence\n\n``````\n``````````";
        let result = render_markdown_fallback(md, WIDTH);
        assert!(!result.is_empty() || md.is_empty());
    }

    // -- count_rendered_lines --------------------------------------------

    #[test]
    fn test_count_rendered_lines_basic() {
        let rendered = "line one\nline two\nline three";
        assert_eq!(count_rendered_lines(rendered), 3);
    }

    #[test]
    fn test_count_rendered_lines_empty() {
        assert_eq!(count_rendered_lines(""), 0);
    }

    #[test]
    fn test_count_rendered_lines_from_render() {
        let md = "# Title\n\nParagraph one.\n\nParagraph two.";
        let rendered = render_markdown(md, WIDTH);
        let lines = count_rendered_lines(&rendered);
        assert!(lines >= 3, "expected at least 3 lines, got {lines}");
    }

    #[test]
    fn test_render_code_block_only_response() {
        let md = "```rust\nfn main() {\n    println!(\"Hello\");\n}\n```";
        let result = render_markdown_fallback(md, 80);
        assert!(result.contains("fn main()"));
        assert!(result.contains("println!"));
    }

    #[test]
    fn test_render_table_wider_than_terminal() {
        let md = "| Col1 | Col2 | Col3 | Col4 | Col5 | Col6 | Col7 | Col8 | Col9 | Col10 |\n|------|------|------|------|------|------|------|------|------|-------|\n| a | b | c | d | e | f | g | h | i | j |";
        let result = render_markdown_fallback(md, 40);
        assert!(result.contains("a") || result.contains("Col1"));
    }

    #[test]
    fn test_render_large_response() {
        let mut md = String::from("# Large Document\n\n");
        for i in 0..500 {
            md.push_str(&format!("Paragraph {} with some content.\n\n", i));
        }
        let result = render_markdown_fallback(&md, 80);
        assert!(result.contains("Paragraph 0"));
        assert!(result.contains("Paragraph 499"));
        let lines = count_rendered_lines(&result);
        assert!(lines > 500, "expected >500 lines, got {lines}");
    }

    #[test]
    fn test_render_deeply_nested_formatting() {
        let md = "**bold *italic* ~~strikethrough~~** and `code` and [link](url)";
        let result = render_markdown_fallback(md, 80);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_render_markdown_to_stdout_empty() {
        render_markdown_to_stdout("");
        render_markdown_to_stdout("\n\n");
    }
}

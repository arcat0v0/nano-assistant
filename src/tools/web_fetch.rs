use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use scraper::{Html, Selector};
use serde_json::json;
use std::time::Duration;

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_REDIRECTS: usize = 5;
const MAX_OUTPUT_BYTES: usize = 1_048_576; // 1 MiB
const DEFAULT_MAX_LENGTH: usize = 102_400; // 100 KB

/// Convert HTML to readable markdown/text.
///
/// Tries to extract main content via common wiki/article selectors,
/// falls back to `<body>` or the full document.
pub fn html_to_markdown(html: &str) -> String {
    html_to_markdown_with_selector(html, None)
}

/// Convert HTML to readable text, optionally using a custom CSS selector
/// to extract a specific portion of the page.
pub fn html_to_markdown_with_selector(html: &str, selector: Option<&str>) -> String {
    let document = Html::parse_document(html);

    // If caller provided a custom selector, try it first
    let custom_selectors: Vec<&str> = match selector {
        Some(s) => vec![s],
        None => Vec::new(),
    };

    // Common content selectors for wikis and article pages
    let default_selectors = [
        "#mw-content-text", // MediaWiki
        "#content",         // Generic content div
        "article",          // HTML5 article element
        "main",             // HTML5 main element
        ".moin-content",    // MoinMoin wiki
    ];

    let all_selectors: Vec<&str> = custom_selectors
        .iter()
        .chain(default_selectors.iter())
        .copied()
        .collect();

    for sel_str in &all_selectors {
        if let Ok(sel) = Selector::parse(sel_str) {
            if let Some(element) = document.select(&sel).next() {
                let inner_html = element.inner_html();
                return html2text::from_read(inner_html.as_bytes(), 120)
                    .unwrap_or_else(|_| inner_html);
            }
        }
    }

    // Fallback: try <body>, then full document
    if let Ok(body_sel) = Selector::parse("body") {
        if let Some(body) = document.select(&body_sel).next() {
            let inner = body.inner_html();
            return html2text::from_read(inner.as_bytes(), 120).unwrap_or_else(|_| inner);
        }
    }

    html2text::from_read(html.as_bytes(), 120).unwrap_or_else(|_| html.to_string())
}

/// Fetch a URL and return its content as readable text.
/// HTML is converted to plain text; JSON and plain text are returned as-is.
pub struct WebFetchTool {
    client: reqwest::Client,
}

impl WebFetchTool {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .user_agent(format!("nano-assistant/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { client }
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

fn truncate_to_limit(s: &mut String, limit: usize) {
    let limit = limit.min(MAX_OUTPUT_BYTES);
    if s.len() > limit {
        let mut boundary = limit;
        while boundary > 0 && !s.is_char_boundary(boundary) {
            boundary -= 1;
        }
        s.truncate(boundary);
        s.push_str("\n... [output truncated]");
    }
}

/// Determine if a content-type header indicates HTML.
fn is_html(content_type: &str) -> bool {
    content_type.contains("text/html") || content_type.contains("application/xhtml")
}

/// Determine if a content-type header indicates binary (non-text) content.
fn is_binary(content_type: &str) -> bool {
    let text_types = [
        "text/",
        "application/json",
        "application/xml",
        "application/javascript",
        "application/x-yaml",
        "application/toml",
    ];
    !text_types.iter().any(|t| content_type.contains(t))
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch a URL and return its content as readable text. \
         HTML pages are converted to plain text. \
         JSON and plain text are returned as-is."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "max_length": {
                    "type": "integer",
                    "description": "Maximum output length in bytes (default: 102400, max: 1048576)"
                },
                "selector": {
                    "type": "string",
                    "description": "Optional CSS selector to extract specific content from HTML pages"
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'url' parameter"))?;

        let max_length = args
            .get("max_length")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_MAX_LENGTH);

        let selector = args
            .get("selector")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let response = match self.client.get(url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Request failed: {e}")),
                });
            }
        };

        let status = response.status();
        if !status.is_success() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("HTTP {status}")),
            });
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/plain")
            .to_string();

        if is_binary(&content_type) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Binary content type not supported: {content_type}")),
            });
        }

        let body = match response.text().await {
            Ok(text) => text,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to read response body: {e}")),
                });
            }
        };

        let mut output = if is_html(&content_type) {
            html_to_markdown_with_selector(&body, selector.as_deref())
        } else {
            body
        };

        truncate_to_limit(&mut output, max_length);

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_metadata() {
        let tool = WebFetchTool::new();
        assert_eq!(tool.name(), "web_fetch");
        assert!(tool.description().contains("URL"));
        let schema = tool.parameters_schema();
        assert_eq!(schema["required"], json!(["url"]));
    }

    #[tokio::test]
    async fn missing_url_param_returns_error() {
        let tool = WebFetchTool::new();
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
    }

    #[test]
    fn is_html_detects_html_content_types() {
        assert!(is_html("text/html; charset=utf-8"));
        assert!(is_html("application/xhtml+xml"));
        assert!(!is_html("application/json"));
        assert!(!is_html("text/plain"));
    }

    #[test]
    fn is_binary_detects_binary_content_types() {
        assert!(is_binary("image/png"));
        assert!(is_binary("application/octet-stream"));
        assert!(!is_binary("text/html"));
        assert!(!is_binary("application/json"));
        assert!(!is_binary("text/plain"));
    }

    #[test]
    fn truncate_respects_limit() {
        let mut s = "abcdefghij".to_string();
        truncate_to_limit(&mut s, 5);
        assert!(s.starts_with("abcde"));
        assert!(s.contains("[output truncated]"));
    }

    #[test]
    fn truncate_noop_when_under_limit() {
        let mut s = "short".to_string();
        truncate_to_limit(&mut s, 100);
        assert_eq!(s, "short");
    }
}

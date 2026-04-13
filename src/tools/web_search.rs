use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;

const DEFAULT_TIMEOUT_SECS: u64 = 15;
const DUCKDUCKGO_URL: &str = "https://html.duckduckgo.com/html/";
const MAX_OUTPUT_BYTES: usize = 1_048_576;
const DEFAULT_MAX_RESULTS: usize = 10;

/// Search result from DuckDuckGo.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Search the web using DuckDuckGo and return results.
pub struct WebSearchTool {
    client: reqwest::Client,
}

impl WebSearchTool {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .user_agent(format!("nano-assistant/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { client }
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse DuckDuckGo HTML search results page.
/// Extracts title, URL, and snippet from each result block.
///
/// DDG HTML result structure:
/// ```html
/// <a class="result__a" href="URL">TITLE</a>
/// <a class="result__snippet" ...>SNIPPET</a>
/// ```
pub fn parse_ddg_results(html: &str, max_results: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();

    let mut search_pos = 0;
    while results.len() < max_results {
        let link_marker = "class=\"result__a\"";
        let Some(marker_pos) = html[search_pos..].find(link_marker) else {
            break;
        };
        let abs_marker = search_pos + marker_pos;

        // Extract href from the link tag
        let url = extract_href(&html[search_pos..abs_marker + link_marker.len() + 200])
            .unwrap_or_default();

        // Extract title text (between > and </a>)
        let title_start = abs_marker + link_marker.len();
        let title = extract_tag_text(&html[title_start..]).unwrap_or_default();

        // Find snippet in nearby content
        let snippet_region_start = title_start;
        let snippet_region_end = (snippet_region_start + 2000).min(html.len());
        let snippet_region = &html[snippet_region_start..snippet_region_end];
        let snippet = extract_snippet(snippet_region);

        if !url.is_empty() && !title.is_empty() {
            let clean_url = clean_ddg_url(&url);
            results.push(SearchResult {
                title: html_decode(&title),
                url: clean_url,
                snippet: html_decode(&snippet),
            });
        }

        search_pos = abs_marker + link_marker.len();
    }

    results
}

fn extract_href(fragment: &str) -> Option<String> {
    let href_pos = fragment.find("href=\"")?;
    let start = href_pos + 6;
    let end = fragment[start..].find('"')? + start;
    Some(fragment[start..end].to_string())
}

fn extract_tag_text(fragment: &str) -> Option<String> {
    let start = fragment.find('>')? + 1;
    let end = fragment[start..].find("</a>")? + start;
    let raw = &fragment[start..end];
    Some(strip_html_tags(raw).trim().to_string())
}

fn extract_snippet(region: &str) -> String {
    let marker = "class=\"result__snippet\"";
    if let Some(pos) = region.find(marker) {
        let after = &region[pos + marker.len()..];
        if let Some(text) = extract_tag_text(after) {
            return text;
        }
    }
    String::new()
}

/// Clean DuckDuckGo redirect URL to extract the actual target URL.
fn clean_ddg_url(url: &str) -> String {
    if let Some(uddg_pos) = url.find("uddg=") {
        let encoded = &url[uddg_pos + 5..];
        let end = encoded.find('&').unwrap_or(encoded.len());
        let encoded_url = &encoded[..end];
        return url_decode(encoded_url);
    }
    url.to_string()
}

fn url_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next().unwrap_or(b'0');
            let lo = chars.next().unwrap_or(b'0');
            let hex = [hi, lo];
            if let Ok(hex_str) = std::str::from_utf8(&hex) {
                if let Ok(byte) = u8::from_str_radix(hex_str, 16) {
                    result.push(byte as char);
                    continue;
                }
            }
            result.push('%');
            result.push(hi as char);
            result.push(lo as char);
        } else if b == b'+' {
            result.push(' ');
        } else {
            result.push(b as char);
        }
    }
    result
}

fn strip_html_tags(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result
}

fn html_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn format_results(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "No results found.".to_string();
    }

    let mut output = String::new();
    for (i, r) in results.iter().enumerate() {
        output.push_str(&format!("{}. [{}]({})\n", i + 1, r.title, r.url));
        if !r.snippet.is_empty() {
            output.push_str(&format!("   {}\n", r.snippet));
        }
        output.push('\n');
    }
    output
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web using DuckDuckGo and return results with titles, URLs, and snippets. \
         Free, no API key required."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'query' parameter"))?;

        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_MAX_RESULTS);

        let response = self
            .client
            .post(DUCKDUCKGO_URL)
            .form(&[("q", query), ("kl", "")])
            .send()
            .await;

        let html = match response {
            Ok(resp) if resp.status().is_success() => match resp.text().await {
                Ok(text) => text,
                Err(e) => {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Failed to read response: {e}")),
                    });
                }
            },
            Ok(resp) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("HTTP {}", resp.status())),
                });
            }
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Search request failed: {e}")),
                });
            }
        };

        let results = parse_ddg_results(&html, max_results);
        let mut output = format_results(&results);

        if output.len() > MAX_OUTPUT_BYTES {
            let mut boundary = MAX_OUTPUT_BYTES;
            while boundary > 0 && !output.is_char_boundary(boundary) {
                boundary -= 1;
            }
            output.truncate(boundary);
            output.push_str("\n... [results truncated]");
        }

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
        let tool = WebSearchTool::new();
        assert_eq!(tool.name(), "web_search");
        assert!(tool.description().contains("DuckDuckGo"));
        let schema = tool.parameters_schema();
        assert_eq!(schema["required"], json!(["query"]));
    }

    #[tokio::test]
    async fn missing_query_param_returns_error() {
        let tool = WebSearchTool::new();
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
    }

    #[test]
    fn url_decode_handles_common_encodings() {
        assert_eq!(
            url_decode("https%3A%2F%2Fexample.com"),
            "https://example.com"
        );
        assert_eq!(url_decode("hello+world"), "hello world");
        assert_eq!(url_decode("no%20encoding"), "no encoding");
    }

    #[test]
    fn strip_html_tags_removes_tags() {
        assert_eq!(strip_html_tags("<b>bold</b> text"), "bold text");
        assert_eq!(strip_html_tags("no tags"), "no tags");
        assert_eq!(strip_html_tags("<a href=\"x\">link</a>"), "link");
    }

    #[test]
    fn html_decode_handles_entities() {
        assert_eq!(html_decode("a &amp; b"), "a & b");
        assert_eq!(html_decode("&lt;tag&gt;"), "<tag>");
        assert_eq!(html_decode("it&#39;s"), "it's");
    }

    #[test]
    fn clean_ddg_url_extracts_target() {
        let ddg = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com&rut=abc";
        assert_eq!(clean_ddg_url(ddg), "https://example.com");
    }

    #[test]
    fn clean_ddg_url_passthrough_normal_url() {
        assert_eq!(clean_ddg_url("https://example.com"), "https://example.com");
    }

    #[test]
    fn format_results_empty() {
        assert_eq!(format_results(&[]), "No results found.");
    }

    #[test]
    fn format_results_with_items() {
        let results = vec![SearchResult {
            title: "Example".to_string(),
            url: "https://example.com".to_string(),
            snippet: "An example site".to_string(),
        }];
        let output = format_results(&results);
        assert!(output.contains("1. [Example](https://example.com)"));
        assert!(output.contains("An example site"));
    }

    #[test]
    fn parse_ddg_results_extracts_from_sample_html() {
        let html = r#"
        <div class="result">
            <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Frust-lang.org&rut=abc">Rust Programming Language</a>
            <a class="result__snippet">A language empowering everyone to build reliable software.</a>
        </div>
        "#;
        let results = parse_ddg_results(html, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://rust-lang.org");
        assert!(results[0].snippet.contains("reliable software"));
    }

    #[test]
    fn parse_ddg_results_respects_max() {
        let html = r#"
        <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fa.com">A</a>
        <a class="result__snippet">Snippet A</a>
        <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fb.com">B</a>
        <a class="result__snippet">Snippet B</a>
        <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fc.com">C</a>
        <a class="result__snippet">Snippet C</a>
        "#;
        let results = parse_ddg_results(html, 2);
        assert_eq!(results.len(), 2);
    }
}

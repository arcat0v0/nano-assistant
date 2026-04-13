# Layer 1: Skill Paths + Web Tools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend nano-assistant with skills.sh compatibility (read `~/.agents/skills/`) and two new built-in web tools (`web_fetch`, `web_search`).

**Architecture:** Add `extra_paths` to `SkillsConfig` and modify `load_skills()` to scan multiple directories with priority-based dedup. Add two new tool modules (`web_fetch.rs`, `web_search.rs`) following the existing `ShellTool` pattern. New dependency: `html2text` for HTML-to-text conversion.

**Tech Stack:** Rust, reqwest (existing), html2text (new), tokio (existing)

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `Cargo.toml` | Modify | Add `html2text` dependency |
| `src/config/schema.rs` | Modify | Add `extra_paths` field to `SkillsConfig` |
| `src/config/mod.rs` | Modify | Re-export updated types |
| `src/skills/mod.rs` | Modify | Multi-directory loading with dedup |
| `src/tools/web_fetch.rs` | Create | `WebFetchTool` — fetch URL, HTML-to-text |
| `src/tools/web_search.rs` | Create | `WebSearchTool` — DuckDuckGo HTML search |
| `src/tools/mod.rs` | Modify | Register new tools in `default_tools()` |

---

### Task 1: Add html2text dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add html2text to Cargo.toml**

In the `[dependencies]` section, after the `reqwest` line, add:

```toml
# HTML to text conversion (for web_fetch / web_search)
html2text = "0.14"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check 2>&1 | tail -5`
Expected: `Finished` with no errors (just downloading the new crate)

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: add html2text for web tool HTML parsing"
```

---

### Task 2: Add extra_paths to SkillsConfig

**Files:**
- Modify: `src/config/schema.rs`

- [ ] **Step 1: Write the failing test**

Add this test at the bottom of the `mod tests` block in `src/config/schema.rs`:

```rust
#[test]
fn skills_config_extra_paths_default_empty() {
    let s = SkillsConfig::default();
    assert!(s.extra_paths.is_empty());
}

#[test]
fn toml_deserialization_skills_extra_paths() {
    let toml_str = r#"
        [skills]
        extra_paths = ["/opt/my-skills", "~/.agents/skills"]
    "#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.skills.extra_paths, vec!["/opt/my-skills", "~/.agents/skills"]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib config::schema::tests::skills_config_extra_paths_default_empty 2>&1 | tail -5`
Expected: FAIL — `extra_paths` field does not exist yet

- [ ] **Step 3: Add extra_paths field to SkillsConfig**

In `src/config/schema.rs`, modify the `SkillsConfig` struct:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct SkillsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default)]
    pub allow_scripts: bool,

    #[serde(default)]
    pub skills_dir: Option<String>,

    /// Additional directories to scan for skills.
    /// `~/.agents/skills` is always scanned automatically (hardcoded default).
    /// Use this for custom paths beyond the defaults.
    #[serde(default)]
    pub extra_paths: Vec<String>,
}
```

Update the `Default` impl:

```rust
impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_scripts: false,
            skills_dir: None,
            extra_paths: Vec::new(),
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib config::schema::tests 2>&1 | tail -10`
Expected: All tests pass (including existing ones — `extra_paths` defaults to empty vec so existing deserialization tests are unaffected)

- [ ] **Step 5: Commit**

```bash
git add src/config/schema.rs
git commit -m "feat(config): add extra_paths to SkillsConfig"
```

---

### Task 3: Multi-directory skill loading with dedup

**Files:**
- Modify: `src/skills/mod.rs`

- [ ] **Step 1: Write the failing test**

Add a new test module at the bottom of `src/skills/mod.rs`:

```rust
#[cfg(test)]
mod multi_dir_tests {
    use super::*;
    use std::fs;

    fn write_skill_md(dir: &Path, name: &str, desc: &str) {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {desc}\n---\n# {name}\n"),
        )
        .unwrap();
    }

    #[test]
    fn load_skills_multi_merges_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let primary = tmp.path().join("primary");
        let secondary = tmp.path().join("secondary");

        write_skill_md(&primary, "alpha", "Primary alpha");
        write_skill_md(&secondary, "beta", "Secondary beta");

        let skills = load_skills_multi(&[&primary, &secondary], false);
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
    }

    #[test]
    fn load_skills_multi_primary_wins_on_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let primary = tmp.path().join("primary");
        let secondary = tmp.path().join("secondary");

        write_skill_md(&primary, "clash", "I am primary");
        write_skill_md(&secondary, "clash", "I am secondary");

        let skills = load_skills_multi(&[&primary, &secondary], false);
        let clash = skills.iter().find(|s| s.name == "clash").unwrap();
        assert_eq!(clash.description, "I am primary");
    }

    #[test]
    fn load_skills_multi_skips_nonexistent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let primary = tmp.path().join("primary");
        let ghost = tmp.path().join("does-not-exist");

        write_skill_md(&primary, "solo", "Only one");

        let skills = load_skills_multi(&[&primary, &ghost], false);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "solo");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib skills::multi_dir_tests 2>&1 | tail -5`
Expected: FAIL — `load_skills_multi` does not exist yet

- [ ] **Step 3: Implement load_skills_multi**

Add this function in `src/skills/mod.rs`, after the existing `load_skills_from_directory` function:

```rust
/// Load skills from multiple directories with priority-based dedup.
/// Earlier directories have higher priority — if two directories contain
/// a skill with the same name, the one from the earlier directory wins.
pub fn load_skills_multi(dirs: &[&Path], allow_scripts: bool) -> Vec<Skill> {
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut all_skills: Vec<Skill> = Vec::new();

    for dir in dirs {
        let dir_skills = load_skills_from_directory(dir, allow_scripts);
        for skill in dir_skills {
            if seen_names.insert(skill.name.clone()) {
                all_skills.push(skill);
            } else {
                tracing::debug!(
                    "skipping duplicate skill '{}' from {}",
                    skill.name,
                    dir.display()
                );
            }
        }
    }

    all_skills
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib skills::multi_dir_tests 2>&1 | tail -10`
Expected: All 3 tests pass

- [ ] **Step 5: Update load_skills to use load_skills_multi**

Replace the existing `load_skills` function body:

```rust
/// Public entry point: load all skills from configured directories.
/// Scans in priority order:
/// 1. Primary skills dir (~/.config/nano-assistant/skills/ or custom)
/// 2. ~/.agents/skills/ (hardcoded, for skills.sh compatibility)
/// 3. Any extra_paths from config
pub fn load_skills(config: &crate::config::SkillsConfig) -> Vec<Skill> {
    let primary = match &config.skills_dir {
        Some(dir) => PathBuf::from(dir),
        None => skills_dir(),
    };

    let agents_skills = agents_skills_dir();

    let mut dirs: Vec<PathBuf> = vec![primary];

    // Hardcoded: always include ~/.agents/skills/ if it exists
    if agents_skills.exists() {
        dirs.push(agents_skills);
    }

    // User-configured extra paths
    for extra in &config.extra_paths {
        let expanded = expand_tilde(extra);
        dirs.push(expanded);
    }

    let dir_refs: Vec<&Path> = dirs.iter().map(|p| p.as_path()).collect();
    load_skills_multi(&dir_refs, config.allow_scripts)
}

/// Get the ~/.agents/skills/ directory path (skills.sh default install location).
fn agents_skills_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        return home.join(".agents").join("skills");
    }
    PathBuf::from("~/.agents/skills")
}

/// Expand leading `~` to $HOME.
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}
```

- [ ] **Step 6: Run all skills tests**

Run: `cargo test --lib skills 2>&1 | tail -15`
Expected: All tests pass (existing + new)

- [ ] **Step 7: Commit**

```bash
git add src/skills/mod.rs
git commit -m "feat(skills): multi-directory loading with ~/.agents/skills/ support"
```

---

### Task 4: Implement web_fetch tool

**Files:**
- Create: `src/tools/web_fetch.rs`

- [ ] **Step 1: Create web_fetch.rs with tests**

Create `src/tools/web_fetch.rs`:

```rust
use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_REDIRECTS: usize = 5;
const MAX_OUTPUT_BYTES: usize = 1_048_576; // 1 MiB
const DEFAULT_MAX_LENGTH: usize = 102_400; // 100 KB

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
            .user_agent(format!(
                "nano-assistant/{}",
                env!("CARGO_PKG_VERSION")
            ))
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
                error: Some(format!(
                    "Binary content type not supported: {content_type}"
                )),
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
            html2text::from_read(body.as_bytes(), 80)
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
```

- [ ] **Step 2: Register the module in tools/mod.rs**

In `src/tools/mod.rs`, add the module declaration after `pub mod content_search;`:

```rust
pub mod web_fetch;
```

Do NOT add it to `default_tools()` yet — we'll do that in Task 6 after both tools are ready.

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib tools::web_fetch::tests 2>&1 | tail -10`
Expected: All 6 tests pass

- [ ] **Step 4: Commit**

```bash
git add src/tools/web_fetch.rs src/tools/mod.rs
git commit -m "feat(tools): add web_fetch tool with HTML-to-text conversion"
```

---

### Task 5: Implement web_search tool

**Files:**
- Create: `src/tools/web_search.rs`

- [ ] **Step 1: Create web_search.rs with tests**

Create `src/tools/web_search.rs`:

```rust
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
            .user_agent(format!(
                "nano-assistant/{}",
                env!("CARGO_PKG_VERSION")
            ))
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

    // Extract result blocks by finding result__a links
    let mut search_pos = 0;
    while results.len() < max_results {
        // Find next result link
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
            // DDG wraps URLs through a redirect; extract the actual URL
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

/// Extract href="..." from a tag fragment.
fn extract_href(fragment: &str) -> Option<String> {
    let href_pos = fragment.find("href=\"")?;
    let start = href_pos + 6;
    let end = fragment[start..].find('"')? + start;
    Some(fragment[start..end].to_string())
}

/// Extract text content between > and </a>.
fn extract_tag_text(fragment: &str) -> Option<String> {
    let start = fragment.find('>')? + 1;
    let end = fragment[start..].find("</a>")? + start;
    let raw = &fragment[start..end];
    // Strip nested tags
    Some(strip_html_tags(raw).trim().to_string())
}

/// Extract snippet text from the result__snippet region.
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
/// DDG wraps URLs like: //duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com&...
fn clean_ddg_url(url: &str) -> String {
    if let Some(uddg_pos) = url.find("uddg=") {
        let encoded = &url[uddg_pos + 5..];
        let end = encoded.find('&').unwrap_or(encoded.len());
        let encoded_url = &encoded[..end];
        return url_decode(encoded_url);
    }
    url.to_string()
}

/// Simple URL decoding for %XX sequences.
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

/// Strip HTML tags from a string.
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

/// Decode common HTML entities.
fn html_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Format search results as readable markdown text.
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
        assert_eq!(url_decode("https%3A%2F%2Fexample.com"), "https://example.com");
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
```

- [ ] **Step 2: Register the module in tools/mod.rs**

In `src/tools/mod.rs`, add after the `web_fetch` module declaration:

```rust
pub mod web_search;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib tools::web_search::tests 2>&1 | tail -15`
Expected: All 10 tests pass

- [ ] **Step 4: Commit**

```bash
git add src/tools/web_search.rs src/tools/mod.rs
git commit -m "feat(tools): add web_search tool with DuckDuckGo integration"
```

---

### Task 6: Register web tools in default_tools()

**Files:**
- Modify: `src/tools/mod.rs`

- [ ] **Step 1: Update default_tools to include web tools**

Replace the `default_tools()` function in `src/tools/mod.rs`:

```rust
/// Returns the 8 core tools every agent gets by default.
pub fn default_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(shell::ShellTool::new()),
        Box::new(file_read::FileReadTool::new()),
        Box::new(file_write::FileWriteTool::new()),
        Box::new(file_edit::FileEditTool::new()),
        Box::new(glob_search::GlobSearchTool::new()),
        Box::new(content_search::ContentSearchTool::new()),
        Box::new(web_fetch::WebFetchTool::new()),
        Box::new(web_search::WebSearchTool::new()),
    ]
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check 2>&1 | tail -5`
Expected: `Finished` with no errors

- [ ] **Step 3: Run the full test suite**

Run: `cargo test 2>&1 | tail -15`
Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
git add src/tools/mod.rs
git commit -m "feat(tools): register web_fetch and web_search as default tools"
```

---

### Task 7: Integration smoke test

**Files:**
- No file changes — manual verification

- [ ] **Step 1: Verify skill path expansion works**

Run:
```bash
cargo run -- --help 2>&1 | head -5
```
Expected: Binary compiles and runs (shows help)

- [ ] **Step 2: Check that ~/.agents/skills/ would be scanned**

Run:
```bash
ls ~/.agents/skills/ 2>/dev/null | head -5
```
Expected: Lists skills like `find-skills`, `agent-browser`, etc. (if previously installed)

- [ ] **Step 3: Run the full test suite one final time**

Run: `cargo test 2>&1 | tail -20`
Expected: All tests pass

- [ ] **Step 4: Final commit (if any fixups needed)**

If any fixes were needed during smoke testing, commit them:
```bash
git add -u
git commit -m "fix: layer 1 smoke test fixups"
```

---

## Summary

| Task | What | Files Changed |
|------|------|---------------|
| 1 | Add `html2text` dep | `Cargo.toml` |
| 2 | `extra_paths` in config | `src/config/schema.rs` |
| 3 | Multi-dir skill loading | `src/skills/mod.rs` |
| 4 | `web_fetch` tool | `src/tools/web_fetch.rs`, `src/tools/mod.rs` |
| 5 | `web_search` tool | `src/tools/web_search.rs`, `src/tools/mod.rs` |
| 6 | Register tools | `src/tools/mod.rs` |
| 7 | Integration smoke test | (none) |

After Layer 1 is complete, Layer 2 (MCP Client) and Layer 3 (Domain Skills) plans will be written separately.

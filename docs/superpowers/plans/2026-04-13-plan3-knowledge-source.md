# Knowledge Source System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Knowledge Source skill type that connects nano-assistant to external wikis (ArchWiki, Debian Wiki, etc.) via engine adapters, automatically registering `search` and `read` tools per source.

**Architecture:** A new `src/knowledge/` module defines a `KnowledgeSource` trait with `search` and `read` methods. Engine adapters (`MediaWikiAdapter`, `MoinMoinAdapter`, `WebAdapter`) implement the trait. Knowledge source skills use `type = "knowledge-source"` in their TOML/markdown frontmatter and include a `[source]` section specifying engine + base URL. The skill loader detects this type and creates adapter instances that register as callable tools. Also enhances `web_fetch` to output Markdown instead of plain text.

**Tech Stack:** Rust, `reqwest`, `scraper` crate (HTML parsing), existing `html2text`

**Depends on:** Plan 2 (Skill Versioning) — builtin skill embedding for wiki skills

---

### Task 1: Enhance web_fetch HTML → Markdown conversion

**Files:**
- Modify: `Cargo.toml` (add `scraper` dependency)
- Modify: `src/tools/web_fetch.rs`

- [ ] **Step 1: Add scraper dependency**

In `Cargo.toml`, add under `[dependencies]`:

```toml
scraper = "0.22"
```

Run: `cargo check` to verify dependency resolves.

- [ ] **Step 2: Add html_to_markdown function in web_fetch.rs**

Add a new function that converts HTML to Markdown preserving structure. Place it before the `WebFetchTool` impl:

```rust
/// Convert HTML to Markdown, preserving code blocks, tables, and headings.
/// Falls back to html2text if parsing fails.
fn html_to_markdown(html: &str) -> String {
    use scraper::{Html, Selector};

    let document = Html::parse_document(html);

    // Try to extract main content area (common wiki patterns)
    let main_selectors = [
        "#mw-content-text",  // MediaWiki
        "#content",          // Generic
        "article",           // Semantic HTML
        "main",              // Semantic HTML
        ".moin-content",     // MoinMoin
    ];

    let content_html = main_selectors
        .iter()
        .find_map(|sel| {
            Selector::parse(sel).ok().and_then(|s| {
                document.select(&s).next().map(|el| el.inner_html())
            })
        })
        .unwrap_or_else(|| {
            // Fallback: use body or full document
            Selector::parse("body")
                .ok()
                .and_then(|s| document.select(&s).next().map(|el| el.inner_html()))
                .unwrap_or_else(|| html.to_string())
        });

    // Use html2text on the extracted content for now.
    // This preserves the content extraction benefit while keeping conversion simple.
    // TODO: Replace with a proper HTML→Markdown converter if html2text quality is insufficient.
    html2text::from_read(content_html.as_bytes(), 120)
        .unwrap_or_else(|_| content_html)
}
```

- [ ] **Step 3: Add optional CSS selector parameter to web_fetch**

In `WebFetchTool::parameters_schema()`, add a `selector` parameter:

```json
"selector": {
    "type": "string",
    "description": "Optional CSS selector to extract only a specific part of the page"
}
```

- [ ] **Step 4: Update execute to use html_to_markdown and selector**

In the `execute` method, replace the `html2text::from_read` call with:

```rust
let mut output = if is_html(&content_type) {
    // Apply CSS selector if provided
    if let Some(selector_str) = args.get("selector").and_then(|v| v.as_str()) {
        use scraper::{Html, Selector};
        let document = Html::parse_document(&body);
        if let Ok(selector) = Selector::parse(selector_str) {
            let extracted: Vec<String> = document
                .select(&selector)
                .map(|el| el.inner_html())
                .collect();
            if extracted.is_empty() {
                format!("No elements matched selector: {}", selector_str)
            } else {
                let combined = extracted.join("\n\n");
                html2text::from_read(combined.as_bytes(), 120)
                    .unwrap_or(combined)
            }
        } else {
            html_to_markdown(&body)
        }
    } else {
        html_to_markdown(&body)
    }
} else {
    body
};
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check 2>&1 | head -20`
Expected: compiles

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml src/tools/web_fetch.rs
git commit -m "feat(web_fetch): HTML→Markdown conversion with CSS selector support"
```

---

### Task 2: Create KnowledgeSource trait and types

**Files:**
- Create: `src/knowledge/mod.rs`
- Create: `src/knowledge/types.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Add knowledge module to lib.rs**

```rust
pub mod knowledge;
```

- [ ] **Step 2: Create src/knowledge/types.rs**

```rust
/// Result from a knowledge source search.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub title: String,
    pub snippet: String,
    pub page_id: String,
    pub url: String,
}

/// Content from a knowledge source page.
#[derive(Debug, Clone)]
pub struct PageContent {
    pub title: String,
    pub content: String,
    pub sections: Vec<String>,
    pub url: String,
}

/// Configuration for a knowledge source, parsed from skill TOML/frontmatter.
#[derive(Debug, Clone)]
pub struct KnowledgeSourceConfig {
    pub name: String,
    pub engine: String,
    pub base_url: String,
    pub language: Option<String>,
    pub triggers: Vec<String>,
    pub priority: u32,
}
```

- [ ] **Step 3: Create src/knowledge/mod.rs**

```rust
pub mod types;
mod mediawiki;
mod moinmoin;
mod web;
mod tool;

pub use types::{KnowledgeSourceConfig, PageContent, SearchResult};

use anyhow::Result;
use async_trait::async_trait;

/// A queryable external knowledge source.
#[async_trait]
pub trait KnowledgeSource: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    async fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchResult>>;
    async fn read(&self, page_id: &str, section: Option<&str>) -> Result<PageContent>;
}

/// Create a knowledge source from config, dispatching to the correct engine adapter.
pub fn create_source(config: KnowledgeSourceConfig) -> Result<Box<dyn KnowledgeSource>> {
    match config.engine.as_str() {
        "mediawiki" => Ok(Box::new(mediawiki::MediaWikiSource::new(config))),
        "moinmoin" => Ok(Box::new(moinmoin::MoinMoinSource::new(config))),
        "web" => Ok(Box::new(web::WebSource::new(config))),
        other => anyhow::bail!("Unknown knowledge source engine: {other}"),
    }
}

/// Convert a knowledge source into two Tool implementations (search + read).
pub fn source_to_tools(source: Box<dyn KnowledgeSource>) -> Vec<Box<dyn crate::tools::Tool>> {
    let source = std::sync::Arc::new(source);
    vec![
        Box::new(tool::KnowledgeSearchTool::new(std::sync::Arc::clone(&source))),
        Box::new(tool::KnowledgeReadTool::new(source)),
    ]
}
```

- [ ] **Step 4: Verify it compiles (will fail — adapter modules don't exist yet)**

Run: `cargo check 2>&1 | head -5`
Expected: errors about missing modules. This is expected — we'll create them next.

- [ ] **Step 5: Commit (types and trait only)**

```bash
git add src/knowledge/mod.rs src/knowledge/types.rs src/lib.rs
git commit -m "feat(knowledge): add KnowledgeSource trait and types"
```

---

### Task 3: Implement MediaWiki adapter

**Files:**
- Create: `src/knowledge/mediawiki.rs`

- [ ] **Step 1: Create MediaWiki adapter**

```rust
use super::types::{KnowledgeSourceConfig, PageContent, SearchResult};
use super::KnowledgeSource;
use anyhow::Result;
use async_trait::async_trait;

pub struct MediaWikiSource {
    config: KnowledgeSourceConfig,
    client: reqwest::Client,
    api_url: String,
}

impl MediaWikiSource {
    pub fn new(config: KnowledgeSourceConfig) -> Self {
        let api_url = format!("{}/api.php", config.base_url.trim_end_matches('/'));
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(format!("nano-assistant/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { config, client, api_url }
    }
}

#[async_trait]
impl KnowledgeSource for MediaWikiSource {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn description(&self) -> &str {
        "MediaWiki knowledge source"
    }

    async fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchResult>> {
        let resp = self.client
            .get(&self.api_url)
            .query(&[
                ("action", "query"),
                ("list", "search"),
                ("srsearch", query),
                ("srlimit", &limit.to_string()),
                ("format", "json"),
            ])
            .send()
            .await?;

        let json: serde_json::Value = resp.json().await?;
        let results = json["query"]["search"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|item| {
                        let title = item["title"].as_str().unwrap_or_default();
                        let page_id = item["pageid"].as_u64().unwrap_or_default().to_string();
                        let snippet = item["snippet"].as_str().unwrap_or_default();
                        // Strip HTML tags from snippet
                        let clean_snippet = snippet
                            .replace("<span class=\"searchmatch\">", "")
                            .replace("</span>", "");
                        let url = format!(
                            "{}/wiki/{}",
                            self.config.base_url.trim_end_matches('/'),
                            title.replace(' ', "_")
                        );
                        SearchResult {
                            title: title.to_string(),
                            snippet: clean_snippet,
                            page_id,
                            url,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(results)
    }

    async fn read(&self, page_id: &str, section: Option<&str>) -> Result<PageContent> {
        let mut params = vec![
            ("action", "parse"),
            ("format", "json"),
            ("prop", "wikitext|sections"),
        ];

        // page_id can be numeric ID or page title
        if page_id.parse::<u64>().is_ok() {
            params.push(("pageid", page_id));
        } else {
            params.push(("page", page_id));
        }

        if let Some(sec) = section {
            params.push(("section", sec));
        }

        let resp = self.client
            .get(&self.api_url)
            .query(&params)
            .send()
            .await?;

        let json: serde_json::Value = resp.json().await?;
        let parse = &json["parse"];

        let title = parse["title"].as_str().unwrap_or_default().to_string();
        let wikitext = parse["wikitext"]["*"].as_str().unwrap_or_default();

        let sections: Vec<String> = parse["sections"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s["line"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let url = format!(
            "{}/wiki/{}",
            self.config.base_url.trim_end_matches('/'),
            title.replace(' ', "_")
        );

        // Simple wikitext → plain text conversion (strip markup)
        let content = simple_wikitext_to_text(wikitext);

        Ok(PageContent {
            title,
            content,
            sections,
            url,
        })
    }
}

/// Basic wikitext → readable text conversion.
/// Handles common patterns: headings, links, templates, bold/italic.
fn simple_wikitext_to_text(wikitext: &str) -> String {
    let mut result = String::with_capacity(wikitext.len());

    for line in wikitext.lines() {
        let trimmed = line.trim();

        // Convert headings: == Title == → ## Title
        if trimmed.starts_with("==") {
            let level = trimmed.chars().take_while(|c| *c == '=').count();
            let title = trimmed
                .trim_start_matches('=')
                .trim_end_matches('=')
                .trim();
            let md_prefix = "#".repeat(level.min(6));
            result.push_str(&format!("{} {}\n", md_prefix, title));
            continue;
        }

        // Strip [[link|display]] → display, [[link]] → link
        let mut processed = line.to_string();
        while let Some(start) = processed.find("[[") {
            if let Some(end) = processed[start..].find("]]") {
                let inner = &processed[start + 2..start + end];
                let display = inner.split('|').last().unwrap_or(inner);
                processed = format!(
                    "{}{}{}",
                    &processed[..start],
                    display,
                    &processed[start + end + 2..]
                );
            } else {
                break;
            }
        }

        // Strip '''bold''' → bold, ''italic'' → italic
        processed = processed.replace("'''", "**");
        processed = processed.replace("''", "*");

        // Skip template lines ({{...}})
        if trimmed.starts_with("{{") && trimmed.ends_with("}}") {
            continue;
        }

        result.push_str(&processed);
        result.push('\n');
    }

    result
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check 2>&1 | head -10`
Expected: may still fail due to missing moinmoin/web/tool modules.

- [ ] **Step 3: Commit**

```bash
git add src/knowledge/mediawiki.rs
git commit -m "feat(knowledge): implement MediaWiki adapter with search and read"
```

---

### Task 4: Implement MoinMoin and Web adapters

**Files:**
- Create: `src/knowledge/moinmoin.rs`
- Create: `src/knowledge/web.rs`

- [ ] **Step 1: Create MoinMoin adapter**

```rust
use super::types::{KnowledgeSourceConfig, PageContent, SearchResult};
use super::KnowledgeSource;
use anyhow::Result;
use async_trait::async_trait;

pub struct MoinMoinSource {
    config: KnowledgeSourceConfig,
    client: reqwest::Client,
}

impl MoinMoinSource {
    pub fn new(config: KnowledgeSourceConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(format!("nano-assistant/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { config, client }
    }
}

#[async_trait]
impl KnowledgeSource for MoinMoinSource {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn description(&self) -> &str {
        "MoinMoin wiki knowledge source"
    }

    async fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchResult>> {
        // MoinMoin fullsearch via HTTP, parse HTML results
        let url = format!(
            "{}/?action=fullsearch&value={}&titlesearch=on",
            self.config.base_url.trim_end_matches('/'),
            urlencoding::encode(query)
        );

        let resp = self.client.get(&url).send().await?;
        let html = resp.text().await?;

        // Parse search results from HTML using scraper
        use scraper::{Html, Selector};
        let document = Html::parse_document(&html);
        let link_selector = Selector::parse(".searchresults a, #content a[href]")
            .unwrap_or_else(|_| Selector::parse("a").unwrap());

        let mut results = Vec::new();
        for element in document.select(&link_selector) {
            if results.len() >= limit as usize {
                break;
            }
            if let Some(href) = element.value().attr("href") {
                let title = element.text().collect::<String>();
                if title.trim().is_empty() || href.contains("action=") {
                    continue;
                }
                let full_url = if href.starts_with("http") {
                    href.to_string()
                } else {
                    format!("{}{}", self.config.base_url.trim_end_matches('/'), href)
                };
                results.push(SearchResult {
                    title: title.trim().to_string(),
                    snippet: String::new(),
                    page_id: title.trim().to_string(),
                    url: full_url,
                });
            }
        }

        Ok(results)
    }

    async fn read(&self, page_id: &str, _section: Option<&str>) -> Result<PageContent> {
        // Fetch raw content via ?action=raw
        let url = format!(
            "{}/{}?action=raw",
            self.config.base_url.trim_end_matches('/'),
            page_id.replace(' ', "%20")
        );

        let resp = self.client.get(&url).send().await?;
        let content = resp.text().await?;

        let page_url = format!(
            "{}/{}",
            self.config.base_url.trim_end_matches('/'),
            page_id.replace(' ', "%20")
        );

        Ok(PageContent {
            title: page_id.to_string(),
            content,
            sections: Vec::new(),
            url: page_url,
        })
    }
}
```

- [ ] **Step 2: Create Web adapter (fallback)**

```rust
use super::types::{KnowledgeSourceConfig, PageContent, SearchResult};
use super::KnowledgeSource;
use anyhow::Result;
use async_trait::async_trait;

pub struct WebSource {
    config: KnowledgeSourceConfig,
    client: reqwest::Client,
}

impl WebSource {
    pub fn new(config: KnowledgeSourceConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(format!("nano-assistant/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { config, client }
    }
}

#[async_trait]
impl KnowledgeSource for WebSource {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn description(&self) -> &str {
        "Web-based knowledge source"
    }

    async fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchResult>> {
        // Use DuckDuckGo site-scoped search (same approach as web_search tool)
        let site_query = format!("site:{} {}", self.config.base_url, query);
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(&site_query)
        );

        let resp = self.client.get(&url).send().await?;
        let html = resp.text().await?;

        use scraper::{Html, Selector};
        let document = Html::parse_document(&html);
        let result_selector = Selector::parse(".result__a")
            .unwrap_or_else(|_| Selector::parse("a").unwrap());
        let snippet_selector = Selector::parse(".result__snippet")
            .unwrap_or_else(|_| Selector::parse("span").unwrap());

        let links: Vec<_> = document.select(&result_selector).collect();
        let snippets: Vec<_> = document.select(&snippet_selector).collect();

        let mut results = Vec::new();
        for (i, link) in links.iter().enumerate() {
            if results.len() >= limit as usize {
                break;
            }
            let title = link.text().collect::<String>();
            let href = link.value().attr("href").unwrap_or_default();
            let snippet = snippets
                .get(i)
                .map(|s| s.text().collect::<String>())
                .unwrap_or_default();

            results.push(SearchResult {
                title: title.trim().to_string(),
                snippet: snippet.trim().to_string(),
                page_id: href.to_string(),
                url: href.to_string(),
            });
        }

        Ok(results)
    }

    async fn read(&self, page_id: &str, _section: Option<&str>) -> Result<PageContent> {
        let url = if page_id.starts_with("http") {
            page_id.to_string()
        } else {
            format!(
                "{}/{}",
                self.config.base_url.trim_end_matches('/'),
                page_id
            )
        };

        let resp = self.client.get(&url).send().await?;
        let html = resp.text().await?;
        let content = crate::tools::web_fetch::html_to_markdown(&html);

        Ok(PageContent {
            title: page_id.to_string(),
            content,
            sections: Vec::new(),
            url,
        })
    }
}
```

Note: `html_to_markdown` in web_fetch.rs needs to be made `pub` for the web adapter to use it.

- [ ] **Step 3: Make html_to_markdown public in web_fetch.rs**

Change `fn html_to_markdown` to `pub fn html_to_markdown`.

- [ ] **Step 4: Add urlencoding dependency to Cargo.toml**

```toml
urlencoding = "2"
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check 2>&1 | head -10`

- [ ] **Step 6: Commit**

```bash
git add src/knowledge/moinmoin.rs src/knowledge/web.rs src/tools/web_fetch.rs Cargo.toml
git commit -m "feat(knowledge): add MoinMoin and Web fallback adapters"
```

---

### Task 5: Create KnowledgeSearch and KnowledgeRead tool wrappers

**Files:**
- Create: `src/knowledge/tool.rs`

- [ ] **Step 1: Create tool wrappers**

```rust
use super::KnowledgeSource;
use crate::tools::{Tool, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Tool wrapper for knowledge source search.
pub struct KnowledgeSearchTool {
    source: Arc<Box<dyn KnowledgeSource>>,
    tool_name: String,
    tool_description: String,
}

impl KnowledgeSearchTool {
    pub fn new(source: Arc<Box<dyn KnowledgeSource>>) -> Self {
        let tool_name = format!("{}.search", source.name());
        let tool_description = format!(
            "Search {} for relevant pages. Returns titles, snippets, and page IDs.",
            source.name()
        );
        Self {
            source,
            tool_name,
            tool_description,
        }
    }
}

#[async_trait]
impl Tool for KnowledgeSearchTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.tool_description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum results to return (default: 5)",
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let query = args["query"].as_str().unwrap_or_default();
        let limit = args["limit"].as_u64().unwrap_or(5) as u32;

        match self.source.search(query, limit).await {
            Ok(results) => {
                if results.is_empty() {
                    return Ok(ToolResult {
                        success: true,
                        output: format!("No results found for '{query}'"),
                        error: None,
                    });
                }

                let mut output = String::new();
                for (i, r) in results.iter().enumerate() {
                    output.push_str(&format!(
                        "{}. **{}** (id: {})\n   {}\n   {}\n\n",
                        i + 1,
                        r.title,
                        r.page_id,
                        r.snippet,
                        r.url
                    ));
                }
                Ok(ToolResult {
                    success: true,
                    output,
                    error: None,
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Search failed: {e}")),
            }),
        }
    }
}

/// Tool wrapper for knowledge source page read.
pub struct KnowledgeReadTool {
    source: Arc<Box<dyn KnowledgeSource>>,
    tool_name: String,
    tool_description: String,
}

impl KnowledgeReadTool {
    pub fn new(source: Arc<Box<dyn KnowledgeSource>>) -> Self {
        let tool_name = format!("{}.read", source.name());
        let tool_description = format!(
            "Read a page from {}. Use page_id from search results. Optionally specify a section.",
            source.name()
        );
        Self {
            source,
            tool_name,
            tool_description,
        }
    }
}

#[async_trait]
impl Tool for KnowledgeReadTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.tool_description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "page_id": {
                    "type": "string",
                    "description": "Page ID or title from search results"
                },
                "section": {
                    "type": "string",
                    "description": "Optional section number or name to read"
                }
            },
            "required": ["page_id"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let page_id = args["page_id"].as_str().unwrap_or_default();
        let section = args["section"].as_str();

        match self.source.read(page_id, section).await {
            Ok(page) => {
                let mut output = format!("# {}\n\nURL: {}\n\n", page.title, page.url);
                if !page.sections.is_empty() {
                    output.push_str("**Sections:** ");
                    output.push_str(&page.sections.join(", "));
                    output.push_str("\n\n");
                }
                output.push_str(&page.content);

                // Truncate to avoid blowing up context
                const MAX_CHARS: usize = 50_000;
                if output.len() > MAX_CHARS {
                    // Truncate at paragraph boundary
                    if let Some(pos) = output[..MAX_CHARS].rfind("\n\n") {
                        output.truncate(pos);
                    } else {
                        output.truncate(MAX_CHARS);
                    }
                    output.push_str("\n\n[... content truncated]");
                }

                Ok(ToolResult {
                    success: true,
                    output,
                    error: None,
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Read failed: {e}")),
            }),
        }
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check 2>&1 | head -10`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add src/knowledge/tool.rs
git commit -m "feat(knowledge): add KnowledgeSearch and KnowledgeRead tool wrappers"
```

---

### Task 6: Parse knowledge-source skill type and integrate with skill loader

**Files:**
- Modify: `src/skills/mod.rs`
- Modify: `src/cli/commands.rs` (agent building)

- [ ] **Step 1: Add knowledge source parsing to skill loader**

In `src/skills/mod.rs`, add a function to detect and parse knowledge source skills:

```rust
/// Check if a skill is a knowledge-source type and extract its config.
pub fn parse_knowledge_source_config(skill: &Skill) -> Option<crate::knowledge::KnowledgeSourceConfig> {
    // Check for type = "knowledge-source" in skill metadata
    // This requires checking the raw TOML/frontmatter data.
    // For builtin knowledge sources, we embed the config directly.
    // For file-based skills, re-parse the file to check for [source] section.

    if let Some(location) = &skill.location {
        let toml_path = location.parent()?.join("SKILL.toml");
        if toml_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&toml_path) {
                if let Ok(value) = content.parse::<toml::Value>() {
                    let skill_type = value
                        .get("skill")
                        .and_then(|s| s.get("type"))
                        .and_then(|t| t.as_str());

                    if skill_type == Some("knowledge-source") {
                        if let Some(source) = value.get("source") {
                            return Some(crate::knowledge::KnowledgeSourceConfig {
                                name: skill.name.clone(),
                                engine: source.get("engine")
                                    .and_then(|e| e.as_str())
                                    .unwrap_or("web")
                                    .to_string(),
                                base_url: source.get("base_url")
                                    .and_then(|u| u.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                language: source.get("language")
                                    .and_then(|l| l.as_str())
                                    .map(String::from),
                                triggers: value.get("routing")
                                    .and_then(|r| r.get("triggers"))
                                    .and_then(|t| t.as_array())
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|v| v.as_str().map(String::from))
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                                priority: value.get("routing")
                                    .and_then(|r| r.get("priority"))
                                    .and_then(|p| p.as_integer())
                                    .unwrap_or(10) as u32,
                            });
                        }
                    }
                }
            }
        }
    }

    None
}
```

- [ ] **Step 2: Integrate knowledge source tools into agent building**

In `src/cli/commands.rs`, in the `build_agent` or `run_chat` function where skills are converted to tools, add knowledge source detection:

```rust
// After loading skills and converting to skill tools...
// Check for knowledge source skills and create their tools
for skill in &skills {
    if let Some(ks_config) = crate::skills::parse_knowledge_source_config(skill) {
        match crate::knowledge::create_source(ks_config) {
            Ok(source) => {
                let ks_tools = crate::knowledge::source_to_tools(source);
                for tool in ks_tools {
                    tools.push(tool);
                }
            }
            Err(e) => {
                tracing::warn!("Failed to create knowledge source '{}': {e}", skill.name);
            }
        }
    }
}
```

- [ ] **Step 3: Add knowledge source hints to system prompt**

In `src/agent/prompt.rs`, in `SystemPromptBuilder::build`, add a section for knowledge sources after the skills section. The knowledge source tools are already in the tools list, but we add descriptive hints:

```rust
// After skills section
if !ctx.knowledge_source_hints.is_empty() {
    prompt.push_str("\n## Available Knowledge Sources\n");
    for hint in ctx.knowledge_source_hints {
        prompt.push_str(&format!("- {} (triggers: {})\n", hint.name, hint.triggers.join(", ")));
    }
    prompt.push('\n');
}
```

Update `PromptContext` to include `knowledge_source_hints`.

- [ ] **Step 4: Verify it compiles**

Run: `cargo check 2>&1 | head -20`

- [ ] **Step 5: Commit**

```bash
git add src/skills/mod.rs src/cli/commands.rs src/agent/prompt.rs
git commit -m "feat(knowledge): integrate knowledge sources into skill loader and agent"
```

---

### Task 7: Add builtin wiki knowledge source skills

**Files:**
- Create: `skills/arch-wiki/SKILL.toml`

- [ ] **Step 1: Create ArchWiki knowledge source skill**

```toml
[skill]
name = "arch-wiki"
type = "knowledge-source"
description = "ArchLinux official wiki — comprehensive Linux documentation"
version = "0.3.0"
tags = ["linux", "archlinux", "wiki"]

[source]
engine = "mediawiki"
base_url = "https://wiki.archlinux.org"
language = "en"

[routing]
triggers = ["arch", "archlinux", "pacman", "makepkg", "AUR", "PKGBUILD", "systemd", "mkinitcpio"]
priority = 10
```

- [ ] **Step 2: Add to BUILTIN_SKILLS in skills/mod.rs**

Add to the `BUILTIN_SKILLS` constant:

```rust
("arch-wiki", include_str!("../../skills/arch-wiki/SKILL.toml")),
```

Note: For TOML-based builtin skills, adjust `load_builtin_skills()` to detect `.toml` content and parse accordingly. Check if content starts with `[skill]` to choose TOML vs Markdown parsing.

- [ ] **Step 3: Update load_builtin_skills to handle both formats**

```rust
fn load_builtin_skills() -> Vec<Skill> {
    let mut skills = Vec::new();
    for (name, content) in BUILTIN_SKILLS {
        let skill = if content.trim_start().starts_with('[') {
            // TOML format
            parse_builtin_toml(name, content)
        } else {
            // Markdown format
            parse_builtin_md(name, content)
        };
        if let Some(mut skill) = skill {
            skill.version = NA_VERSION.to_string();
            skill.is_builtin = true;
            skill.source = Some(SkillSource::Builtin);
            skills.push(skill);
        }
    }
    skills
}

fn parse_builtin_toml(name: &str, content: &str) -> Option<Skill> {
    let manifest: SkillManifest = toml::from_str(content).ok()?;
    Some(Skill {
        name: manifest.skill.name,
        description: manifest.skill.description,
        version: String::new(), // Will be overwritten
        author: manifest.skill.author,
        tags: manifest.skill.tags,
        tools: manifest.tools,
        prompts: manifest.prompts,
        location: None,
        is_builtin: false, // Will be overwritten
        source: None,       // Will be overwritten
    })
}

fn parse_builtin_md(name: &str, content: &str) -> Option<Skill> {
    let parsed = parse_skill_markdown(content);
    Some(Skill {
        name: parsed.meta.name.unwrap_or_else(|| name.to_string()),
        description: parsed
            .meta
            .description
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| extract_description(&parsed.body)),
        version: String::new(),
        author: parsed.meta.author,
        tags: parsed.meta.tags,
        tools: Vec::new(),
        prompts: if parsed.body.trim().is_empty() {
            Vec::new()
        } else {
            vec![parsed.body]
        },
        location: None,
        is_builtin: false,
        source: None,
    })
}
```

- [ ] **Step 4: Handle builtin knowledge source config parsing**

For builtin skills, `location` is `None`, so `parse_knowledge_source_config` won't find a file. Store the raw TOML content alongside builtins so it can be re-parsed:

Add a field to Skill:

```rust
#[serde(skip)]
pub raw_content: Option<String>,
```

Set it in `load_builtin_skills`:

```rust
skill.raw_content = Some(content.to_string());
```

Update `parse_knowledge_source_config` to also check `raw_content`:

```rust
// At the start of parse_knowledge_source_config:
if let Some(ref raw) = skill.raw_content {
    if let Ok(value) = raw.parse::<toml::Value>() {
        // ... same parsing logic as file-based ...
    }
}
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check 2>&1 | head -20`

- [ ] **Step 6: Commit**

```bash
git add skills/arch-wiki/ src/skills/mod.rs
git commit -m "feat(knowledge): add builtin ArchWiki knowledge source"
```

---

### Task 8: End-to-end verification

**Files:** None (testing only)

- [ ] **Step 1: Build**

Run: `cargo build 2>&1 | tail -5`

- [ ] **Step 2: Skills list should show arch-wiki**

Run: `cargo run -- skills list`
Expected: arch-wiki appears as builtin

- [ ] **Step 3: Test ArchWiki search (if API key configured)**

Run interactively and ask: "Search ArchWiki for nginx installation"
Expected: nana calls `arch-wiki.search` tool, returns structured results.

- [ ] **Step 4: Test ArchWiki read**

Follow up with reading a result page and verify clean content is returned.

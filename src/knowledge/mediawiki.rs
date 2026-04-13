use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

use super::types::{KnowledgeSourceConfig, PageContent, SearchResult};
use super::KnowledgeSource;

/// MediaWiki API adapter (Wikipedia, ArchWiki, etc.).
pub struct MediaWikiSource {
    name: String,
    description: String,
    base_url: String,
    #[allow(dead_code)]
    language: String,
    client: Client,
}

impl MediaWikiSource {
    pub fn new(config: &KnowledgeSourceConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(format!("nano-assistant/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            name: config.name.clone(),
            description: format!("MediaWiki knowledge source: {}", config.base_url),
            base_url: config.base_url.trim_end_matches('/').to_string(),
            language: config.language.clone(),
            client,
        }
    }

    fn api_url(&self) -> String {
        format!("{}/api.php", self.base_url)
    }
}

// ─── MediaWiki API response types ────────────────────────────────────

#[derive(Debug, Deserialize)]
struct MwSearchResponse {
    query: Option<MwSearchQuery>,
}

#[derive(Debug, Deserialize)]
struct MwSearchQuery {
    search: Vec<MwSearchResult>,
}

#[derive(Debug, Deserialize)]
struct MwSearchResult {
    title: String,
    snippet: String,
    pageid: u64,
}

#[derive(Debug, Deserialize)]
struct MwParseResponse {
    parse: Option<MwParse>,
}

#[derive(Debug, Deserialize)]
struct MwParse {
    title: String,
    wikitext: Option<MwWikitext>,
    sections: Option<Vec<MwSection>>,
}

#[derive(Debug, Deserialize)]
struct MwWikitext {
    #[serde(rename = "*")]
    content: String,
}

#[derive(Debug, Deserialize)]
struct MwSection {
    line: String,
    index: String,
    #[allow(dead_code)]
    level: String,
}

// ─── Wikitext → plain text conversion ────────────────────────────────

/// Simple wikitext to readable text conversion.
/// Handles headings, links, bold/italic, and skips templates.
fn wikitext_to_text(wikitext: &str) -> String {
    let mut output = String::with_capacity(wikitext.len());
    let mut template_depth = 0u32;

    for line in wikitext.lines() {
        let trimmed = line.trim();

        // Skip empty lines in templates
        if template_depth > 0 {
            template_depth += trimmed.matches("{{").count() as u32;
            template_depth = template_depth.saturating_sub(trimmed.matches("}}").count() as u32);
            continue;
        }

        // Track template starts
        let opens = trimmed.matches("{{").count() as u32;
        let closes = trimmed.matches("}}").count() as u32;
        if opens > closes {
            template_depth = opens - closes;
            continue;
        }

        // Convert headings: ===Foo=== → ### Foo
        if trimmed.starts_with("==") && trimmed.ends_with("==") {
            let level = trimmed.chars().take_while(|c| *c == '=').count();
            let inner = trimmed.trim_start_matches('=').trim_end_matches('=').trim();
            let prefix = "#".repeat(level.min(6));
            output.push_str(&format!("{prefix} {inner}\n"));
            continue;
        }

        // Process inline markup
        let processed = convert_inline_markup(trimmed);
        output.push_str(&processed);
        output.push('\n');
    }

    output
}

/// Convert inline wikitext markup: [[links]], '''bold''', ''italic''.
fn convert_inline_markup(line: &str) -> String {
    let mut result = line.to_string();

    // Convert [[link|display]] → display, [[link]] → link
    while let Some(start) = result.find("[[") {
        if let Some(end) = result[start..].find("]]") {
            let end = start + end;
            let inner = &result[start + 2..end];
            let display = if let Some((_target, text)) = inner.split_once('|') {
                text.to_string()
            } else {
                inner.to_string()
            };
            result = format!("{}{}{}", &result[..start], display, &result[end + 2..]);
        } else {
            break;
        }
    }

    // Bold: '''text''' → text
    while let Some(start) = result.find("'''") {
        if let Some(end) = result[start + 3..].find("'''") {
            let end = start + 3 + end;
            let inner = &result[start + 3..end];
            result = format!("{}{}{}", &result[..start], inner, &result[end + 3..]);
        } else {
            break;
        }
    }

    // Italic: ''text'' → text
    while let Some(start) = result.find("''") {
        if let Some(end) = result[start + 2..].find("''") {
            let end = start + 2 + end;
            let inner = &result[start + 2..end];
            result = format!("{}{}{}", &result[..start], inner, &result[end + 2..]);
        } else {
            break;
        }
    }

    result
}

/// Extract a named section from wikitext given section metadata.
fn extract_section(wikitext: &str, section_index: &str, sections: &[MwSection]) -> String {
    // Find the target section and the next section at the same or higher level
    let target = sections.iter().find(|s| s.index == section_index);
    let target_level = target
        .map(|s| s.level.parse::<usize>().unwrap_or(2))
        .unwrap_or(2);

    let target_heading = target.map(|s| s.line.as_str()).unwrap_or("");

    // Find the start of the section by looking for its heading
    let heading_markers = "=".repeat(target_level);
    let heading_pattern = format!("{heading_markers} {target_heading} {heading_markers}");
    let heading_pattern_nospace = format!("{heading_markers}{target_heading}{heading_markers}");

    let start_pos = wikitext
        .find(&heading_pattern)
        .or_else(|| wikitext.find(&heading_pattern_nospace))
        .unwrap_or(0);

    let after_heading = if start_pos > 0 {
        // Skip past the heading line itself
        let rest = &wikitext[start_pos..];
        if let Some(nl) = rest.find('\n') {
            start_pos + nl + 1
        } else {
            start_pos
        }
    } else {
        0
    };

    // Find the next section at the same or higher level
    let remaining = &wikitext[after_heading..];
    let end_pos = remaining
        .lines()
        .enumerate()
        .find(|(_, line)| {
            let trimmed = line.trim();
            if trimmed.starts_with("==") && trimmed.ends_with("==") {
                let level = trimmed.chars().take_while(|c| *c == '=').count();
                level <= target_level
            } else {
                false
            }
        })
        .map(|(i, _)| {
            remaining
                .lines()
                .take(i)
                .map(|l| l.len() + 1)
                .sum::<usize>()
        })
        .unwrap_or(remaining.len());

    let section_text = &remaining[..end_pos];
    wikitext_to_text(section_text)
}

#[async_trait]
impl KnowledgeSource for MediaWikiSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<SearchResult>> {
        let url = self.api_url();
        let resp = self
            .client
            .get(&url)
            .query(&[
                ("action", "query"),
                ("list", "search"),
                ("srsearch", query),
                ("srlimit", &limit.to_string()),
                ("format", "json"),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("MediaWiki search failed: HTTP {}", resp.status());
        }

        let data: MwSearchResponse = resp.json().await?;

        let results = data
            .query
            .map(|q| {
                q.search
                    .into_iter()
                    .map(|r| {
                        // Strip HTML from snippet
                        let snippet = strip_html_tags(&r.snippet);
                        SearchResult {
                            title: r.title.clone(),
                            snippet,
                            page_id: r.pageid.to_string(),
                            url: format!("{}/index.php?curid={}", self.base_url, r.pageid),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(results)
    }

    async fn read(&self, page_id: &str, section: Option<&str>) -> anyhow::Result<PageContent> {
        let url = self.api_url();
        let mut query = vec![
            ("action", "parse"),
            ("prop", "wikitext|sections"),
            ("format", "json"),
        ];

        // page_id can be a numeric ID or a title
        if page_id.chars().all(|c| c.is_ascii_digit()) {
            query.push(("pageid", page_id));
        } else {
            query.push(("page", page_id));
        }

        let resp = self.client.get(&url).query(&query).send().await?;

        if !resp.status().is_success() {
            anyhow::bail!("MediaWiki parse failed: HTTP {}", resp.status());
        }

        let data: MwParseResponse = resp.json().await?;
        let parse = data
            .parse
            .ok_or_else(|| anyhow::anyhow!("No parse result returned"))?;

        let wikitext = parse.wikitext.map(|w| w.content).unwrap_or_default();

        let section_names: Vec<String> = parse
            .sections
            .as_ref()
            .map(|secs| secs.iter().map(|s| s.line.clone()).collect())
            .unwrap_or_default();

        let content = if let Some(section_name) = section {
            // Find section by name or index
            if let Some(sections) = &parse.sections {
                let section_index = sections
                    .iter()
                    .find(|s| s.line.eq_ignore_ascii_case(section_name) || s.index == section_name)
                    .map(|s| s.index.clone());

                if let Some(idx) = section_index {
                    extract_section(&wikitext, &idx, sections)
                } else {
                    wikitext_to_text(&wikitext)
                }
            } else {
                wikitext_to_text(&wikitext)
            }
        } else {
            wikitext_to_text(&wikitext)
        };

        Ok(PageContent {
            title: parse.title,
            content,
            sections: section_names,
            url: format!("{}/index.php?curid={}", self.base_url, page_id),
        })
    }
}

/// Strip HTML tags from a string (used for search snippets).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wikitext_headings() {
        let input = "== Introduction ==\nSome text.\n=== Subsection ===\nMore text.";
        let output = wikitext_to_text(input);
        assert!(output.contains("## Introduction"));
        assert!(output.contains("### Subsection"));
        assert!(output.contains("Some text."));
    }

    #[test]
    fn wikitext_links() {
        let text = convert_inline_markup("See [[Arch Linux]] and [[pacman|the package manager]].");
        assert!(text.contains("Arch Linux"));
        assert!(text.contains("the package manager"));
        assert!(!text.contains("[["));
        assert!(!text.contains("]]"));
    }

    #[test]
    fn wikitext_bold_italic() {
        let text = convert_inline_markup("This is '''bold''' and ''italic''.");
        assert_eq!(text, "This is bold and italic.");
    }

    #[test]
    fn wikitext_skips_templates() {
        let input = "Before.\n{{Infobox\n| key = value\n}}\nAfter.";
        let output = wikitext_to_text(input);
        assert!(output.contains("Before."));
        assert!(output.contains("After."));
        assert!(!output.contains("Infobox"));
    }

    #[test]
    fn strip_html_tags_basic() {
        let input = "Hello <b>world</b> and <span class=\"x\">foo</span>";
        assert_eq!(strip_html_tags(input), "Hello world and foo");
    }

    #[test]
    fn mediawiki_source_name() {
        let config = KnowledgeSourceConfig {
            name: "arch-wiki".to_string(),
            engine: "mediawiki".to_string(),
            base_url: "https://wiki.archlinux.org".to_string(),
            language: "en".to_string(),
            triggers: vec![],
            priority: 10,
        };
        let source = MediaWikiSource::new(&config);
        assert_eq!(source.name(), "arch-wiki");
        assert_eq!(source.api_url(), "https://wiki.archlinux.org/api.php");
    }
}

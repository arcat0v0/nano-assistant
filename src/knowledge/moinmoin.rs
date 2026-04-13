use async_trait::async_trait;
use reqwest::Client;
use scraper::{Html, Selector};
use std::time::Duration;

use super::types::{KnowledgeSourceConfig, PageContent, SearchResult};
use super::KnowledgeSource;

/// MoinMoin wiki adapter.
///
/// - search: `/?action=fullsearch&value=...` (parses HTML results)
/// - read: `/PageName?action=raw` (raw wiki markup)
pub struct MoinMoinSource {
    name: String,
    description: String,
    base_url: String,
    client: Client,
}

impl MoinMoinSource {
    pub fn new(config: &KnowledgeSourceConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(format!(
                "nano-assistant/{}",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            name: config.name.clone(),
            description: format!("MoinMoin wiki: {}", config.base_url),
            base_url: config.base_url.trim_end_matches('/').to_string(),
            client,
        }
    }
}

#[async_trait]
impl KnowledgeSource for MoinMoinSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<SearchResult>> {
        let encoded = urlencoding::encode(query);
        let url = format!(
            "{}/?action=fullsearch&value={}&titlesearch=0",
            self.base_url, encoded
        );

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("MoinMoin search failed: HTTP {}", resp.status());
        }

        let body = resp.text().await?;
        let document = Html::parse_document(&body);

        let mut results = Vec::new();

        // MoinMoin search results are typically in a list of links
        // Try multiple selectors for different MoinMoin themes
        let selectors = [
            "div.searchresults a",
            ".searchresults a",
            "#content a[href]",
        ];

        for sel_str in selectors {
            if let Ok(sel) = Selector::parse(sel_str) {
                for element in document.select(&sel) {
                    if results.len() >= limit {
                        break;
                    }

                    let title = element.text().collect::<String>().trim().to_string();
                    if title.is_empty() {
                        continue;
                    }

                    let href = element
                        .value()
                        .attr("href")
                        .unwrap_or_default()
                        .to_string();

                    let page_id = title.replace(' ', "/");
                    let page_url = if href.starts_with("http") {
                        href
                    } else {
                        format!("{}/{}", self.base_url, page_id)
                    };

                    results.push(SearchResult {
                        title: title.clone(),
                        snippet: String::new(),
                        page_id,
                        url: page_url,
                    });
                }
                if !results.is_empty() {
                    break;
                }
            }
        }

        Ok(results)
    }

    async fn read(&self, page_id: &str, section: Option<&str>) -> anyhow::Result<PageContent> {
        let encoded = urlencoding::encode(page_id);
        let url = format!("{}/{}?action=raw", self.base_url, encoded);

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("MoinMoin read failed: HTTP {}", resp.status());
        }

        let body = resp.text().await?;

        // Extract sections from MoinMoin markup (= Heading =, == Heading ==, etc.)
        let sections: Vec<String> = body
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                trimmed.starts_with('=') && trimmed.ends_with('=')
            })
            .map(|line| {
                line.trim()
                    .trim_matches('=')
                    .trim()
                    .to_string()
            })
            .collect();

        let content = if let Some(section_name) = section {
            extract_moinmoin_section(&body, section_name)
        } else {
            body.clone()
        };

        Ok(PageContent {
            title: page_id.replace('/', " "),
            content,
            sections,
            url: format!("{}/{}", self.base_url, encoded),
        })
    }
}

/// Extract a named section from MoinMoin wiki markup.
fn extract_moinmoin_section(content: &str, section_name: &str) -> String {
    let mut in_section = false;
    let mut section_level = 0usize;
    let mut result = String::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('=') && trimmed.ends_with('=') {
            let level = trimmed.chars().take_while(|c| *c == '=').count();
            let heading = trimmed.trim_matches('=').trim();

            if heading.eq_ignore_ascii_case(section_name) {
                in_section = true;
                section_level = level;
                continue;
            } else if in_section && level <= section_level {
                break;
            }
        }

        if in_section {
            result.push_str(line);
            result.push('\n');
        }
    }

    if result.is_empty() {
        content.to_string()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moinmoin_source_name() {
        let config = KnowledgeSourceConfig {
            name: "moin-wiki".to_string(),
            engine: "moinmoin".to_string(),
            base_url: "https://moin.example.com".to_string(),
            language: "en".to_string(),
            triggers: vec![],
            priority: 10,
        };
        let source = MoinMoinSource::new(&config);
        assert_eq!(source.name(), "moin-wiki");
    }

    #[test]
    fn extract_section_basic() {
        let content = "= Intro =\nIntro text\n== Details ==\nDetail text\n== Other ==\nOther text";
        let section = extract_moinmoin_section(content, "Details");
        assert!(section.contains("Detail text"));
        assert!(!section.contains("Other text"));
    }

    #[test]
    fn extract_section_not_found_returns_full() {
        let content = "= Intro =\nSome content";
        let section = extract_moinmoin_section(content, "Missing");
        assert_eq!(section, content);
    }
}

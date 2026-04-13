use async_trait::async_trait;
use reqwest::Client;
use scraper::{Html, Selector};
use std::time::Duration;

use super::types::{KnowledgeSourceConfig, PageContent, SearchResult};
use super::KnowledgeSource;
use crate::tools::web_fetch::html_to_markdown;

/// Generic web-based knowledge source (fallback adapter).
///
/// - search: DuckDuckGo site-scoped search
/// - read: HTTP fetch with html_to_markdown conversion
pub struct WebSource {
    name: String,
    description: String,
    base_url: String,
    client: Client,
}

impl WebSource {
    pub fn new(config: &KnowledgeSourceConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(format!("nano-assistant/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            name: config.name.clone(),
            description: format!("Web knowledge source: {}", config.base_url),
            base_url: config.base_url.trim_end_matches('/').to_string(),
            client,
        }
    }
}

#[async_trait]
impl KnowledgeSource for WebSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<SearchResult>> {
        // Use DuckDuckGo HTML search with site: scope
        let site_query = format!("site:{} {}", self.base_url, query);
        let encoded = urlencoding::encode(&site_query);
        let url = format!("https://html.duckduckgo.com/html/?q={encoded}");

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Web search failed: HTTP {}", resp.status());
        }

        let body = resp.text().await?;
        let document = Html::parse_document(&body);

        let mut results = Vec::new();

        // DuckDuckGo HTML results
        if let Ok(result_sel) = Selector::parse(".result") {
            let title_sel = Selector::parse(".result__title a").ok();
            let snippet_sel = Selector::parse(".result__snippet").ok();

            for result_elem in document.select(&result_sel) {
                if results.len() >= limit {
                    break;
                }

                let title = title_sel
                    .as_ref()
                    .and_then(|sel| result_elem.select(sel).next())
                    .map(|el| el.text().collect::<String>().trim().to_string())
                    .unwrap_or_default();

                if title.is_empty() {
                    continue;
                }

                let snippet = snippet_sel
                    .as_ref()
                    .and_then(|sel| result_elem.select(sel).next())
                    .map(|el| el.text().collect::<String>().trim().to_string())
                    .unwrap_or_default();

                let href = title_sel
                    .as_ref()
                    .and_then(|sel| result_elem.select(sel).next())
                    .and_then(|el| el.value().attr("href"))
                    .unwrap_or_default()
                    .to_string();

                let page_url = if href.starts_with("http") {
                    href.clone()
                } else {
                    format!("{}{}", self.base_url, href)
                };

                results.push(SearchResult {
                    title,
                    snippet,
                    page_id: page_url.clone(),
                    url: page_url,
                });
            }
        }

        Ok(results)
    }

    async fn read(&self, page_id: &str, _section: Option<&str>) -> anyhow::Result<PageContent> {
        // page_id is a URL for the web adapter
        let url = if page_id.starts_with("http") {
            page_id.to_string()
        } else {
            format!("{}/{}", self.base_url, page_id)
        };

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Web fetch failed: HTTP {}", resp.status());
        }

        let body = resp.text().await?;
        let content = html_to_markdown(&body);

        // Try to extract title from HTML
        let document = Html::parse_document(&body);
        let title = Selector::parse("title")
            .ok()
            .and_then(|sel| document.select(&sel).next())
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_else(|| page_id.to_string());

        Ok(PageContent {
            title,
            content,
            sections: Vec::new(),
            url,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_source_name() {
        let config = KnowledgeSourceConfig {
            name: "docs".to_string(),
            engine: "web".to_string(),
            base_url: "https://docs.example.com".to_string(),
            language: "en".to_string(),
            triggers: vec![],
            priority: 10,
        };
        let source = WebSource::new(&config);
        assert_eq!(source.name(), "docs");
        assert!(source.description().contains("docs.example.com"));
    }
}

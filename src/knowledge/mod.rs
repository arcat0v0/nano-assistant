pub mod mediawiki;
pub mod moinmoin;
pub mod tool;
pub mod types;
pub mod web;

use async_trait::async_trait;
use std::sync::Arc;

pub use types::{KnowledgeSourceConfig, PageContent, SearchResult};

/// A knowledge source provides search and read access to an external wiki
/// or documentation system.
#[async_trait]
pub trait KnowledgeSource: Send + Sync {
    /// Short name for this source (used as tool name prefix).
    fn name(&self) -> &str;

    /// Human-readable description of the source.
    fn description(&self) -> &str;

    /// Search the knowledge source for pages matching a query.
    async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<SearchResult>>;

    /// Read a specific page by its ID, optionally limited to a section.
    async fn read(&self, page_id: &str, section: Option<&str>) -> anyhow::Result<PageContent>;
}

/// Create a knowledge source from configuration.
pub fn create_source(config: &KnowledgeSourceConfig) -> Box<dyn KnowledgeSource> {
    match config.engine.as_str() {
        "mediawiki" => Box::new(mediawiki::MediaWikiSource::new(config)),
        "moinmoin" => Box::new(moinmoin::MoinMoinSource::new(config)),
        _ => Box::new(web::WebSource::new(config)),
    }
}

/// Wrap a knowledge source into Tool trait objects (search + read).
pub fn source_to_tools(source: Box<dyn KnowledgeSource>) -> Vec<Box<dyn crate::tools::Tool>> {
    let shared: Arc<Box<dyn KnowledgeSource>> = Arc::new(source);
    vec![
        Box::new(tool::KnowledgeSearchTool::new(Arc::clone(&shared))),
        Box::new(tool::KnowledgeReadTool::new(Arc::clone(&shared))),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_mediawiki_source() {
        let config = KnowledgeSourceConfig {
            name: "test-wiki".to_string(),
            engine: "mediawiki".to_string(),
            base_url: "https://wiki.example.com".to_string(),
            language: "en".to_string(),
            triggers: vec![],
            priority: 10,
        };
        let source = create_source(&config);
        assert_eq!(source.name(), "test-wiki");
    }

    #[test]
    fn create_moinmoin_source() {
        let config = KnowledgeSourceConfig {
            name: "moin-wiki".to_string(),
            engine: "moinmoin".to_string(),
            base_url: "https://moin.example.com".to_string(),
            language: "en".to_string(),
            triggers: vec![],
            priority: 10,
        };
        let source = create_source(&config);
        assert_eq!(source.name(), "moin-wiki");
    }

    #[test]
    fn create_web_source_for_unknown_engine() {
        let config = KnowledgeSourceConfig {
            name: "generic".to_string(),
            engine: "unknown-engine".to_string(),
            base_url: "https://docs.example.com".to_string(),
            language: "en".to_string(),
            triggers: vec![],
            priority: 10,
        };
        let source = create_source(&config);
        assert_eq!(source.name(), "generic");
    }

    #[test]
    fn source_to_tools_produces_two_tools() {
        let config = KnowledgeSourceConfig {
            name: "wiki".to_string(),
            engine: "mediawiki".to_string(),
            base_url: "https://wiki.example.com".to_string(),
            language: "en".to_string(),
            triggers: vec![],
            priority: 10,
        };
        let source = create_source(&config);
        let tools = source_to_tools(source);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name(), "wiki.search");
        assert_eq!(tools[1].name(), "wiki.read");
    }
}

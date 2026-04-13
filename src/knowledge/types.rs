use serde::{Deserialize, Serialize};

/// A single search result from a knowledge source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub snippet: String,
    pub page_id: String,
    pub url: String,
}

/// Full page content retrieved from a knowledge source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageContent {
    pub title: String,
    pub content: String,
    pub sections: Vec<String>,
    pub url: String,
}

/// Configuration for a knowledge source, parsed from skill TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeSourceConfig {
    pub name: String,
    pub engine: String,
    pub base_url: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub triggers: Vec<String>,
    #[serde(default = "default_priority")]
    pub priority: u32,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_priority() -> u32 {
    10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_result_roundtrip() {
        let result = SearchResult {
            title: "Test".to_string(),
            snippet: "A snippet".to_string(),
            page_id: "42".to_string(),
            url: "https://example.com".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: SearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.title, "Test");
        assert_eq!(parsed.page_id, "42");
    }

    #[test]
    fn page_content_roundtrip() {
        let content = PageContent {
            title: "Page".to_string(),
            content: "Body text".to_string(),
            sections: vec!["Intro".into(), "Details".into()],
            url: "https://example.com/page".to_string(),
        };
        let json = serde_json::to_string(&content).unwrap();
        let parsed: PageContent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.sections.len(), 2);
    }

    #[test]
    fn config_defaults() {
        let toml_str = r#"
            name = "test"
            engine = "mediawiki"
            base_url = "https://wiki.example.com"
        "#;
        let config: KnowledgeSourceConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.language, "en");
        assert_eq!(config.priority, 10);
        assert!(config.triggers.is_empty());
    }
}

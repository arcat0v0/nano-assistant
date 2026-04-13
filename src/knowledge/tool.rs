use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

use crate::tools::traits::{Tool, ToolResult};
use super::KnowledgeSource;

const MAX_READ_CHARS: usize = 50_000;

/// Tool wrapper for searching a knowledge source.
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
            source.description()
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
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 5)"
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

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(5);

        match self.source.search(query, limit).await {
            Ok(results) => {
                let output = serde_json::to_string_pretty(&results)
                    .unwrap_or_else(|_| "[]".to_string());
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

/// Tool wrapper for reading a page from a knowledge source.
pub struct KnowledgeReadTool {
    source: Arc<Box<dyn KnowledgeSource>>,
    tool_name: String,
    tool_description: String,
}

impl KnowledgeReadTool {
    pub fn new(source: Arc<Box<dyn KnowledgeSource>>) -> Self {
        let tool_name = format!("{}.read", source.name());
        let tool_description = format!(
            "Read a page from {}. Use page_id from search results. \
             Optionally specify a section name to read only that section.",
            source.description()
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
        json!({
            "type": "object",
            "properties": {
                "page_id": {
                    "type": "string",
                    "description": "The page identifier (from search results)"
                },
                "section": {
                    "type": "string",
                    "description": "Optional section name to read only that section"
                }
            },
            "required": ["page_id"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let page_id = args
            .get("page_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'page_id' parameter"))?;

        let section = args.get("section").and_then(|v| v.as_str());

        match self.source.read(page_id, section).await {
            Ok(page) => {
                let mut output = format!("# {}\n\nURL: {}\n\n", page.title, page.url);

                if !page.sections.is_empty() {
                    output.push_str("Sections: ");
                    output.push_str(&page.sections.join(", "));
                    output.push_str("\n\n");
                }

                output.push_str(&page.content);

                // Truncate at paragraph boundary if over limit
                truncate_at_paragraph(&mut output, MAX_READ_CHARS);

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

/// Truncate text at a paragraph boundary (double newline) if it exceeds the limit.
fn truncate_at_paragraph(text: &mut String, max_chars: usize) {
    if text.len() <= max_chars {
        return;
    }

    // Find the last paragraph boundary before the limit
    let search_region = &text[..max_chars];
    let boundary = search_region
        .rfind("\n\n")
        .unwrap_or_else(|| {
            // Fall back to last newline
            search_region.rfind('\n').unwrap_or(max_chars)
        });

    // Ensure we're at a char boundary
    let mut pos = boundary;
    while pos > 0 && !text.is_char_boundary(pos) {
        pos -= 1;
    }

    text.truncate(pos);
    text.push_str("\n\n... [content truncated]");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_text_unchanged() {
        let mut text = "Short text.".to_string();
        truncate_at_paragraph(&mut text, 100);
        assert_eq!(text, "Short text.");
    }

    #[test]
    fn truncate_at_paragraph_boundary() {
        let mut text = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.".to_string();
        truncate_at_paragraph(&mut text, 30);
        assert!(text.contains("First paragraph."));
        assert!(text.contains("[content truncated]"));
        assert!(!text.contains("Third paragraph."));
    }

    #[test]
    fn truncate_falls_back_to_newline() {
        let mut text = "Line one\nLine two\nLine three is quite long indeed".to_string();
        truncate_at_paragraph(&mut text, 20);
        assert!(text.contains("[content truncated]"));
    }

    #[test]
    fn search_tool_metadata() {
        let config = super::super::types::KnowledgeSourceConfig {
            name: "wiki".to_string(),
            engine: "mediawiki".to_string(),
            base_url: "https://wiki.example.com".to_string(),
            language: "en".to_string(),
            triggers: vec![],
            priority: 10,
        };
        let source = super::super::create_source(&config);
        let shared: Arc<Box<dyn KnowledgeSource>> = Arc::new(source);
        let tool = KnowledgeSearchTool::new(shared);
        assert_eq!(tool.name(), "wiki.search");
        assert!(tool.description().contains("Search"));
        let schema = tool.parameters_schema();
        assert_eq!(schema["required"], json!(["query"]));
    }

    #[test]
    fn read_tool_metadata() {
        let config = super::super::types::KnowledgeSourceConfig {
            name: "wiki".to_string(),
            engine: "mediawiki".to_string(),
            base_url: "https://wiki.example.com".to_string(),
            language: "en".to_string(),
            triggers: vec![],
            priority: 10,
        };
        let source = super::super::create_source(&config);
        let shared: Arc<Box<dyn KnowledgeSource>> = Arc::new(source);
        let tool = KnowledgeReadTool::new(shared);
        assert_eq!(tool.name(), "wiki.read");
        assert!(tool.description().contains("Read"));
        let schema = tool.parameters_schema();
        assert_eq!(schema["required"], json!(["page_id"]));
    }
}

use rig::tool::{DynamicTool, ToolExecutionError, ToolOutput};
use serde_json::json;
use std::sync::Arc;

use super::KnowledgeSource;
use crate::tools::provider_name::{dynamic_tool_name, ToolNamespace};

const MAX_READ_CHARS: usize = 50_000;

/// Tool wrapper for searching a knowledge source.
pub struct KnowledgeSearchTool {
    source: Arc<Box<dyn KnowledgeSource>>,
    tool_name: String,
    tool_description: String,
}

impl KnowledgeSearchTool {
    pub fn new(source: Arc<Box<dyn KnowledgeSource>>) -> Self {
        let tool_name = dynamic_tool_name(ToolNamespace::Knowledge, source.name(), "search");
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
    pub fn into_dynamic(self) -> DynamicTool {
        let source = self.source;
        DynamicTool::new(
            self.tool_name,
            self.tool_description,
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
            }),
            move |_context, args| {
                let source = Arc::clone(&source);
                Box::pin(async move {
                    let query = args.get("query").and_then(|v| v.as_str()).ok_or_else(|| {
                        ToolExecutionError::invalid_args("Missing 'query' parameter")
                    })?;
                    let limit = args
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize)
                        .unwrap_or(5);
                    let results = source
                        .search(query, limit)
                        .await
                        .map_err(|e| ToolExecutionError::other(format!("Search failed: {e}")))?;
                    let output =
                        serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_string());
                    Ok(ToolOutput::text(output))
                })
            },
        )
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
        let tool_name = dynamic_tool_name(ToolNamespace::Knowledge, source.name(), "read");
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
    pub fn into_dynamic(self) -> DynamicTool {
        let source = self.source;
        DynamicTool::new(
            self.tool_name,
            self.tool_description,
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
            }),
            move |_context, args| {
                let source = Arc::clone(&source);
                Box::pin(async move {
                    let page_id =
                        args.get("page_id")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| {
                                ToolExecutionError::invalid_args("Missing 'page_id' parameter")
                            })?;
                    let section = args.get("section").and_then(|v| v.as_str());
                    let page = source
                        .read(page_id, section)
                        .await
                        .map_err(|e| ToolExecutionError::other(format!("Read failed: {e}")))?;
                    let mut output = format!("# {}\n\nURL: {}\n\n", page.title, page.url);

                    if !page.sections.is_empty() {
                        output.push_str("Sections: ");
                        output.push_str(&page.sections.join(", "));
                        output.push_str("\n\n");
                    }

                    output.push_str(&page.content);
                    truncate_at_paragraph(&mut output, MAX_READ_CHARS);
                    Ok(ToolOutput::text(output))
                })
            },
        )
    }
}

/// Truncate text at a paragraph boundary (double newline) if it exceeds the limit.
fn truncate_at_paragraph(text: &mut String, max_chars: usize) {
    if text.len() <= max_chars {
        return;
    }

    // Find the last paragraph boundary before the limit
    let search_region = &text[..max_chars];
    let boundary = search_region.rfind("\n\n").unwrap_or_else(|| {
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

    struct ExampleSource;

    #[async_trait::async_trait]
    impl KnowledgeSource for ExampleSource {
        fn name(&self) -> &str {
            "wiki"
        }

        fn description(&self) -> &str {
            "example wiki"
        }

        async fn search(
            &self,
            query: &str,
            limit: usize,
        ) -> anyhow::Result<Vec<super::super::SearchResult>> {
            Ok(vec![super::super::SearchResult {
                title: format!("{query} ({limit})"),
                snippet: "Matched title".into(),
                page_id: "42".into(),
                url: "https://example.com/42".into(),
            }])
        }

        async fn read(
            &self,
            page_id: &str,
            section: Option<&str>,
        ) -> anyhow::Result<super::super::PageContent> {
            Ok(super::super::PageContent {
                title: format!("Page {page_id}"),
                content: section.unwrap_or("Full page").into(),
                sections: vec!["Overview".into()],
                url: "https://example.com/42".into(),
            })
        }
    }

    #[tokio::test]
    async fn knowledge_tools_return_source_results_through_rig() {
        use rig::tool::{ToolContext, ToolErrorKind, ToolSet};

        let tools = super::super::source_to_tools(Box::new(ExampleSource));
        let tools = ToolSet::from_dynamic_tools(tools);
        let mut context = ToolContext::new();
        let missing_query = tools
            .execute("knowledge__wiki__search", "{}", &mut context)
            .await;
        assert!(missing_query.is_error_kind(ToolErrorKind::InvalidArgs));
        let search = tools
            .execute(
                "knowledge__wiki__search",
                r#"{"query":"rust","limit":3}"#,
                &mut context,
            )
            .await;
        let results: Vec<super::super::SearchResult> =
            serde_json::from_str(search.output().as_text().unwrap()).unwrap();
        assert_eq!(results[0].title, "rust (3)");
        assert_eq!(results[0].page_id, "42");
        let read = tools
            .execute(
                "knowledge__wiki__read",
                r#"{"page_id":"42","section":"Overview"}"#,
                &mut context,
            )
            .await;
        assert!(read.is_success());
        assert!(read
            .output()
            .as_text()
            .unwrap()
            .contains("# Page 42\n\nURL: https://example.com/42"));
        assert!(read.output().as_text().unwrap().contains("Overview"));
    }
}

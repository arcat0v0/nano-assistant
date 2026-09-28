use std::fmt::Write;
use std::sync::Arc;

use parking_lot::Mutex;
use rig::tool::{Tool, ToolContext, ToolExecutionError};

use super::deferred::{ActivatedToolSet, DeferredMcpToolSet};

const DEFAULT_MAX_RESULTS: usize = 5;

pub struct ToolSearchTool {
    deferred: Vec<DeferredMcpToolSet>,
    activated: Arc<Mutex<ActivatedToolSet>>,
}

impl ToolSearchTool {
    pub fn new(deferred: DeferredMcpToolSet, activated: Arc<Mutex<ActivatedToolSet>>) -> Self {
        Self::new_multi(vec![deferred], activated)
    }

    pub fn new_multi(
        deferred: Vec<DeferredMcpToolSet>,
        activated: Arc<Mutex<ActivatedToolSet>>,
    ) -> Self {
        Self {
            deferred,
            activated,
        }
    }

    fn find_set(&self, name: &str) -> Option<&DeferredMcpToolSet> {
        self.deferred
            .iter()
            .find(|set| set.get_by_name(name).is_some())
    }

    fn activate(&self, name: &str, output: &mut String) -> Result<bool, ToolExecutionError> {
        let Some(set) = self.find_set(name) else {
            return Ok(false);
        };
        let mut activated = self.activated.lock();
        let definition = match activated.get(name) {
            Some(tool) => tool.definition(),
            None => {
                let tool = set.activate(name).expect("discovered MCP tool");
                let definition = tool.definition();
                activated.activate(tool);
                definition
            }
        };
        drop(activated);
        let definition = serde_json::to_string(&definition).map_err(|e| {
            ToolExecutionError::other(format!("Could not serialize tool definition: {e}"))
        })?;
        let _ = writeln!(output, "<function>{definition}</function>");
        Ok(true)
    }

    fn select_tools(&self, names: &[&str]) -> Result<String, ToolExecutionError> {
        let mut output = String::from("<functions>\n");
        let mut not_found = Vec::new();
        for name in names.iter().copied().filter(|name| !name.is_empty()) {
            if !self.activate(name, &mut output)? {
                not_found.push(name);
            }
        }
        output.push_str("</functions>\n");
        if !not_found.is_empty() {
            let _ = write!(output, "\nNot found: {}", not_found.join(", "));
        }
        Ok(output)
    }
}

impl Tool for ToolSearchTool {
    const NAME: &'static str = "tool_search";
    type Args = serde_json::Value;
    type Output = String;
    type Error = ToolExecutionError;

    fn description(&self) -> String {
        "Fetch full schema definitions for deferred MCP tools so they can be called. \
         Use \"select:name1,name2\" for exact match or keywords to search."
            .into()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "description": "Query to find deferred tools. Use \"select:<tool_name>\" for direct selection, or keywords to search.",
                    "type": "string"
                },
                "max_results": {
                    "description": "Maximum number of results to return (default: 5)",
                    "type": "number",
                    "default": DEFAULT_MAX_RESULTS
                }
            },
            "required": ["query"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: serde_json::Value,
    ) -> Result<String, ToolExecutionError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        if query.is_empty() {
            return Err(ToolExecutionError::invalid_args(
                "query parameter is required",
            ));
        }

        if let Some(names_str) = query.strip_prefix("select:") {
            let names: Vec<&str> = names_str.split(',').map(str::trim).collect();
            return self.select_tools(&names);
        }

        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|v| usize::try_from(v).unwrap_or(DEFAULT_MAX_RESULTS))
            .unwrap_or(DEFAULT_MAX_RESULTS);
        let results: Vec<_> = self
            .deferred
            .iter()
            .flat_map(|set| set.search(query, max_results))
            .take(max_results)
            .collect();
        if results.is_empty() {
            return Ok("No matching deferred tools found.".into());
        }

        let mut output = String::from("<functions>\n");
        for stub in results {
            self.activate(&stub.prefixed_name, &mut output)?;
        }
        output.push_str("</functions>\n");
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::client::McpRegistry;
    use crate::mcp::deferred::DeferredMcpToolStub;
    use crate::mcp::protocol::McpToolDef;
    use rig::tool::{ToolErrorKind, ToolSet};

    async fn make_deferred_set(stubs: Vec<DeferredMcpToolStub>) -> DeferredMcpToolSet {
        let registry = Arc::new(McpRegistry::connect_all(&[]).await.unwrap());
        DeferredMcpToolSet { stubs, registry }
    }

    fn make_stub(name: &str, desc: &str) -> DeferredMcpToolStub {
        DeferredMcpToolStub::new(
            name.into(),
            McpToolDef {
                name: name.into(),
                description: Some(desc.into()),
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
            },
        )
    }

    #[tokio::test]
    async fn missing_query_is_model_visible_invalid_args() {
        let tool = ToolSearchTool::new(
            make_deferred_set(vec![]).await,
            Arc::new(Mutex::new(ActivatedToolSet::new())),
        );
        let tools = ToolSet::from_tools(vec![tool]);
        let result = tools
            .execute(
                ToolSearchTool::NAME,
                r#"{"query":""}"#,
                &mut ToolContext::new(),
            )
            .await;
        assert!(result.is_error_kind(ToolErrorKind::InvalidArgs));
        assert!(result
            .error()
            .unwrap()
            .model_output()
            .as_text()
            .unwrap()
            .contains("query parameter is required"));
    }

    #[tokio::test]
    async fn search_and_selection_activate_tools_across_sets() {
        let activated = Arc::new(Mutex::new(ActivatedToolSet::new()));
        let tool = ToolSearchTool::new_multi(
            vec![
                make_deferred_set(vec![make_stub("mcp__fs__read", "Read a file")]).await,
                make_deferred_set(vec![make_stub("mcp__db__query", "Query database")]).await,
            ],
            Arc::clone(&activated),
        );
        let mut context = ToolContext::new();
        let output = tool
            .call(&mut context, serde_json::json!({"query": "read"}))
            .await
            .unwrap();
        assert!(output.contains("mcp__fs__read"));
        assert!(!output.contains("mcp__db__query"));
        assert!(activated.lock().is_activated("mcp__fs__read"));
        assert!(!activated.lock().is_activated("mcp__db__query"));

        let output = tool
            .call(
                &mut context,
                serde_json::json!({"query": "select:mcp__db__query,missing"}),
            )
            .await
            .unwrap();
        assert!(output.contains("mcp__db__query"));
        assert!(output.contains("Not found: missing"));
        assert!(activated.lock().is_activated("mcp__db__query"));
        assert_eq!(activated.lock().tool_names().len(), 2);

        tool.call(
            &mut context,
            serde_json::json!({"query": "select:mcp__fs__read"}),
        )
        .await
        .unwrap();
        assert_eq!(activated.lock().tool_names().len(), 2);
    }

    #[tokio::test]
    async fn search_serializes_tool_descriptions_as_json() {
        let tool = ToolSearchTool::new(
            make_deferred_set(vec![make_stub("mcp__fs__read", "Read \"quoted\" paths")]).await,
            Arc::new(Mutex::new(ActivatedToolSet::new())),
        );
        let output = tool
            .call(
                &mut ToolContext::new(),
                serde_json::json!({"query": "select:mcp__fs__read"}),
            )
            .await
            .unwrap();
        let definition = output
            .strip_prefix("<functions>\n<function>")
            .unwrap()
            .strip_suffix("</function>\n</functions>\n")
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(definition).unwrap();
        assert_eq!(parsed["description"], "Read \"quoted\" paths");
    }
}

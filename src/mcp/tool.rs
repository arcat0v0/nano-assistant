use std::sync::Arc;

use rig::tool::{DynamicTool, ToolExecutionError, ToolOutput};

use super::client::McpRegistry;
use super::protocol::McpToolDef;

pub struct McpToolWrapper {
    prefixed_name: String,
    description: String,
    input_schema: serde_json::Value,
    registry: Arc<McpRegistry>,
}

impl McpToolWrapper {
    pub fn new(prefixed_name: String, def: McpToolDef, registry: Arc<McpRegistry>) -> Self {
        let description = def.description.unwrap_or_else(|| "MCP tool".to_string());
        Self {
            prefixed_name,
            description,
            input_schema: def.input_schema,
            registry,
        }
    }

    pub fn into_dynamic(self) -> DynamicTool {
        let Self {
            prefixed_name,
            description,
            input_schema,
            registry,
        } = self;
        let name = prefixed_name.clone();
        DynamicTool::new(
            prefixed_name,
            description,
            input_schema,
            move |_context, args| {
                let registry = Arc::clone(&registry);
                let name = name.clone();
                Box::pin(async move {
                    let output = registry
                        .call_tool(&name, args)
                        .await
                        .map_err(|e| ToolExecutionError::other(e.to_string()))?;
                    Ok(ToolOutput::text(output))
                })
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::tool::{ToolContext, ToolSet};
    use serde_json::json;

    #[tokio::test]
    async fn dynamic_mcp_tool_exposes_definition_and_reports_unknown_tool() {
        let registry = Arc::new(McpRegistry::connect_all(&[]).await.unwrap());
        let schema = json!({"type": "object", "properties": {"path": {"type": "string"}}});
        let def = McpToolDef {
            name: "read".into(),
            description: Some("Read a file".into()),
            input_schema: schema.clone(),
        };
        let dynamic = McpToolWrapper::new("fs__read".into(), def, registry).into_dynamic();
        let definition = dynamic.definition();
        assert_eq!(definition.name, "fs__read");
        assert_eq!(definition.description, "Read a file");
        assert_eq!(definition.parameters, schema);

        let tools = ToolSet::from_dynamic_tools(vec![dynamic]);
        let result = tools
            .execute("fs__read", "{}", &mut ToolContext::new())
            .await;
        assert!(result.is_error());
        assert!(result
            .error()
            .unwrap()
            .message()
            .contains("unknown MCP tool"));
    }
}

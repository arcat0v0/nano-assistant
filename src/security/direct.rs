use crate::tools::traits::{Tool, ToolResult};

pub async fn execute(
    tool: &dyn Tool,
    args: serde_json::Value,
) -> anyhow::Result<ToolResult> {
    tool.execute(args).await
}

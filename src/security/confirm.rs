use crate::security::UserConfirmation;
use crate::tools::traits::{Tool, ToolResult};

pub async fn execute(
    tool: &dyn Tool,
    args: serde_json::Value,
    confirmer: &dyn UserConfirmation,
) -> anyhow::Result<ToolResult> {
    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("<unknown>");

    if confirmer.confirm(command).await {
        tool.execute(args).await
    } else {
        Ok(ToolResult {
            success: false,
            output: String::new(),
            error: Some("Execution denied by user".into()),
        })
    }
}

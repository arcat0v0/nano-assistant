use super::file_mutation::PreparedFileMutation;
use crate::security::{ResolvedAction, SecurityManager, SecurityMode, ToolAction};
use rig::tool::{DynamicTool, Tool, ToolContext, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct FileWriteTool;

impl FileWriteTool {
    pub fn new() -> Self {
        Self
    }

    pub(crate) fn into_dynamic(self, security: Arc<SecurityManager>) -> DynamicTool {
        security.register_prepared_tool(Self::NAME);
        DynamicTool::new(
            Self::NAME,
            self.description(),
            self.parameters(),
            move |_context, args| {
                let security = Arc::clone(&security);
                Box::pin(
                    async move { Self::run(args, Some(&security)).await.map(ToolOutput::text) },
                )
            },
        )
    }

    async fn run(
        args: Value,
        security: Option<&SecurityManager>,
    ) -> Result<String, ToolExecutionError> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolExecutionError::invalid_args("Missing 'path' parameter"))?;
        let content = args
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolExecutionError::invalid_args("Missing 'content' parameter"))?;
        let prepared = PreparedFileMutation::write(std::path::Path::new(path), content).await?;
        let message = format!("Written {} bytes to {path}", content.len());
        let mut receipt = None;
        if let Some(security) = security.filter(|security| security.mode() == SecurityMode::Auto) {
            let evidence = prepared.evidence();
            receipt = security
                .authorize_prepared(
                    &ToolAction {
                        tool_name: Self::NAME,
                        args: &args,
                        description: Some(&Self.description()),
                        resolved: Some(ResolvedAction::FileMutation {
                            evidence: &evidence,
                        }),
                    },
                    &prepared.preview(),
                )
                .await
                .map_err(ToolExecutionError::permission_denied)?;
        }
        let result = prepared.commit().await;
        if let Some(security) = security {
            security.record_execution(receipt.as_ref(), result.is_ok());
        }
        result?;
        Ok(message)
    }
}

impl Default for FileWriteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for FileWriteTool {
    const NAME: &'static str = "file_write";
    type Args = Value;
    type Output = String;
    type Error = ToolExecutionError;

    fn description(&self) -> String {
        "Write content to a file. Creates parent directories if needed. \
         Overwrites existing files."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Value,
    ) -> Result<String, ToolExecutionError> {
        Self::run(args, None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_nested_file_and_blocks_builtin_skill_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/file.txt");
        FileWriteTool
            .call(
                &mut ToolContext::default(),
                json!({"path": path, "content": "hello"}),
            )
            .await
            .unwrap();
        assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "hello");
        let builtin =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("skills/arch-wiki/SKILL.toml");
        let original = tokio::fs::read_to_string(&builtin).await.unwrap();
        let error = FileWriteTool
            .call(
                &mut ToolContext::default(),
                json!({"path": builtin, "content": "malicious"}),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("builtin skill"));
        assert_eq!(tokio::fs::read_to_string(&builtin).await.unwrap(), original);
        let nonexistent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("skills/arch-wiki/new-folder/new-file.txt");
        let error = FileWriteTool
            .call(
                &mut ToolContext::default(),
                json!({"path": nonexistent, "content": "malicious"}),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("builtin skill"));
        assert!(!nonexistent.exists());
    }
}

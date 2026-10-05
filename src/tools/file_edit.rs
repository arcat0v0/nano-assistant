use super::file_mutation::PreparedFileMutation;
use crate::security::{ResolvedAction, SecurityManager, SecurityMode, ToolAction};
use rig::tool::{DynamicTool, Tool, ToolContext, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct FileEditTool;

impl FileEditTool {
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
        let old = args
            .get("old_string")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolExecutionError::invalid_args("Missing 'old_string' parameter"))?;
        let new = args
            .get("new_string")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolExecutionError::invalid_args("Missing 'new_string' parameter"))?;
        let prepared = PreparedFileMutation::edit(std::path::Path::new(path), old, new).await?;
        let message = format!("Edited {path}: replaced 1 occurrence");
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

impl Default for FileEditTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for FileEditTool {
    const NAME: &'static str = "file_edit";
    type Args = Value;
    type Output = String;
    type Error = ToolExecutionError;

    fn description(&self) -> String {
        "Edit a file by replacing an exact string match with new content. \
         The old_string must appear exactly once in the file."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact text to find and replace (must appear exactly once)"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement text (empty string to delete the matched text)"
                }
            },
            "required": ["path", "old_string", "new_string"]
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
    async fn zero_matches_leave_file_unchanged() {
        let file = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(file.path(), "original content")
            .await
            .unwrap();

        let err = FileEditTool
            .call(
                &mut ToolContext::default(),
                json!({"path": file.path(), "old_string": "absent", "new_string": "changed"}),
            )
            .await
            .unwrap_err();

        assert_eq!(err.kind(), rig::tool::ToolErrorKind::NotFound);
        assert!(err.model_feedback().unwrap().contains("old_string"));
        assert_eq!(
            tokio::fs::read_to_string(file.path()).await.unwrap(),
            "original content"
        );
    }

    #[tokio::test]
    async fn empty_old_string_is_rejected_without_mutation() {
        let file = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(file.path(), "original content")
            .await
            .unwrap();

        let err = FileEditTool
            .call(
                &mut ToolContext::default(),
                json!({"path": file.path(), "old_string": "", "new_string": "changed"}),
            )
            .await
            .unwrap_err();

        assert_eq!(err.kind(), rig::tool::ToolErrorKind::InvalidArgs);
        let feedback = err.model_feedback().unwrap();
        assert!(feedback.contains("old_string"));
        assert!(feedback.contains("empty"));
        assert_eq!(
            tokio::fs::read_to_string(file.path()).await.unwrap(),
            "original content"
        );
    }

    #[tokio::test]
    async fn empty_new_string_deletes_the_unique_match() {
        let file = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(file.path(), "before REMOVE after")
            .await
            .unwrap();

        FileEditTool
            .call(
                &mut ToolContext::default(),
                json!({"path": file.path(), "old_string": "REMOVE ", "new_string": ""}),
            )
            .await
            .unwrap();

        assert_eq!(
            tokio::fs::read_to_string(file.path()).await.unwrap(),
            "before after"
        );
    }

    #[tokio::test]
    async fn duplicate_match_error_is_model_visible_and_correction_succeeds() {
        let file = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(file.path(), "repeat repeat")
            .await
            .unwrap();
        let mut context = ToolContext::default();

        let err = FileEditTool
            .call(
                &mut context,
                json!({"path": file.path(), "old_string": "repeat", "new_string": "changed"}),
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind(), rig::tool::ToolErrorKind::InvalidArgs);
        let feedback = err.model_feedback().unwrap();
        assert!(
            feedback.contains('2'),
            "feedback omits match count: {feedback}"
        );
        assert!(
            feedback.contains("once") || feedback.contains("unique"),
            "feedback omits uniqueness requirement: {feedback}"
        );
        assert_eq!(
            tokio::fs::read_to_string(file.path()).await.unwrap(),
            "repeat repeat"
        );

        FileEditTool
            .call(
                &mut context,
                json!({"path": file.path(), "old_string": "repeat repeat", "new_string": "changed"}),
            )
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(file.path()).await.unwrap(),
            "changed"
        );
    }

    #[tokio::test]
    async fn builtin_skill_source_cannot_be_edited() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("skills/arch-wiki/SKILL.toml");
        let args = json!({"path": path, "old_string": "description", "new_string": "changed"});
        let err = FileEditTool
            .call(&mut ToolContext::default(), args)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("builtin skill"));
    }
}

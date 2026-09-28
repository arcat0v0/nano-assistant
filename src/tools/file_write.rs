use rig::tool::{Tool, ToolContext, ToolExecutionError};
use serde_json::{json, Value};

pub struct FileWriteTool;

impl FileWriteTool {
    pub fn new() -> Self {
        Self
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
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolExecutionError::invalid_args("Missing 'path' parameter"))?;
        let content = args
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolExecutionError::invalid_args("Missing 'content' parameter"))?;

        let full_path = std::path::Path::new(path);
        if super::is_protected_skill_path(full_path) {
            return Err(ToolExecutionError::permission_denied(format!(
                "Refusing to write builtin skill source: {}",
                full_path.display()
            )));
        }

        if let Some(parent) = full_path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ToolExecutionError::other(format!(
                    "Failed to create parent directory for {path}: {e}"
                ))
            })?;
        }
        tokio::fs::write(full_path, content)
            .await
            .map_err(|e| ToolExecutionError::other(format!("Failed to write file {path}: {e}")))?;
        Ok(format!("Written {} bytes to {path}", content.len()))
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

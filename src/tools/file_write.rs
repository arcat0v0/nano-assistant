use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;

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

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates parent directories if needed. \
         Overwrites existing files."
    }

    fn parameters_schema(&self) -> serde_json::Value {
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

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' parameter"))?;

        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'content' parameter"))?;

        let full_path = std::path::Path::new(path);

        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        match tokio::fs::write(full_path, content).await {
            Ok(()) => Ok(ToolResult {
                success: true,
                output: format!("Written {} bytes to {path}", content.len()),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to write file: {e}")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn writes_file() {
        let dir = test_dir("nano_file_write_test");
        let tool = FileWriteTool::new();
        let result = tool
            .execute(json!({
                "path": dir.join("out.txt").to_string_lossy(),
                "content": "hello!"
            }))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("6 bytes"));

        let content = tokio::fs::read_to_string(dir.join("out.txt"))
            .await
            .unwrap();
        assert_eq!(content, "hello!");
    }

    #[tokio::test]
    async fn creates_parent_dirs() {
        let dir = test_dir("nano_file_write_nested");
        let tool = FileWriteTool::new();
        let result = tool
            .execute(json!({
                "path": dir.join("a/b/c/deep.txt").to_string_lossy(),
                "content": "deep"
            }))
            .await
            .unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn missing_params() {
        let tool = FileWriteTool::new();
        assert!(tool.execute(json!({"path": "f.txt"})).await.is_err());
        assert!(tool.execute(json!({"content": "x"})).await.is_err());
    }
}

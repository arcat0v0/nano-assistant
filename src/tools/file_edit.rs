use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;

pub struct FileEditTool;

impl FileEditTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FileEditTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for FileEditTool {
    fn name(&self) -> &str {
        "file_edit"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing an exact string match with new content. \
         The old_string must appear exactly once in the file."
    }

    fn parameters_schema(&self) -> serde_json::Value {
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

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' parameter"))?;

        let old_string = args
            .get("old_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'old_string' parameter"))?;

        let new_string = args
            .get("new_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'new_string' parameter"))?;

        if old_string.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("old_string must not be empty".into()),
            });
        }

        let path_buf = std::path::PathBuf::from(path);
        if crate::skills::is_builtin_skill_path(&path_buf) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Refusing to edit builtin skill source: {}",
                    path_buf.display()
                )),
            });
        }

        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to read file: {e}")),
                });
            }
        };

        let match_count = content.matches(old_string).count();

        if match_count == 0 {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("old_string not found in file".into()),
            });
        }

        if match_count > 1 {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "old_string matches {match_count} times; must match exactly once"
                )),
            });
        }

        let new_content = content.replacen(old_string, new_string, 1);

        match tokio::fs::write(path, &new_content).await {
            Ok(()) => Ok(ToolResult {
                success: true,
                output: format!(
                    "Edited {path}: replaced 1 occurrence ({} bytes)",
                    new_content.len()
                ),
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
    use crate::tools::Tool;

    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn replaces_single_match() {
        let dir = test_dir("nano_file_edit_test");
        let file = dir.join("test.txt");
        std::fs::write(&file, "hello world").unwrap();

        let tool = FileEditTool::new();
        let result = tool
            .execute(json!({
                "path": file.to_string_lossy(),
                "old_string": "hello",
                "new_string": "goodbye"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("replaced 1 occurrence"));

        let content = tokio::fs::read_to_string(&file).await.unwrap();
        assert_eq!(content, "goodbye world");
    }

    #[tokio::test]
    async fn not_found() {
        let dir = test_dir("nano_file_edit_notfound");
        let file = dir.join("test.txt");
        std::fs::write(&file, "hello world").unwrap();

        let tool = FileEditTool::new();
        let result = tool
            .execute(json!({
                "path": file.to_string_lossy(),
                "old_string": "nonexistent",
                "new_string": "x"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("not found"));
    }

    #[tokio::test]
    async fn multiple_matches_rejected() {
        let dir = test_dir("nano_file_edit_multi");
        let file = dir.join("test.txt");
        std::fs::write(&file, "aaa bbb aaa").unwrap();

        let tool = FileEditTool::new();
        let result = tool
            .execute(json!({
                "path": file.to_string_lossy(),
                "old_string": "aaa",
                "new_string": "ccc"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("matches 2 times"));
    }

    #[tokio::test]
    async fn delete_via_empty_new_string() {
        let dir = test_dir("nano_file_edit_delete");
        let file = dir.join("test.txt");
        std::fs::write(&file, "keep remove keep").unwrap();

        let tool = FileEditTool::new();
        let result = tool
            .execute(json!({
                "path": file.to_string_lossy(),
                "old_string": " remove",
                "new_string": ""
            }))
            .await
            .unwrap();

        assert!(result.success);
        let content = tokio::fs::read_to_string(&file).await.unwrap();
        assert_eq!(content, "keep keep");
    }

    #[tokio::test]
    async fn empty_old_string_rejected() {
        let tool = FileEditTool::new();
        let result = tool
            .execute(json!({"path": "/tmp/x", "old_string": "", "new_string": "y"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("must not be empty"));
    }

    #[tokio::test]
    async fn rejects_builtin_skill_edits() {
        let builtin_skill = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("skills")
            .join("arch-wiki")
            .join("SKILL.toml");

        let original = tokio::fs::read_to_string(&builtin_skill).await.unwrap();
        let old_string =
            r#"description = "ArchLinux official wiki — comprehensive Linux documentation""#;

        let tool = FileEditTool::new();
        let result = tool
            .execute(json!({
                "path": builtin_skill.to_string_lossy(),
                "old_string": old_string,
                "new_string": r#"description = "Modified""#
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("builtin skill"));

        let after = tokio::fs::read_to_string(&builtin_skill).await.unwrap();
        assert_eq!(after, original);
    }
}

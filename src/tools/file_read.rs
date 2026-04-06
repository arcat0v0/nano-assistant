use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;

const MAX_FILE_SIZE_BYTES: u64 = 10 * 1024 * 1024;

/// Read file contents with optional line range.
pub struct FileReadTool;

impl FileReadTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FileReadTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read file contents with line numbers. Supports partial reading via offset and limit. \
         Binary files are returned with lossy UTF-8 conversion."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "Starting line number (1-based, default: 1)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to return (default: all)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' parameter"))?;

        let resolved = tokio::fs::canonicalize(path).await;

        let resolved = match resolved {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to resolve file path: {e}")),
                });
            }
        };

        match tokio::fs::metadata(&resolved).await {
            Ok(meta) if meta.len() > MAX_FILE_SIZE_BYTES => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "File too large: {} bytes (limit: {MAX_FILE_SIZE_BYTES} bytes)",
                        meta.len()
                    )),
                });
            }
            Ok(_) => {}
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to read file metadata: {e}")),
                });
            }
        }

        match tokio::fs::read_to_string(&resolved).await {
            Ok(contents) => {
                let lines: Vec<&str> = contents.lines().collect();
                let total = lines.len();

                if total == 0 {
                    return Ok(ToolResult {
                        success: true,
                        output: String::new(),
                        error: None,
                    });
                }

                let offset = args
                    .get("offset")
                    .and_then(|v| v.as_u64())
                    .map(|v| {
                        usize::try_from(v.max(1))
                            .unwrap_or(usize::MAX)
                            .saturating_sub(1)
                    })
                    .unwrap_or(0);
                let start = offset.min(total);

                let end = match args.get("limit").and_then(|v| v.as_u64()) {
                    Some(l) => {
                        let limit = usize::try_from(l).unwrap_or(usize::MAX);
                        (start.saturating_add(limit)).min(total)
                    }
                    None => total,
                };

                if start >= end {
                    return Ok(ToolResult {
                        success: true,
                        output: format!("[No lines in range, file has {total} lines]"),
                        error: None,
                    });
                }

                let numbered: String = lines[start..end]
                    .iter()
                    .enumerate()
                    .map(|(i, line)| format!("{}: {}", start + i + 1, line))
                    .collect::<Vec<_>>()
                    .join("\n");

                let partial = start > 0 || end < total;
                let summary = if partial {
                    format!("\n[Lines {}-{} of {total}]", start + 1, end)
                } else {
                    format!("\n[{total} lines total]")
                };

                Ok(ToolResult {
                    success: true,
                    output: format!("{numbered}{summary}"),
                    error: None,
                })
            }
            Err(_) => {
                let bytes = tokio::fs::read(&resolved)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to read file: {e}"))?;

                let lossy = String::from_utf8_lossy(&bytes).into_owned();
                Ok(ToolResult {
                    success: true,
                    output: format!("[binary file, lossy UTF-8 conversion]\n{lossy}"),
                    error: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn reads_existing_file() {
        let dir = test_dir("nano_file_read_test");
        std::fs::write(dir.join("test.txt"), "hello world").unwrap();

        let tool = FileReadTool::new();
        let result = tool
            .execute(json!({"path": dir.join("test.txt").to_string_lossy()}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("1: hello world"));
    }

    #[tokio::test]
    async fn offset_and_limit() {
        let dir = test_dir("nano_file_read_offset");
        std::fs::write(dir.join("lines.txt"), "a\nb\nc\nd\ne").unwrap();

        let tool = FileReadTool::new();
        let result = tool
            .execute(json!({
                "path": dir.join("lines.txt").to_string_lossy(),
                "offset": 2,
                "limit": 2
            }))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("2: b"));
        assert!(result.output.contains("3: c"));
        assert!(result.output.contains("[Lines 2-3 of 5]"));
    }

    #[tokio::test]
    async fn nonexistent_file() {
        let tool = FileReadTool::new();
        let result = tool
            .execute(json!({"path": "/nonexistent_xyz_12345"}))
            .await
            .unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn missing_path_param() {
        let tool = FileReadTool::new();
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
    }
}

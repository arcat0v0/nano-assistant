use rig::tool::{Tool, ToolContext, ToolExecutionError};
use serde_json::{json, Value};

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

impl Tool for FileReadTool {
    const NAME: &'static str = "file_read";
    type Args = Value;
    type Output = String;
    type Error = ToolExecutionError;

    fn description(&self) -> String {
        "Read file contents with line numbers. Supports partial reading via offset and limit. \
         Binary files are returned with lossy UTF-8 conversion."
            .into()
    }

    fn parameters(&self) -> Value {
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

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Value,
    ) -> Result<String, ToolExecutionError> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolExecutionError::invalid_args("Missing 'path' parameter"))?;
        let resolved = tokio::fs::canonicalize(path).await.map_err(|e| {
            ToolExecutionError::other(format!("Failed to resolve file path {path}: {e}"))
        })?;

        let metadata = tokio::fs::metadata(&resolved).await.map_err(|e| {
            ToolExecutionError::other(format!("Failed to read file metadata for {path}: {e}"))
        })?;
        if metadata.len() > MAX_FILE_SIZE_BYTES {
            return Err(ToolExecutionError::other(format!(
                "File too large: {} bytes (limit: {MAX_FILE_SIZE_BYTES} bytes)",
                metadata.len()
            )));
        }
        match tokio::fs::read_to_string(&resolved).await {
            Ok(contents) => {
                let lines: Vec<&str> = contents.lines().collect();
                let total = lines.len();

                if total == 0 {
                    return Ok(String::new());
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
                    return Ok(format!("[No lines in range, file has {total} lines]"));
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

                Ok(format!("{numbered}{summary}"))
            }
            Err(_) => {
                let bytes = tokio::fs::read(&resolved).await.map_err(|e| {
                    ToolExecutionError::other(format!("Failed to read file {path}: {e}"))
                })?;
                let lossy = String::from_utf8_lossy(&bytes);
                Ok(format!("[binary file, lossy UTF-8 conversion]\n{lossy}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_numbered_requested_lines() {
        let file = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(file.path(), "a\nb\nc\nd").await.unwrap();
        let output = FileReadTool
            .call(
                &mut ToolContext::default(),
                json!({"path": file.path(), "offset": 2, "limit": 2}),
            )
            .await
            .unwrap();
        assert_eq!(output, "2: b\n3: c\n[Lines 2-3 of 4]");
    }
}

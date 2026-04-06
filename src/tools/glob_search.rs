use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;

const MAX_RESULTS: usize = 1000;

pub struct GlobSearchTool;

impl GlobSearchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GlobSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GlobSearchTool {
    fn name(&self) -> &str {
        "glob_search"
    }

    fn description(&self) -> &str {
        "Search for files matching a glob pattern. \
         Returns a sorted list of matching file paths. \
         Examples: '**/*.rs', 'src/**/*.mod.rs'."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match files, e.g. '**/*.rs', 'src/**/mod.rs'"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'pattern' parameter"))?;

        let entries = match glob::glob(pattern) {
            Ok(paths) => paths,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Invalid glob pattern: {e}")),
                });
            }
        };

        let mut results = Vec::new();
        let mut truncated = false;

        for entry in entries {
            let path = match entry {
                Ok(p) => p,
                Err(_) => continue,
            };

            if path.is_dir() {
                continue;
            }

            results.push(path.to_string_lossy().to_string());

            if results.len() >= MAX_RESULTS {
                truncated = true;
                break;
            }
        }

        results.sort();

        let output = if results.is_empty() {
            format!("No files matching pattern '{pattern}' found.")
        } else {
            use std::fmt::Write;
            let mut buf = results.join("\n");
            if truncated {
                let _ = write!(
                    buf,
                    "\n\n[Results truncated: showing first {MAX_RESULTS} of more matches]"
                );
            }
            let _ = write!(buf, "\n\nTotal: {} files", results.len());
            buf
        };

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
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
    async fn finds_files() {
        let dir = test_dir("nano_glob_test");
        std::fs::write(dir.join("a.txt"), "").unwrap();
        std::fs::write(dir.join("b.rs"), "").unwrap();

        let tool = GlobSearchTool::new();
        let result = tool
            .execute(json!({"pattern": dir.join("*.txt").to_string_lossy()}))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("a.txt"));
        assert!(!result.output.contains("b.rs"));
    }

    #[tokio::test]
    async fn no_matches() {
        let tool = GlobSearchTool::new();
        let result = tool
            .execute(json!({"pattern": "/nonexistent/**/*.xyz"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("No files matching"));
    }

    #[tokio::test]
    async fn missing_param() {
        let tool = GlobSearchTool::new();
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn invalid_pattern() {
        let tool = GlobSearchTool::new();
        let result = tool.execute(json!({"pattern": "[invalid"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("Invalid glob"));
    }
}

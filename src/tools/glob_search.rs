use rig::tool::{Tool, ToolContext, ToolExecutionError};
use serde_json::{json, Value};

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

impl Tool for GlobSearchTool {
    const NAME: &'static str = "glob_search";
    type Args = Value;
    type Output = String;
    type Error = ToolExecutionError;

    fn description(&self) -> String {
        "Search for files matching a glob pattern. \
         Returns a sorted list of matching file paths. \
         Examples: '**/*.rs', 'src/**/*.mod.rs'."
            .into()
    }

    fn parameters(&self) -> Value {
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

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Value,
    ) -> Result<String, ToolExecutionError> {
        let pattern = args
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolExecutionError::invalid_args("Missing 'pattern' parameter"))?;
        let entries = glob::glob(pattern)
            .map_err(|e| ToolExecutionError::invalid_args(format!("Invalid glob pattern: {e}")))?;

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

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn glob_matches_files_not_directories() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("a.txt"), "")
            .await
            .unwrap();
        tokio::fs::create_dir(dir.path().join("b.txt"))
            .await
            .unwrap();
        let output = GlobSearchTool
            .call(
                &mut ToolContext::default(),
                json!({"pattern": dir.path().join("*.txt")}),
            )
            .await
            .unwrap();
        assert!(output.contains("a.txt"));
        assert!(!output.contains("b.txt"));
    }
}

use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use regex::RegexBuilder;
use serde_json::json;
use std::fmt::Write;
use std::path::Path;

const MAX_RESULTS: usize = 1000;
const MAX_OUTPUT_BYTES: usize = 1_048_576;

pub struct ContentSearchTool;

impl ContentSearchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ContentSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

fn walk_dir(
    root: &Path,
    pattern: &str,
    case_sensitive: bool,
    include: Option<&str>,
) -> anyhow::Result<Vec<MatchResult>> {
    let re = RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|e| anyhow::anyhow!("Invalid regex pattern: {e}"))?;

    let include_glob = include.map(glob::Pattern::new).transpose().ok().flatten();

    let mut results = Vec::new();

    fn visit(
        dir: &Path,
        re: &regex::Regex,
        include_glob: &Option<glob::Pattern>,
        results: &mut Vec<MatchResult>,
    ) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, re, include_glob, results);
                continue;
            }

            if let Some(ref glob) = include_glob {
                let name = path.file_name().map(|n| n.to_string_lossy());
                if let Some(name) = name {
                    if !glob.matches(&name) {
                        continue;
                    }
                } else {
                    continue;
                }
            }

            if let Ok(content) = std::fs::read_to_string(&path) {
                for (line_num, line) in content.lines().enumerate() {
                    if re.is_match(line) {
                        results.push(MatchResult {
                            path: path.clone(),
                            line_num: line_num + 1,
                            line: line.to_string(),
                        });
                    }
                    if results.len() >= MAX_RESULTS {
                        return;
                    }
                }
            }
        }
    }

    visit(root, &re, &include_glob, &mut results);
    Ok(results)
}

struct MatchResult {
    path: std::path::PathBuf,
    line_num: usize,
    line: String,
}

#[async_trait]
impl Tool for ContentSearchTool {
    fn name(&self) -> &str {
        "content_search"
    }

    fn description(&self) -> &str {
        "Search file contents by regex pattern. Returns matching lines with file paths and line numbers."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: current directory)"
                },
                "include": {
                    "type": "string",
                    "description": "File glob filter, e.g. '*.rs', '*.{ts,tsx}'"
                },
                "case_sensitive": {
                    "type": "boolean",
                    "description": "Case-sensitive matching (default: true)"
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

        if pattern.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Empty pattern is not allowed.".into()),
            });
        }

        let search_path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let include = args.get("include").and_then(|v| v.as_str());
        let case_sensitive = args
            .get("case_sensitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let pattern_owned = pattern.to_string();
        let include_owned = include.map(|s| s.to_string());
        let search_path_owned = search_path.to_string();

        let matches = tokio::task::spawn_blocking(move || {
            walk_dir(
                Path::new(&search_path_owned),
                &pattern_owned,
                case_sensitive,
                include_owned.as_deref(),
            )
        })
        .await
        .map_err(|e| anyhow::anyhow!("Search task failed: {e}"))??;

        if matches.is_empty() {
            return Ok(ToolResult {
                success: true,
                output: "No matches found.".into(),
                error: None,
            });
        }

        let mut buf = String::new();
        let mut file_count = std::collections::HashSet::new();

        for m in &matches {
            let path_str = m.path.to_string_lossy();
            file_count.insert(path_str.to_string());
            writeln!(buf, "{}:{}:{}", path_str, m.line_num, m.line).unwrap();

            if buf.len() > MAX_OUTPUT_BYTES {
                buf.push_str("\n\n[Output truncated: exceeded 1 MB limit]");
                break;
            }
        }

        writeln!(
            buf,
            "\n\nTotal: {} matches in {} files",
            matches.len(),
            file_count.len()
        )
        .unwrap();

        Ok(ToolResult {
            success: true,
            output: buf,
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
    async fn finds_matches() {
        let dir = test_dir("nano_content_search_test");
        std::fs::write(
            dir.join("main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        std::fs::write(dir.join("lib.rs"), "pub fn greet() {}\n").unwrap();

        let tool = ContentSearchTool::new();
        let result = tool
            .execute(json!({"pattern": "fn main", "path": dir.to_string_lossy()}))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("main.rs"));
        assert!(result.output.contains("fn main"));
    }

    #[tokio::test]
    async fn case_insensitive() {
        let dir = test_dir("nano_content_search_ci");
        std::fs::write(dir.join("test.txt"), "Hello World\n").unwrap();

        let tool = ContentSearchTool::new();
        let result = tool
            .execute(json!({
                "pattern": "HELLO",
                "path": dir.to_string_lossy(),
                "case_sensitive": false
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Hello World"));
    }

    #[tokio::test]
    async fn include_filter() {
        let dir = test_dir("nano_content_search_include");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("readme.txt"), "fn main is great\n").unwrap();

        let tool = ContentSearchTool::new();
        let result = tool
            .execute(json!({
                "pattern": "fn",
                "path": dir.to_string_lossy(),
                "include": "*.rs"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("main.rs"));
        assert!(!result.output.contains("readme.txt"));
    }

    #[tokio::test]
    async fn no_matches() {
        let dir = test_dir("nano_content_search_none");
        std::fs::write(dir.join("test.txt"), "hello\n").unwrap();

        let tool = ContentSearchTool::new();
        let result = tool
            .execute(json!({"pattern": "nonexistent_xyz", "path": dir.to_string_lossy()}))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("No matches found"));
    }

    #[tokio::test]
    async fn empty_pattern_rejected() {
        let tool = ContentSearchTool::new();
        let result = tool.execute(json!({"pattern": ""})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("Empty pattern"));
    }

    #[tokio::test]
    async fn missing_param() {
        let tool = ContentSearchTool::new();
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
    }
}

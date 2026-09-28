use regex::RegexBuilder;
use rig::tool::{Tool, ToolContext, ToolExecutionError};
use serde_json::{json, Value};
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

impl Tool for ContentSearchTool {
    const NAME: &'static str = "content_search";
    type Args = Value;
    type Output = String;
    type Error = ToolExecutionError;

    fn description(&self) -> String {
        "Search file contents by regex pattern. Returns matching lines with file paths and line numbers.".into()
    }

    fn parameters(&self) -> Value {
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

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Value,
    ) -> Result<String, ToolExecutionError> {
        let pattern = args
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolExecutionError::invalid_args("Missing 'pattern' parameter"))?;
        if pattern.is_empty() {
            return Err(ToolExecutionError::invalid_args(
                "Empty pattern is not allowed.",
            ));
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
        .map_err(|e| ToolExecutionError::other(format!("Search task failed: {e}")))?
        .map_err(|e| ToolExecutionError::invalid_args(e.to_string()))?;

        if matches.is_empty() {
            return Ok("No matches found.".into());
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

        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn searches_content_case_insensitively_with_include_filter() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("match.rs"), "Hello World")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("omit.txt"), "Hello World")
            .await
            .unwrap();
        let output = ContentSearchTool.call(&mut ToolContext::default(), json!({
            "pattern": "hello", "path": dir.path(), "include": "*.rs", "case_sensitive": false
        })).await.unwrap();
        assert!(output.contains("match.rs:1:Hello World"));
        assert!(!output.contains("omit.txt"));
    }
}

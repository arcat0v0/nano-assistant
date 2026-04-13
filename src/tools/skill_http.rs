use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;

use crate::skills::SkillTool;
use crate::tools::traits::{Tool, ToolResult};

const HTTP_TIMEOUT_SECS: u64 = 30;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024; // 1 MiB

/// A tool that performs a skill-defined HTTP GET request.
///
/// The URL template comes from the skill's `SkillTool.command` field.
/// Placeholder parameters like `{{key}}` are replaced with values provided
/// at call time.
pub struct SkillHttpTool {
    tool_name: String,
    url: String,
    args: HashMap<String, String>,
}

impl SkillHttpTool {
    pub fn new(skill_name: &str, tool: &SkillTool) -> Self {
        let tool_name = format!("{}.{}", skill_name, tool.name);
        let url = tool.command.clone();
        let args = tool.args.clone();
        Self {
            tool_name,
            url,
            args,
        }
    }

    #[cfg(test)]
    pub fn url_template(&self) -> &str {
        &self.url
    }
}

#[async_trait]
impl Tool for SkillHttpTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        "Skill HTTP request"
    }

    fn parameters_schema(&self) -> Value {
        let mut properties = serde_json::Map::new();
        for (key, desc) in &self.args {
            properties.insert(
                key.clone(),
                json!({
                    "type": "string",
                    "description": desc,
                }),
            );
        }
        json!({
            "type": "object",
            "properties": properties,
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let mut url = self.url.clone();
        for key in self.args.keys() {
            if let Some(value) = args.get(key).and_then(|v| v.as_str()) {
                url = url.replace(&format!("{{{{{}}}", key), value);
            }
        }

        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Only http/https URLs are allowed".to_string()),
            });
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build()?;

        let response = client.get(&url).send().await?;

        if !response.status().is_success() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("HTTP {}", response.status())),
            });
        }

        let bytes = response.bytes().await?;
        let text = String::from_utf8_lossy(&bytes).to_string();
        let truncated = text.len() > MAX_RESPONSE_BYTES;
        let result = if truncated {
            format!(
                "{}...\n[response truncated at {} bytes]",
                &text[..MAX_RESPONSE_BYTES],
                MAX_RESPONSE_BYTES
            )
        } else {
            text
        };

        Ok(ToolResult {
            success: true,
            output: result,
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_skill_tool(
        name: &str,
        kind: &str,
        command: &str,
        args: HashMap<String, String>,
    ) -> SkillTool {
        SkillTool {
            name: name.to_string(),
            description: format!("{} tool", name),
            kind: kind.to_string(),
            command: command.to_string(),
            args,
        }
    }

    #[test]
    fn tool_name_is_dotted() {
        let tool = make_skill_tool("fetch", "http", "https://example.com", HashMap::new());
        let ht = SkillHttpTool::new("my-skill", &tool);
        assert_eq!(ht.name(), "my-skill.fetch");
    }

    #[test]
    fn url_parameter_substitution() {
        let mut args = HashMap::new();
        args.insert("user".to_string(), "GitHub username".to_string());
        let tool = make_skill_tool(
            "profile",
            "http",
            "https://api.example.com/users/{{user}}",
            args,
        );
        let ht = SkillHttpTool::new("demo", &tool);
        assert_eq!(ht.url_template(), "https://api.example.com/users/{{user}}");
    }

    #[test]
    fn non_http_url_rejected() {
        let tool = make_skill_tool("bad", "http", "ftp://evil.com/file", HashMap::new());
        let ht = SkillHttpTool::new("demo", &tool);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(ht.execute(json!({}))).unwrap();
        assert!(!result.success);
        assert_eq!(
            result.error.as_deref(),
            Some("Only http/https URLs are allowed")
        );
    }

    #[test]
    fn parameters_schema_includes_args() {
        let mut args = HashMap::new();
        args.insert("query".to_string(), "Search query".to_string());
        let tool = make_skill_tool(
            "search",
            "http",
            "https://api.example.com/search?q={{query}}",
            args,
        );
        let ht = SkillHttpTool::new("demo", &tool);
        let schema = ht.parameters_schema();

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["query"]["type"], "string");
        assert_eq!(schema["properties"]["query"]["description"], "Search query");
    }

    #[test]
    fn parameters_schema_empty_when_no_args() {
        let tool = make_skill_tool("ping", "http", "https://example.com/health", HashMap::new());
        let ht = SkillHttpTool::new("demo", &tool);
        let schema = ht.parameters_schema();

        assert_eq!(schema["type"], "object");
        assert!(schema["properties"].as_object().unwrap().is_empty());
    }
}

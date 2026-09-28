use rig::tool::{DynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;

use super::provider_name::{dynamic_tool_name, ToolNamespace};
use crate::skills::SkillTool;

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
        let tool_name = dynamic_tool_name(ToolNamespace::Skill, skill_name, &tool.name);
        let url = tool.command.clone();
        let args = tool.args.clone();
        Self {
            tool_name,
            url,
            args,
        }
    }

    pub fn into_dynamic(self) -> DynamicTool {
        let name = self.tool_name.clone();
        let schema = self.parameters();
        let tool = std::sync::Arc::new(self);
        DynamicTool::new(name, "Skill HTTP request", schema, move |_context, args| {
            let tool = std::sync::Arc::clone(&tool);
            Box::pin(async move { tool.run(args).await.map(ToolOutput::text) })
        })
    }

    fn parameters(&self) -> Value {
        let mut properties = serde_json::Map::new();
        for (key, desc) in &self.args {
            properties.insert(key.clone(), json!({"type": "string", "description": desc}));
        }
        json!({"type": "object", "properties": properties})
    }
    async fn run(&self, args: Value) -> Result<String, ToolExecutionError> {
        let mut url = self.url.clone();
        for key in self.args.keys() {
            if let Some(value) = args.get(key).and_then(|v| v.as_str()) {
                url = url.replace(&format!("{{{{{}}}", key), value);
            }
        }

        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(ToolExecutionError::invalid_args(
                "Only http/https URLs are allowed",
            ));
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build()
            .map_err(|e| {
                ToolExecutionError::network(format!("Failed to build HTTP client: {e}"))
            })?;
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| ToolExecutionError::network(format!("Request failed: {e}")))?;

        if !response.status().is_success() {
            return Err(ToolExecutionError::provider(format!(
                "HTTP {}",
                response.status()
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| ToolExecutionError::network(format!("Failed to read response: {e}")))?;
        let text = String::from_utf8_lossy(&bytes);
        let result = if text.len() > MAX_RESPONSE_BYTES {
            let mut boundary = MAX_RESPONSE_BYTES;
            while !text.is_char_boundary(boundary) {
                boundary -= 1;
            }
            format!(
                "{}...\n[response truncated at {} bytes]",
                &text[..boundary],
                MAX_RESPONSE_BYTES
            )
        } else {
            text.into_owned()
        };

        Ok(result)
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
    fn non_http_url_rejected() {
        let tool = make_skill_tool("bad", "http", "ftp://evil.com/file", HashMap::new());
        let ht = SkillHttpTool::new("demo", &tool);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let error = rt.block_on(ht.run(json!({}))).unwrap_err();
        assert_eq!(error.to_string(), "Only http/https URLs are allowed");
    }
}

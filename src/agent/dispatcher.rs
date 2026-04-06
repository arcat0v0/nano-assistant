//! Tool call dispatcher — parses LLM responses for tool invocations.
//!
//! Supports two modes:
//! - **Native**: provider returns structured `ToolCall` objects (OpenAI, Anthropic, Gemini).
//! - **XML**: provider embeds tool calls in `<antml:invoke>` tags in text output.

use crate::providers::{ChatMessage, ChatResponse};
use crate::tools::{Tool, ToolSpec};
use serde_json::Value;
use std::fmt::Write;

/// A parsed tool call ready for execution.
#[derive(Debug, Clone)]
pub struct ParsedToolCall {
    pub name: String,
    pub arguments: Value,
    pub tool_call_id: Option<String>,
}

/// Result of executing a tool.
#[derive(Debug, Clone)]
pub struct ToolExecutionResult {
    pub name: String,
    pub output: String,
    pub success: bool,
    pub tool_call_id: Option<String>,
}

/// Parses tool calls from LLM responses and formats results for the history.
pub trait ToolDispatcher: Send + Sync {
    fn parse_response(&self, response: &ChatResponse) -> (String, Vec<ParsedToolCall>);
    fn format_results(&self, results: &[ToolExecutionResult]) -> ChatMessage;
    fn prompt_instructions(&self) -> String;
    fn should_send_tool_specs(&self) -> bool;
}

#[derive(Default)]
pub struct XmlToolDispatcher;

impl XmlToolDispatcher {
    const TAG_OPEN: &'static str = "<antml:invoke";
    const TAG_CLOSE: &'static str = "</antml:invoke>";

    fn strip_think_tags(s: &str) -> String {
        let mut result = String::with_capacity(s.len());
        let mut rest = s;
        loop {
            if let Some(start) = rest.find("💭") {
                result.push_str(&rest[..start]);
                if let Some(end) = rest[start..].find("💭") {
                    rest = &rest[start + end + "💭".len()..];
                } else {
                    break;
                }
            } else {
                result.push_str(rest);
                break;
            }
        }
        result
    }

    fn parse_xml_tool_calls(text: &str) -> (String, Vec<ParsedToolCall>) {
        let cleaned = Self::strip_think_tags(text);
        let mut text_parts = Vec::new();
        let mut calls = Vec::new();
        let mut remaining = cleaned.as_str();

        while let Some(start) = remaining.find(Self::TAG_OPEN) {
            let before = &remaining[..start];
            if !before.trim().is_empty() {
                text_parts.push(before.trim().to_string());
            }

            let after_open = &remaining[start + Self::TAG_OPEN.len()..];

            // Extract name from: <antml:invoke name="tool_name">
            let name = if let Some(name_start) = after_open.find("name=\"") {
                let after_name = &after_open[name_start + 6..];
                if let Some(name_end) = after_name.find('"') {
                    after_name[..name_end].to_string()
                } else {
                    remaining = &remaining[start + Self::TAG_OPEN.len()..];
                    continue;
                }
            } else {
                remaining = &remaining[start + Self::TAG_OPEN.len()..];
                continue;
            };

            // Find closing > of opening tag
            let tag_end = match after_open.find('>') {
                Some(pos) => pos,
                None => break,
            };

            // Find the closing tag
            let inner_start = start + Self::TAG_OPEN.len() + tag_end + 1;
            if let Some(close_pos) = remaining[inner_start..].find(Self::TAG_CLOSE) {
                let inner = remaining[inner_start..inner_start + close_pos].trim();
                let arguments = serde_json::from_str::<Value>(inner).unwrap_or_else(|e| {
                    tracing::warn!(tool = %name, error = %e, "Malformed tool call JSON");
                    Value::Object(serde_json::Map::new())
                });

                calls.push(ParsedToolCall {
                    name,
                    arguments,
                    tool_call_id: None,
                });

                remaining = &remaining[inner_start + close_pos + Self::TAG_CLOSE.len()..];
            } else {
                break;
            }
        }

        if !remaining.trim().is_empty() {
            text_parts.push(remaining.trim().to_string());
        }

        (text_parts.join("\n"), calls)
    }

    pub fn tool_specs(tools: &[Box<dyn Tool>]) -> Vec<ToolSpec> {
        tools.iter().map(|t| t.spec()).collect()
    }
}

impl ToolDispatcher for XmlToolDispatcher {
    fn parse_response(&self, response: &ChatResponse) -> (String, Vec<ParsedToolCall>) {
        let text = response.text_or_empty();
        Self::parse_xml_tool_calls(text)
    }

    fn format_results(&self, results: &[ToolExecutionResult]) -> ChatMessage {
        let mut content = String::from("[Tool results]\n");
        for result in results {
            let status = if result.success { "ok" } else { "error" };
            let _ = writeln!(
                content,
                "<tool_result name=\"{}\" status=\"{}\">\n{}\n</tool_result>",
                result.name, status, result.output
            );
        }
        ChatMessage::user(content)
    }

    fn prompt_instructions(&self) -> String {
        let mut instructions = String::new();
        instructions.push_str("## Tool Use Protocol\n\n");
        instructions.push_str(
            "To use a tool, wrap a JSON object in `antml:invoke name=\"tool_name\"` tags:\n\n",
        );
        instructions.push_str(
            "```\n\
             antml:invoke name=\"tool_name\"\n\
             {\"param\": \"value\"}\n\
             antml:invoke\n\
             ```\n\n",
        );
        instructions
    }

    fn should_send_tool_specs(&self) -> bool {
        false
    }
}

pub struct NativeToolDispatcher;

impl ToolDispatcher for NativeToolDispatcher {
    fn parse_response(&self, response: &ChatResponse) -> (String, Vec<ParsedToolCall>) {
        let text = response.text.clone().unwrap_or_default();
        let calls = response
            .tool_calls
            .iter()
            .map(|tc| ParsedToolCall {
                name: tc.name.clone(),
                arguments: serde_json::from_str(&tc.arguments).unwrap_or_else(|e| {
                    tracing::warn!(
                        tool = %tc.name,
                        error = %e,
                        "Failed to parse native tool call arguments"
                    );
                    Value::Object(serde_json::Map::new())
                }),
                tool_call_id: Some(tc.id.clone()),
            })
            .collect();
        (text, calls)
    }

    fn format_results(&self, results: &[ToolExecutionResult]) -> ChatMessage {
        let mut parts = Vec::new();
        for result in results {
            let id = result.tool_call_id.as_deref().unwrap_or("unknown");
            let payload = serde_json::json!({
                "tool_call_id": id,
                "content": result.output,
            });
            parts.push(ChatMessage::tool(payload.to_string()));
        }
        parts
            .into_iter()
            .next()
            .unwrap_or_else(|| ChatMessage::tool("{}"))
    }

    fn prompt_instructions(&self) -> String {
        String::new()
    }

    fn should_send_tool_specs(&self) -> bool {
        true
    }
}

/// Select the appropriate dispatcher based on provider capabilities.
pub fn create_dispatcher(native_tool_calling: bool) -> Box<dyn ToolDispatcher> {
    if native_tool_calling {
        Box::new(NativeToolDispatcher)
    } else {
        Box::new(XmlToolDispatcher)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ToolCall;

    fn make_response(text: &str) -> ChatResponse {
        ChatResponse {
            text: Some(text.into()),
            tool_calls: vec![],
        }
    }

    #[test]
    fn xml_parses_single_tool_call() {
        let response = make_response(
            "Checking\n<antml:invoke name=\"shell\">\n{\"command\":\"ls\"}\n</antml:invoke>",
        );
        let dispatcher = XmlToolDispatcher;
        let (_, calls) = dispatcher.parse_response(&response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(calls[0].arguments["command"], "ls");
    }

    #[test]
    fn xml_extracts_text_around_tool_calls() {
        let response = make_response(
            "Let me check.\n<antml:invoke name=\"read\">\n{\"path\":\"a.txt\"}\n</antml:invoke>\nDone.",
        );
        let dispatcher = XmlToolDispatcher;
        let (text, calls) = dispatcher.parse_response(&response);
        assert_eq!(calls.len(), 1);
        assert!(text.contains("Let me check"));
        assert!(text.contains("Done"));
    }

    #[test]
    fn xml_no_calls_returns_full_text() {
        let response = make_response("Just a regular response");
        let dispatcher = XmlToolDispatcher;
        let (text, calls) = dispatcher.parse_response(&response);
        assert!(calls.is_empty());
        assert_eq!(text, "Just a regular response");
    }

    #[test]
    fn xml_malformed_json_defaults_to_empty_args() {
        let response = make_response("<antml:invoke name=\"shell\">\nnot json\n</antml:invoke>");
        let dispatcher = XmlToolDispatcher;
        let (_, calls) = dispatcher.parse_response(&response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments, Value::Object(serde_json::Map::new()));
    }

    #[test]
    fn xml_strips_think_tags() {
        let response = make_response("💭I should list files💭\n<antml:invoke name=\"shell\">\n{\"command\":\"ls\"}\n</antml:invoke>");
        let dispatcher = XmlToolDispatcher;
        let (text, calls) = dispatcher.parse_response(&response);
        assert_eq!(calls.len(), 1);
        assert!(!text.contains("💭"));
    }

    #[test]
    fn native_parses_tool_calls_from_response() {
        let response = ChatResponse {
            text: Some("ok".into()),
            tool_calls: vec![ToolCall {
                id: "tc1".into(),
                name: "file_read".into(),
                arguments: "{\"path\":\"a.txt\"}".into(),
            }],
        };
        let dispatcher = NativeToolDispatcher;
        let (_, calls) = dispatcher.parse_response(&response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool_call_id.as_deref(), Some("tc1"));
        assert_eq!(calls[0].arguments["path"], "a.txt");
    }

    #[test]
    fn xml_format_results_contains_tool_result_tags() {
        let dispatcher = XmlToolDispatcher;
        let msg = dispatcher.format_results(&[ToolExecutionResult {
            name: "shell".into(),
            output: "file1.txt\nfile2.txt".into(),
            success: true,
            tool_call_id: None,
        }]);
        assert_eq!(msg.role, "user");
        assert!(msg.content.contains("<tool_result"));
        assert!(msg.content.contains("name=\"shell\""));
        assert!(msg.content.contains("status=\"ok\""));
    }

    #[test]
    fn xml_format_results_includes_error_status() {
        let dispatcher = XmlToolDispatcher;
        let msg = dispatcher.format_results(&[ToolExecutionResult {
            name: "shell".into(),
            output: "permission denied".into(),
            success: false,
            tool_call_id: None,
        }]);
        assert!(msg.content.contains("status=\"error\""));
    }

    #[test]
    fn native_format_results_returns_tool_message() {
        let dispatcher = NativeToolDispatcher;
        let msg = dispatcher.format_results(&[ToolExecutionResult {
            name: "shell".into(),
            output: "hello".into(),
            success: true,
            tool_call_id: Some("tc-1".into()),
        }]);
        assert_eq!(msg.role, "tool");
        let parsed: Value = serde_json::from_str(&msg.content).unwrap();
        assert_eq!(parsed["tool_call_id"], "tc-1");
        assert_eq!(parsed["content"], "hello");
    }

    #[test]
    fn create_dispatcher_selects_native() {
        let d = create_dispatcher(true);
        assert!(d.should_send_tool_specs());
    }

    #[test]
    fn create_dispatcher_selects_xml() {
        let d = create_dispatcher(false);
        assert!(!d.should_send_tool_specs());
    }
}

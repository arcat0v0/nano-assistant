use crate::tools::ToolSpec;
use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

pub type BoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + Send + 'a>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
    pub fn tool(content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

impl ChatResponse {
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
    pub fn text_or_empty(&self) -> &str {
        self.text.as_deref().unwrap_or("")
    }
}

#[derive(Debug, Clone)]
pub struct StreamChunk {
    pub delta: String,
    pub is_final: bool,
}

impl StreamChunk {
    pub fn delta(text: impl Into<String>) -> Self {
        Self {
            delta: text.into(),
            is_final: false,
        }
    }
    pub fn final_chunk() -> Self {
        Self {
            delta: String::new(),
            is_final: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatRequest<'a> {
    pub messages: &'a [ChatMessage],
    pub tools: Option<&'a [ToolSpec]>,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderCapabilities {
    pub native_tool_calling: bool,
    pub streaming: bool,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::default()
    }

    fn supports_native_tools(&self) -> bool {
        self.capabilities().native_tool_calling
    }

    fn supports_streaming(&self) -> bool {
        self.capabilities().streaming
    }

    /// Unified chat entry point. Always sends full message history.
    /// Provider implementations should override `chat_with_history` to handle API-specific mapping.
    async fn chat(
        &self,
        request: ChatRequest<'_>,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<ChatResponse> {
        let text = self
            .chat_with_history(request.messages, model, temperature)
            .await?;
        Ok(ChatResponse {
            text: Some(text),
            tool_calls: vec![],
        })
    }

    async fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<ChatResponse> {
        if self.supports_native_tools() {
            anyhow::bail!(
                "provider declares native tool support but did not override chat_with_tools()"
            )
        }
        let _ = tools;
        self.chat(
            ChatRequest {
                messages,
                tools: None,
            },
            model,
            temperature,
        )
        .await
    }

    fn stream_chat(
        &self,
        _messages: &[ChatMessage],
        _model: &str,
        _temperature: f64,
    ) -> BoxStream<'static, anyhow::Result<StreamChunk>> {
        Box::pin(futures::stream::empty())
    }

    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String>;

    async fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let system = messages
            .iter()
            .find(|m| m.role == "system")
            .map(|m| m.content.as_str());
        let last_user = messages
            .iter()
            .rfind(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .unwrap_or("");
        self.chat_with_system(system, last_user, model, temperature)
            .await
    }

    async fn warmup(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_message_constructors() {
        assert_eq!(ChatMessage::system("s").role, "system");
        assert_eq!(ChatMessage::user("u").role, "user");
        assert_eq!(ChatMessage::assistant("a").role, "assistant");
        assert_eq!(ChatMessage::tool("t").role, "tool");
    }

    #[test]
    fn chat_response_helpers() {
        let empty = ChatResponse {
            text: None,
            tool_calls: vec![],
        };
        assert!(!empty.has_tool_calls());
        assert_eq!(empty.text_or_empty(), "");

        let with_tools = ChatResponse {
            text: Some("check".into()),
            tool_calls: vec![ToolCall {
                id: "1".into(),
                name: "s".into(),
                arguments: "{}".into(),
            }],
        };
        assert!(with_tools.has_tool_calls());
        assert_eq!(with_tools.text_or_empty(), "check");
    }

    #[test]
    fn tool_call_serialization() {
        let tc = ToolCall {
            id: "c1".into(),
            name: "f".into(),
            arguments: r#"{"x":1}"#.into(),
        };
        let json = serde_json::to_string(&tc).unwrap();
        assert!(json.contains("c1") && json.contains("f"));
    }

    #[test]
    fn provider_capabilities_default() {
        let caps = ProviderCapabilities::default();
        assert!(!caps.native_tool_calling);
        assert!(!caps.streaming);
    }

    #[test]
    fn stream_chunk_constructors() {
        assert!(!StreamChunk::delta("hi").is_final);
        assert!(StreamChunk::final_chunk().is_final);
    }
}

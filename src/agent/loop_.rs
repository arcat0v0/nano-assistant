//! Core agent loop — the turn-based execution engine.
//!
//! Flow: user input → system prompt → LLM call → tool execution → repeat → final response.
//! Simplified from ZeroClaw's `agent.rs` turn() (~160 lines) without caching, observers,
//! model switching, or context compression.

use crate::agent::dispatcher::{create_dispatcher, ToolDispatcher, ToolExecutionResult};
use crate::agent::prompt::{PromptContext, SystemPromptBuilder};
use crate::agent::streaming::StreamOutputEvent;
use crate::config::Config;
use crate::memory::Memory;
use crate::providers::{ChatMessage, ChatRequest, ChatResponse, Provider};
use crate::skills::Skill;
use crate::tools::{Tool, ToolSpec};
use anyhow::{bail, Result};
use std::sync::Arc;

const STREAM_TOOL_MARKER_WINDOW_CHARS: usize = 512;
const TOOL_MARKERS: [&str; 2] = ["<antml:invoke", "\"tool_calls\""];

/// A single agent turn result.
#[derive(Debug, Clone)]
pub struct TurnResult {
    pub response: String,
    pub tool_calls_count: usize,
}

/// Conversation history for the agent.
#[derive(Debug, Clone, Default)]
pub struct ConversationHistory {
    messages: Vec<ChatMessage>,
}

impl ConversationHistory {
    pub fn new() -> Self {
        Self { messages: Vec::new() }
    }

    pub fn push(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
    }

    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn trim_to(&mut self, max_messages: usize) {
        if self.messages.len() > max_messages {
            let start = self.messages.len() - max_messages;
            // Always keep the system message (first one) if present
            if !self.messages.is_empty() && self.messages[0].role == "system" && start > 1 {
                // Keep system message + (max_messages - 1) most recent messages
                let non_system_start = self.messages.len() - (max_messages - 1);
                let mut trimmed = vec![self.messages[0].clone()];
                trimmed.extend_from_slice(&self.messages[non_system_start..]);
                self.messages = trimmed;
            } else {
                self.messages = self.messages[start..].to_vec();
            }
        }
    }
}

/// The agent — orchestrates LLM calls, tool dispatch, and conversation history.
pub struct Agent {
    provider: Arc<dyn Provider>,
    tools: Vec<Box<dyn Tool>>,
    tool_specs: Vec<ToolSpec>,
    memory: Option<Arc<dyn Memory>>,
    config: Config,
    history: ConversationHistory,
    dispatcher: Box<dyn ToolDispatcher>,
    last_visible_len: usize,
    skills: Vec<Skill>,
}

impl Agent {
    pub fn new(
        provider: Arc<dyn Provider>,
        tools: Vec<Box<dyn Tool>>,
        memory: Option<Arc<dyn Memory>>,
        config: Config,
    ) -> Self {
        Self::with_skills(provider, tools, memory, config, Vec::new())
    }

    pub fn with_skills(
        provider: Arc<dyn Provider>,
        tools: Vec<Box<dyn Tool>>,
        memory: Option<Arc<dyn Memory>>,
        config: Config,
        skills: Vec<Skill>,
    ) -> Self {
        let tool_specs: Vec<ToolSpec> = tools.iter().map(|t| t.spec()).collect();
        let dispatcher = create_dispatcher(provider.supports_native_tools());

        Self {
            provider,
            tools,
            tool_specs,
            memory,
            config,
            history: ConversationHistory::new(),
            dispatcher,
            last_visible_len: 0,
            skills,
        }
    }

    /// Execute one agent turn with the given user message.
    ///
    /// 1. Build system prompt (if first turn)
    /// 2. Load memory context (if memory available)
    /// 3. Call LLM
    /// 4. If tool calls → execute → append results → loop
    /// 5. Return final text response
    pub async fn turn(&mut self, user_message: &str) -> Result<TurnResult> {
        if self.history.is_empty() {
            let system_prompt = self.build_system_prompt();
            self.history.push(ChatMessage::system(system_prompt));
        }

        let enriched = self.enrich_user_message(user_message).await;
        self.history.push(ChatMessage::user(enriched));

        let model = self.config.provider.model.clone().unwrap_or_else(|| "gpt-4o-mini".into());
        let temperature = self.config.provider.temperature;
        let max_iterations = self.config.behavior.max_iterations;

        let mut total_tool_calls = 0;

        for _ in 0..max_iterations {
            let messages = self.history.messages();
            let response = self.call_llm(messages, &model, temperature).await?;

            let (text, calls) = self.dispatcher.parse_response(&response);

            if calls.is_empty() {
                let final_text = if text.is_empty() {
                    response.text_or_empty().to_string()
                } else {
                    text
                };

                self.history.push(ChatMessage::assistant(final_text.clone()));
                self.trim_history();

                return Ok(TurnResult {
                    response: final_text,
                    tool_calls_count: total_tool_calls,
                });
            }

            if !text.is_empty() {
                self.history.push(ChatMessage::assistant(text));
            }

            let results = self.execute_tools(&calls).await?;
            total_tool_calls += results.len();

            let result_msg = self.dispatcher.format_results(&results);
            self.history.push(result_msg);

            self.trim_history();
        }

        bail!(
            "Agent exceeded maximum tool iterations ({})",
            max_iterations
        )
    }

    /// Execute one agent turn with streaming.
    ///
    /// Returns the accumulated response text. Tool calls within the loop
    /// are still non-streamed (tools need full arguments before execution).
    pub async fn turn_streamed(
        &mut self,
        user_message: &str,
        mut on_chunk: impl FnMut(StreamOutputEvent),
    ) -> Result<TurnResult> {
        if self.history.is_empty() {
            let system_prompt = self.build_system_prompt();
            self.history.push(ChatMessage::system(system_prompt));
        }

        let enriched = self.enrich_user_message(user_message).await;
        self.history.push(ChatMessage::user(enriched));

        let model = self.config.provider.model.clone().unwrap_or_else(|| "gpt-4o-mini".into());
        let temperature = self.config.provider.temperature;
        let max_iterations = self.config.behavior.max_iterations;
        let mut total_tool_calls = 0;

        for _ in 0..max_iterations {
            let messages = self.history.messages();

            if self.provider.supports_streaming() {
                let mut accumulated = String::new();
                let mut stream = self.provider.stream_chat(messages, &model, temperature);
                let mut suppress_forwarding = false;
                let mut marker_window = String::new();
                let mut pending_visible = String::new();

                use futures::StreamExt;
                while let Some(chunk_result) = stream.next().await {
                    match chunk_result {
                        Ok(chunk) => {
                            if chunk.is_final {
                                break;
                            }
                            accumulated.push_str(&chunk.delta);

                            marker_window.push_str(&chunk.delta);
                            if marker_window.len() > STREAM_TOOL_MARKER_WINDOW_CHARS {
                                let keep_from = marker_window.len() - STREAM_TOOL_MARKER_WINDOW_CHARS;
                                let boundary = marker_window
                                    .char_indices()
                                    .find(|(idx, _)| *idx >= keep_from)
                                    .map_or(0, |(idx, _)| idx);
                                marker_window.drain(..boundary);
                            }

                            if !suppress_forwarding && {
                                let lowered = marker_window.to_ascii_lowercase();
                                lowered.contains("<antml:invoke")
                                    || lowered.contains("\"tool_calls\"")
                            } {
                                suppress_forwarding = true;
                                pending_visible.clear();
                                if self.last_visible_len > 0 {
                                    on_chunk(StreamOutputEvent::Clear);
                                    self.last_visible_len = 0;
                                }
                            }

                            if suppress_forwarding || chunk.delta.is_empty() {
                                continue;
                            }

                            pending_visible.push_str(&chunk.delta);
                            let reserve = trailing_tool_prefix_len(&pending_visible);
                            let emit_len = pending_visible.len().saturating_sub(reserve);
                            if emit_len == 0 {
                                continue;
                            }

                            let emit_text = pending_visible[..emit_len].to_string();
                            pending_visible.drain(..emit_len);

                            if self.last_visible_len == 0 {
                                on_chunk(StreamOutputEvent::Clear);
                            }

                            self.last_visible_len += emit_text.len();
                            on_chunk(StreamOutputEvent::Content(emit_text));
                        }
                        Err(e) => return Err(e),
                    }
                }

                if !suppress_forwarding && !pending_visible.is_empty() {
                    if self.last_visible_len == 0 {
                        on_chunk(StreamOutputEvent::Clear);
                    }
                    self.last_visible_len += pending_visible.len();
                    on_chunk(StreamOutputEvent::Content(pending_visible));
                }
                self.last_visible_len = 0;

                let fake_response = ChatResponse {
                    text: Some(accumulated.clone()),
                    tool_calls: vec![],
                };
                let (text, calls) = self.dispatcher.parse_response(&fake_response);

                if calls.is_empty() {
                    self.history.push(ChatMessage::assistant(accumulated.clone()));
                    self.trim_history();
                    return Ok(TurnResult {
                        response: accumulated,
                        tool_calls_count: total_tool_calls,
                    });
                }

                if !text.is_empty() {
                    self.history.push(ChatMessage::assistant(text));
                }

                // Notify about tool execution
                for call in &calls {
                    let summary = crate::console::args_summary(&call.name, &call.arguments);
                    on_chunk(StreamOutputEvent::Progress(format!(
                        "\n{}",
                        crate::console::format_tool_pending(&call.name, &summary)
                    )));
                }
                let results = self.execute_tools(&calls).await?;
                total_tool_calls += results.len();
                for (call, result) in calls.iter().zip(results.iter()) {
                    let summary = crate::console::args_summary(&call.name, &call.arguments);
                    on_chunk(StreamOutputEvent::Progress(
                        crate::console::format_tool_call_line(&result.name, &summary, result.success),
                    ));
                }

                let result_msg = self.dispatcher.format_results(&results);
                self.history.push(result_msg);
                self.trim_history();
            } else {
                let response = self.call_llm(messages, &model, temperature).await?;
                let (text, calls) = self.dispatcher.parse_response(&response);

                if calls.is_empty() {
                    let final_text = if text.is_empty() {
                        response.text_or_empty().to_string()
                    } else {
                        text
                    };
                    on_chunk(StreamOutputEvent::Content(final_text.clone()));
                    self.history.push(ChatMessage::assistant(final_text.clone()));
                    self.trim_history();
                    return Ok(TurnResult {
                        response: final_text,
                        tool_calls_count: total_tool_calls,
                    });
                }

                if !text.is_empty() {
                    self.history.push(ChatMessage::assistant(text));
                }

                let results = self.execute_tools(&calls).await?;
                total_tool_calls += results.len();
                let result_msg = self.dispatcher.format_results(&results);
                self.history.push(result_msg);
                self.trim_history();
            }
        }

        bail!(
            "Agent exceeded maximum tool iterations ({})",
            max_iterations
        )
    }

    /// Clear conversation history, starting fresh on next turn.
    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    /// Get a reference to the conversation history.
    pub fn history(&self) -> &[ChatMessage] {
        self.history.messages()
    }

    fn build_system_prompt(&self) -> String {
        let ctx = PromptContext {
            tools: &self.tools,
            tool_specs: &self.tool_specs,
            native_tool_calling: self.provider.supports_native_tools(),
            dispatcher_instructions: &self.dispatcher.prompt_instructions(),
            skills: &self.skills,
        };
        SystemPromptBuilder::build(&ctx)
    }

    async fn enrich_user_message(&self, user_message: &str) -> String {
        let mut context = String::new();

        if let Some(ref memory) = self.memory {
            if let Ok(entries) = memory.query(user_message, 5, None).await {
                if !entries.is_empty() {
                    let parts: Vec<&str> = entries.iter().map(|e| e.content.as_str()).collect();
                    context = format!("[Relevant context]\n{}\n\n", parts.join("\n"));
                }
            }
        }

        format!("{context}{user_message}")
    }

    async fn call_llm(
        &self,
        messages: &[ChatMessage],
        model: &str,
        temperature: f64,
    ) -> Result<ChatResponse> {
        let tools = if self.dispatcher.should_send_tool_specs() {
            Some(self.tool_specs.as_slice())
        } else {
            None
        };
        self.provider
            .chat(ChatRequest { messages, tools }, model, temperature)
            .await
    }

    async fn execute_tools(
        &self,
        calls: &[crate::agent::dispatcher::ParsedToolCall],
    ) -> Result<Vec<ToolExecutionResult>> {
        let mut results = Vec::with_capacity(calls.len());

        for call in calls {
            let tool = self
                .tools
                .iter()
                .find(|t| t.name() == call.name);

            let result = match tool {
                Some(t) => match t.execute(call.arguments.clone()).await {
                    Ok(tool_result) => ToolExecutionResult {
                        name: call.name.clone(),
                        output: tool_result.output,
                        success: tool_result.success,
                        tool_call_id: call.tool_call_id.clone(),
                    },
                    Err(e) => ToolExecutionResult {
                        name: call.name.clone(),
                        output: format!("Tool execution error: {e}"),
                        success: false,
                        tool_call_id: call.tool_call_id.clone(),
                    },
                },
                None => ToolExecutionResult {
                    name: call.name.clone(),
                    output: format!("Unknown tool: {}", call.name),
                    success: false,
                    tool_call_id: call.tool_call_id.clone(),
                },
            };

            results.push(result);
        }

        Ok(results)
    }

    fn trim_history(&mut self) {
        let max = self.config.memory.max_messages;
        if max > 0 {
            self.history.trim_to(max);
        }
    }
}

fn trailing_tool_prefix_len(text: &str) -> usize {
    let lowered = text.to_ascii_lowercase();

    TOOL_MARKERS
        .iter()
        .flat_map(|marker| {
            (1..marker.len()).rev().find_map(|prefix_len| {
                let prefix = &marker[..prefix_len];
                lowered.ends_with(prefix).then_some(prefix_len)
            })
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BehaviorConfig, MemoryConfig, ProviderConfig};
    use crate::providers::ProviderCapabilities;
    use crate::tools::ToolResult;
    use async_trait::async_trait;

    struct MockProvider {
        native_tools: bool,
        streaming: bool,
    }

    #[async_trait]
    impl Provider for MockProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                native_tool_calling: self.native_tools,
                streaming: self.streaming,
            }
        }

        async fn chat_with_system(
            &self,
            _system: Option<&str>,
            _message: &str,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            Ok("mock response".into())
        }
    }

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "Echoes input" }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"msg": {"type": "string"}}})
        }
        async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
            let msg = args.get("msg").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ToolResult { success: true, output: msg.to_string(), error: None })
        }
    }

    fn make_config() -> Config {
        Config {
            provider: ProviderConfig {
                model: Some("mock".into()),
                temperature: 0.7,
                ..Default::default()
            },
            behavior: BehaviorConfig {
                max_iterations: 10,
                ..Default::default()
            },
            memory: MemoryConfig {
                max_messages: 100,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn agent_creates_with_xml_dispatcher_for_non_native() {
        let provider = Arc::new(MockProvider { native_tools: false, streaming: false });
        let agent = Agent::new(provider, vec![], None, make_config());
        assert!(!agent.dispatcher.should_send_tool_specs());
    }

    #[test]
    fn agent_creates_with_native_dispatcher_for_native() {
        let provider = Arc::new(MockProvider { native_tools: true, streaming: false });
        let agent = Agent::new(provider, vec![], None, make_config());
        assert!(agent.dispatcher.should_send_tool_specs());
    }

    #[test]
    fn agent_builds_system_prompt_on_first_turn() {
        let provider = Arc::new(MockProvider { native_tools: false, streaming: false });
        let agent = Agent::new(provider, vec![], None, make_config());
        assert!(agent.history.is_empty());
    }

    #[test]
    fn conversation_history_push_and_len() {
        let mut history = ConversationHistory::new();
        assert!(history.is_empty());
        history.push(ChatMessage::user("hello"));
        assert_eq!(history.len(), 1);
        assert_eq!(history.messages()[0].role, "user");
    }

    #[test]
    fn conversation_history_trim_preserves_system() {
        let mut history = ConversationHistory::new();
        history.push(ChatMessage::system("sys"));
        for i in 0..10 {
            history.push(ChatMessage::user(format!("msg {i}")));
        }
        history.trim_to(5);
        assert_eq!(history.messages()[0].role, "system");
        assert_eq!(history.len(), 5);
    }

    #[test]
    fn conversation_history_trim_without_system() {
        let mut history = ConversationHistory::new();
        for i in 0..10 {
            history.push(ChatMessage::user(format!("msg {i}")));
        }
        history.trim_to(3);
        assert_eq!(history.len(), 3);
        assert_eq!(history.messages()[0].content, "msg 7");
    }

    #[test]
    fn conversation_history_clear() {
        let mut history = ConversationHistory::new();
        history.push(ChatMessage::user("test"));
        history.clear();
        assert!(history.is_empty());
    }

    #[test]
    fn trailing_tool_prefix_detects_partial_tool_markup() {
        assert_eq!(trailing_tool_prefix_len("<"), 1);
        assert_eq!(trailing_tool_prefix_len("prefix <antml:i"), 8);
        assert_eq!(trailing_tool_prefix_len("{\"too"), 4);
    }

    #[test]
    fn trailing_tool_prefix_ignores_normal_text() {
        assert_eq!(trailing_tool_prefix_len("hello world"), 0);
    }
}

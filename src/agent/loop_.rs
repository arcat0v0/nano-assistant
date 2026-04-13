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
        Self {
            messages: Vec::new(),
        }
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
    system_info: Option<String>,
    // MCP support
    activated_tools: Option<Arc<std::sync::Mutex<crate::mcp::ActivatedToolSet>>>,
    deferred_tool_names: Vec<String>,
}

impl Agent {
    pub fn new(
        provider: Arc<dyn Provider>,
        tools: Vec<Box<dyn Tool>>,
        memory: Option<Arc<dyn Memory>>,
        config: Config,
    ) -> Self {
        Self::with_skills(provider, tools, memory, config, Vec::new(), None)
    }

    pub fn with_skills(
        provider: Arc<dyn Provider>,
        tools: Vec<Box<dyn Tool>>,
        memory: Option<Arc<dyn Memory>>,
        config: Config,
        skills: Vec<Skill>,
        system_info: Option<String>,
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
            system_info,
            activated_tools: None,
            deferred_tool_names: Vec::new(),
        }
    }

    /// Create an agent with MCP server support.
    pub async fn with_mcp(
        provider: Arc<dyn Provider>,
        mut tools: Vec<Box<dyn Tool>>,
        memory: Option<Arc<dyn Memory>>,
        config: Config,
        skills: Vec<Skill>,
        system_info: Option<String>,
    ) -> Self {
        let mut activated_tools = None;
        let mut deferred_tool_names = Vec::new();

        if config.mcp.enabled && !config.mcp.servers.is_empty() {
            match crate::mcp::McpRegistry::connect_all(&config.mcp.servers).await {
                Ok(registry) => {
                    let registry = std::sync::Arc::new(registry);

                    if config.mcp.deferred_loading {
                        let deferred = crate::mcp::DeferredMcpToolSet::from_registry(
                            std::sync::Arc::clone(&registry),
                        )
                        .await;
                        deferred_tool_names = deferred
                            .stubs
                            .iter()
                            .map(|s| s.prefixed_name.clone())
                            .collect();

                        let activated = std::sync::Arc::new(std::sync::Mutex::new(
                            crate::mcp::ActivatedToolSet::new(),
                        ));

                        tools.push(Box::new(crate::mcp::ToolSearchTool::new(
                            deferred,
                            std::sync::Arc::clone(&activated),
                        )));

                        activated_tools = Some(activated);
                    } else {
                        let names = registry.tool_names();
                        for name in names {
                            if let Some(def) = registry.get_tool_def(&name).await {
                                tools.push(Box::new(crate::mcp::McpToolWrapper::new(
                                    name,
                                    def,
                                    std::sync::Arc::clone(&registry),
                                )));
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("MCP registry connection failed: {e:#}");
                }
            }
        }

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
            system_info,
            activated_tools,
            deferred_tool_names,
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
        self.debug_log(format!(
            "turn start streaming=false model={} history={} max_iterations={} user_chars={}",
            self.config
                .provider
                .model
                .as_deref()
                .unwrap_or("gpt-4o-mini"),
            self.history.len(),
            self.config.behavior.max_iterations,
            user_message.chars().count()
        ));
        if self.history.is_empty() {
            let system_prompt = self.build_system_prompt();
            self.debug_log(format!(
                "system prompt initialized chars={}",
                system_prompt.chars().count()
            ));
            self.history.push(ChatMessage::system(system_prompt));
        }

        let enriched = self.enrich_user_message(user_message).await;
        self.history.push(ChatMessage::user(enriched));

        let model = self
            .config
            .provider
            .model
            .clone()
            .unwrap_or_else(|| "gpt-4o-mini".into());
        let temperature = self.config.provider.temperature;
        let max_iterations = self.config.behavior.max_iterations;

        let mut total_tool_calls = 0;

        for iteration in 0..max_iterations {
            self.debug_log(format!(
                "iteration {} start history_messages={}",
                iteration + 1,
                self.history.len()
            ));
            let messages = self.history.messages();
            let response = self.call_llm(messages, &model, temperature).await?;

            let (text, calls) = self.dispatcher.parse_response(&response);
            self.debug_log(format!(
                "iteration {} llm response text_chars={} tool_calls={}",
                iteration + 1,
                response.text_or_empty().chars().count(),
                calls.len()
            ));

            if calls.is_empty() {
                let final_text = if text.is_empty() {
                    response.text_or_empty().to_string()
                } else {
                    text
                };

                self.history
                    .push(ChatMessage::assistant(final_text.clone()));
                self.trim_history();
                self.debug_log(format!(
                    "turn complete final_chars={} total_tool_calls={}",
                    final_text.chars().count(),
                    total_tool_calls
                ));

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

        self.debug_log(format!(
            "turn failed max iterations exceeded limit={}",
            max_iterations
        ));
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
        self.debug_log(format!(
            "turn start streaming=true model={} history={} max_iterations={} user_chars={}",
            self.config
                .provider
                .model
                .as_deref()
                .unwrap_or("gpt-4o-mini"),
            self.history.len(),
            self.config.behavior.max_iterations,
            user_message.chars().count()
        ));
        if self.history.is_empty() {
            let system_prompt = self.build_system_prompt();
            self.debug_log(format!(
                "system prompt initialized chars={}",
                system_prompt.chars().count()
            ));
            self.history.push(ChatMessage::system(system_prompt));
        }

        let enriched = self.enrich_user_message(user_message).await;
        self.history.push(ChatMessage::user(enriched));

        let model = self
            .config
            .provider
            .model
            .clone()
            .unwrap_or_else(|| "gpt-4o-mini".into());
        let temperature = self.config.provider.temperature;
        let max_iterations = self.config.behavior.max_iterations;
        let mut total_tool_calls = 0;

        for iteration in 0..max_iterations {
            self.debug_log(format!(
                "iteration {} start history_messages={}",
                iteration + 1,
                self.history.len()
            ));
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
                                let keep_from =
                                    marker_window.len() - STREAM_TOOL_MARKER_WINDOW_CHARS;
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
                self.debug_log(format!(
                    "iteration {} streamed response text_chars={} tool_calls={}",
                    iteration + 1,
                    accumulated.chars().count(),
                    calls.len()
                ));

                if calls.is_empty() {
                    self.history
                        .push(ChatMessage::assistant(accumulated.clone()));
                    self.trim_history();
                    self.debug_log(format!(
                        "turn complete final_chars={} total_tool_calls={}",
                        accumulated.chars().count(),
                        total_tool_calls
                    ));
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
                        crate::console::format_tool_call_line(
                            &result.name,
                            &summary,
                            result.success,
                        ),
                    ));
                }

                let result_msg = self.dispatcher.format_results(&results);
                self.history.push(result_msg);
                self.trim_history();
            } else {
                let response = self.call_llm(messages, &model, temperature).await?;
                let (text, calls) = self.dispatcher.parse_response(&response);
                self.debug_log(format!(
                    "iteration {} llm response text_chars={} tool_calls={}",
                    iteration + 1,
                    response.text_or_empty().chars().count(),
                    calls.len()
                ));

                if calls.is_empty() {
                    let final_text = if text.is_empty() {
                        response.text_or_empty().to_string()
                    } else {
                        text
                    };
                    on_chunk(StreamOutputEvent::Content(final_text.clone()));
                    self.history
                        .push(ChatMessage::assistant(final_text.clone()));
                    self.trim_history();
                    self.debug_log(format!(
                        "turn complete final_chars={} total_tool_calls={}",
                        final_text.chars().count(),
                        total_tool_calls
                    ));
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

        self.debug_log(format!(
            "turn failed max iterations exceeded limit={}",
            max_iterations
        ));
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
            system_info: self.system_info.as_deref(),
            deferred_tool_names: &self.deferred_tool_names,
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

    /// Build tool specs including any activated MCP tools.
    fn current_tool_specs(&self) -> Vec<ToolSpec> {
        let mut specs: Vec<ToolSpec> = self.tools.iter().map(|t| t.spec()).collect();
        if let Some(at) = &self.activated_tools {
            for spec in at.lock().unwrap().tool_specs() {
                if !specs.iter().any(|s| s.name == spec.name) {
                    specs.push(spec);
                }
            }
        }
        specs
    }

    async fn call_llm(
        &self,
        messages: &[ChatMessage],
        model: &str,
        temperature: f64,
    ) -> Result<ChatResponse> {
        let specs = self.current_tool_specs();
        let tools = if self.dispatcher.should_send_tool_specs() {
            Some(specs.as_slice())
        } else {
            None
        };
        self.debug_log(format!(
            "llm request model={} temperature={} messages={} tools_sent={} total_tools={}",
            model,
            temperature,
            messages.len(),
            tools.map(|t| t.len()).unwrap_or(0),
            specs.len()
        ));
        let response = self
            .provider
            .chat(ChatRequest { messages, tools }, model, temperature)
            .await;
        match &response {
            Ok(resp) => self.debug_log(format!(
                "llm response received text_chars={} native_tool_calls={}",
                resp.text_or_empty().chars().count(),
                resp.tool_calls.len()
            )),
            Err(err) => self.debug_log(format!("llm request failed error={err:#}")),
        }
        response
    }

    async fn execute_tools(
        &mut self,
        calls: &[crate::agent::dispatcher::ParsedToolCall],
    ) -> Result<Vec<ToolExecutionResult>> {
        let mut results = Vec::with_capacity(calls.len());
        let mut needs_skill_rescan = false;
        let mut needs_mcp_reload = false;

        let skill_install_re =
            regex::Regex::new(r"(?:npx\s+)?skills\s+(add|install)\b").expect("valid regex");

        for call in calls {
            self.debug_log(format!(
                "tool call name={} args={}",
                call.name,
                Self::summarize_json(&call.arguments)
            ));
            // 1. Try static tool registry
            let static_tool = self.tools.iter().find(|t| t.name() == call.name);

            // 2. Try activated MCP tools (deferred loading)
            let activated_arc = if static_tool.is_none() {
                self.activated_tools
                    .as_ref()
                    .and_then(|at| at.lock().unwrap().get_resolved(&call.name))
            } else {
                None
            };

            let tool: Option<&dyn Tool> =
                static_tool.map(|t| t.as_ref()).or(activated_arc.as_deref());

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
            self.debug_log(format!(
                "tool result name={} success={} output_chars={} summary={}",
                result.name,
                result.success,
                result.output.chars().count(),
                Self::summarize_text(&result.output)
            ));

            // Post-execution hook detection
            if result.success {
                if call.name == "shell" {
                    if let Some(cmd) = call.arguments.get("command").and_then(|v| v.as_str()) {
                        if skill_install_re.is_match(cmd) {
                            needs_skill_rescan = true;
                        }
                    }
                }

                if call.name == "file_edit" {
                    let file_path = call
                        .arguments
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if file_path.ends_with("config.toml") {
                        let old_str = call
                            .arguments
                            .get("old_string")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let new_str = call
                            .arguments
                            .get("new_string")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if old_str.contains("[mcp")
                            || old_str.contains("[[mcp.servers]]")
                            || new_str.contains("[mcp")
                            || new_str.contains("[[mcp.servers]]")
                        {
                            needs_mcp_reload = true;
                        }
                    }
                }
            }

            results.push(result);
        }

        // Post-execution hooks
        if needs_skill_rescan {
            let skills_config = self.config.skills.clone();
            let new_names = self.rescan_skills(&skills_config);
            if !new_names.is_empty() {
                results.push(ToolExecutionResult {
                    name: "system".to_string(),
                    output: format!(
                        "[Auto-reload] New skills detected and loaded: {}",
                        new_names.join(", ")
                    ),
                    success: true,
                    tool_call_id: None,
                });
            }
        }

        if needs_mcp_reload {
            let reload_result = self.reload_mcp().await;
            let msg = match &reload_result {
                McpReloadResult::Success { new_servers } if !new_servers.is_empty() => {
                    format!(
                        "[Auto-reload] New MCP servers connected: {}",
                        new_servers.join(", ")
                    )
                }
                McpReloadResult::Success { .. } => {
                    "[Auto-reload] MCP config reloaded, no new servers found.".to_string()
                }
                McpReloadResult::PartialFailure { connected, failed } => {
                    let mut msg = String::from("[Auto-reload] MCP reload partial: ");
                    if !connected.is_empty() {
                        msg.push_str(&format!("connected: {}; ", connected.join(", ")));
                    }
                    for (name, err) in failed {
                        msg.push_str(&format!("failed {name}: {err}; "));
                    }
                    msg
                }
                McpReloadResult::Disabled => "[Auto-reload] MCP is disabled in config.".to_string(),
            };
            results.push(ToolExecutionResult {
                name: "system".to_string(),
                output: msg,
                success: true,
                tool_call_id: None,
            });
        }

        Ok(results)
    }

    fn debug_log(&self, message: impl AsRef<str>) {
        if self.config.behavior.debug {
            eprintln!("[debug] {}", message.as_ref());
        }
    }

    fn summarize_json(value: &serde_json::Value) -> String {
        let serialized = serde_json::to_string(value).unwrap_or_else(|_| "<invalid-json>".into());
        Self::summarize_text(&serialized)
    }

    fn summarize_text(text: &str) -> String {
        const LIMIT: usize = 240;
        let single_line = text.replace('\n', "\\n");
        let mut chars = single_line.chars();
        let summary: String = chars.by_ref().take(LIMIT).collect();
        if chars.next().is_some() {
            format!("{summary}...")
        } else {
            summary
        }
    }

    /// Rescan skills directories and add any newly discovered skills.
    /// Returns the names of newly added skills.
    pub fn rescan_skills(&mut self, config: &crate::config::SkillsConfig) -> Vec<String> {
        let fresh = crate::skills::load_skills(config);
        let existing_names: std::collections::HashSet<String> =
            self.skills.iter().map(|s| s.name.clone()).collect();

        let mut new_names = Vec::new();

        for skill in fresh {
            if !existing_names.contains(&skill.name) {
                let skill_tools = crate::skills::skills_to_tools(&[skill.clone()]);
                for tool in skill_tools {
                    let spec = tool.spec();
                    self.tools.push(tool);
                    self.tool_specs.push(spec);
                }
                new_names.push(skill.name.clone());
                self.skills.push(skill);
            }
        }

        new_names
    }

    /// Reload MCP configuration and connect to any new servers.
    pub async fn reload_mcp(&mut self) -> McpReloadResult {
        let config_path = crate::config::schema::default_config_path();
        let config_content = match std::fs::read_to_string(&config_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to read config for MCP reload: {e}");
                return McpReloadResult::Disabled;
            }
        };

        let config: crate::config::Config = match toml::from_str(&config_content) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to parse config for MCP reload: {e}");
                return McpReloadResult::Disabled;
            }
        };

        if !config.mcp.enabled || config.mcp.servers.is_empty() {
            return McpReloadResult::Disabled;
        }

        // Determine which servers are already connected by checking tool name prefixes
        let existing_prefixes: std::collections::HashSet<String> = {
            let mut prefixes = std::collections::HashSet::new();
            for tool in &self.tools {
                if let Some(prefix) = tool.name().split("__").next() {
                    prefixes.insert(prefix.to_string());
                }
            }
            for name in &self.deferred_tool_names {
                if let Some(prefix) = name.split("__").next() {
                    prefixes.insert(prefix.to_string());
                }
            }
            prefixes
        };

        let new_server_configs: Vec<_> = config
            .mcp
            .servers
            .iter()
            .filter(|s| !existing_prefixes.contains(&s.name))
            .cloned()
            .collect();

        if new_server_configs.is_empty() {
            return McpReloadResult::Success {
                new_servers: Vec::new(),
            };
        }

        match crate::mcp::McpRegistry::connect_all(&new_server_configs).await {
            Ok(registry) => {
                let registry = std::sync::Arc::new(registry);
                let mut connected = Vec::new();
                let mut failed: Vec<(String, String)> = Vec::new();

                for server_config in &new_server_configs {
                    let names = registry.tool_names();
                    let server_tools: Vec<_> = names
                        .iter()
                        .filter(|n| n.starts_with(&format!("{}__", server_config.name)))
                        .collect();

                    if server_tools.is_empty() {
                        failed.push((
                            server_config.name.clone(),
                            "no tools discovered".to_string(),
                        ));
                        continue;
                    }

                    for tool_name in server_tools {
                        if let Some(def) = registry.get_tool_def(tool_name).await {
                            let wrapper = crate::mcp::McpToolWrapper::new(
                                tool_name.clone(),
                                def,
                                std::sync::Arc::clone(&registry),
                            );
                            let spec = wrapper.spec();
                            self.tools.push(Box::new(wrapper));
                            self.tool_specs.push(spec);
                        }
                    }

                    connected.push(server_config.name.clone());
                }

                if failed.is_empty() {
                    McpReloadResult::Success {
                        new_servers: connected,
                    }
                } else {
                    McpReloadResult::PartialFailure { connected, failed }
                }
            }
            Err(e) => McpReloadResult::PartialFailure {
                connected: Vec::new(),
                failed: new_server_configs
                    .iter()
                    .map(|s| (s.name.clone(), e.to_string()))
                    .collect(),
            },
        }
    }

    fn trim_history(&mut self) {
        let max = self.config.memory.max_messages;
        if max > 0 {
            self.history.trim_to(max);
        }
    }
}

/// Result of reloading MCP server configuration.
#[derive(Debug)]
pub enum McpReloadResult {
    Success {
        new_servers: Vec<String>,
    },
    PartialFailure {
        connected: Vec<String>,
        failed: Vec<(String, String)>,
    },
    Disabled,
}

fn trailing_tool_prefix_len(text: &str) -> usize {
    let lowered = text.to_ascii_lowercase();

    TOOL_MARKERS
        .iter()
        .flat_map(|marker| {
            (1..marker.len()).rev().find(|prefix_len| {
                let prefix = &marker[..*prefix_len];
                lowered.ends_with(prefix)
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
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes input"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"msg": {"type": "string"}}})
        }
        async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
            let msg = args.get("msg").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ToolResult {
                success: true,
                output: msg.to_string(),
                error: None,
            })
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
        let provider = Arc::new(MockProvider {
            native_tools: false,
            streaming: false,
        });
        let agent = Agent::new(provider, vec![], None, make_config());
        assert!(!agent.dispatcher.should_send_tool_specs());
    }

    #[test]
    fn agent_creates_with_native_dispatcher_for_native() {
        let provider = Arc::new(MockProvider {
            native_tools: true,
            streaming: false,
        });
        let agent = Agent::new(provider, vec![], None, make_config());
        assert!(agent.dispatcher.should_send_tool_specs());
    }

    #[test]
    fn agent_builds_system_prompt_on_first_turn() {
        let provider = Arc::new(MockProvider {
            native_tools: false,
            streaming: false,
        });
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

    #[tokio::test]
    async fn file_edit_mcp_changes_trigger_auto_reload_notice() {
        let provider = Arc::new(MockProvider {
            native_tools: false,
            streaming: false,
        });
        let mut agent = Agent::new(
            provider,
            vec![Box::new(crate::tools::file_edit::FileEditTool::new())],
            None,
            make_config(),
        );

        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "mode = \"test\"\n").unwrap();

        let results = agent
            .execute_tools(&[crate::agent::dispatcher::ParsedToolCall {
                name: "file_edit".to_string(),
                arguments: serde_json::json!({
                    "path": config_path.to_string_lossy(),
                    "old_string": "mode = \"test\"\n",
                    "new_string": "[mcp]\nenabled = false\n"
                }),
                tool_call_id: None,
            }])
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "file_edit");
        assert_eq!(results[1].name, "system");
        assert!(results[1].output.contains("[Auto-reload] MCP"));
    }
}

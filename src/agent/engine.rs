use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use anyhow::Result;
use futures::StreamExt;
use rig::agent::model::ModelHandle;
use rig::agent::{
    AgentBuilder, AgentHook, CompletionCallAction, CompletionCallEvent, HookContext,
    InvalidToolCallAction, InvalidToolCallContext, MultiTurnStreamItem, RequestPatch,
    ToolCall as ToolCallEvent, ToolCallAction, ToolResultAction, ToolResultEvent,
};
use rig::message::{AssistantContent, Message, UserContent};
use rig::streaming::{StreamedAssistantContent, StreamedUserContent};
use rig::tool::{
    server::{ToolServer, ToolServerHandle},
    DynamicTool,
};
use tokio::sync::Mutex;

use crate::agent::prompt::{PromptContext, SystemPromptBuilder};
use crate::agent::streaming::StreamOutputEvent;
use crate::config::{Config, SkillsConfig};
use crate::mcp::{DeferredMcpToolSet, McpRegistry, McpToolWrapper, ToolSearchTool};
use crate::memory::Memory;
use crate::security::SecurityManager;
use crate::skills::Skill;
use crate::tools;

#[derive(Debug, Clone)]
pub struct TurnResult {
    pub response: String,
    pub tool_calls_count: usize,
}

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

fn normalized_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    match (absolute.parent(), absolute.file_name()) {
        (Some(parent), Some(name)) => parent
            .canonicalize()
            .map_or(absolute.clone(), |parent| parent.join(name)),
        _ => absolute,
    }
}

struct RuntimeState {
    handle: ToolServerHandle,
    skills: Vec<Skill>,
    system_info: Option<String>,
    config: Config,
    config_path: PathBuf,
    security: Arc<SecurityManager>,
    deferred_names: Vec<String>,
    deferred_sets: Vec<DeferredMcpToolSet>,
    activated_set: Arc<parking_lot::Mutex<crate::mcp::ActivatedToolSet>>,
    activated: HashSet<String>,
    connected_servers: HashSet<String>,
    preamble: String,
    preamble_dirty: bool,
}

impl RuntimeState {
    async fn add_tool(&mut self, tool: DynamicTool) {
        self.handle.add_dynamic_tool(tool).await;
    }

    async fn refresh_preamble(&mut self) {
        let tools = match self.handle.get_tool_defs(None).await {
            Ok(tools) => tools,
            Err(error) => {
                tracing::warn!("Could not refresh tool definitions: {error}");
                return;
            }
        };
        self.preamble = SystemPromptBuilder::build(&PromptContext {
            tools: &tools,
            config_path: &self.config_path,
            skills: &self.skills,
            system_info: self.system_info.as_deref(),
            deferred_tool_names: &self.deferred_names,
        });
        self.preamble_dirty = true;
    }

    async fn connect_mcp(
        &mut self,
        configs: &[crate::config::schema::McpServerConfig],
        deferred: bool,
    ) -> McpReloadResult {
        if configs.is_empty() {
            return McpReloadResult::Success {
                new_servers: Vec::new(),
            };
        }
        match McpRegistry::connect_all(configs).await {
            Ok(registry) => {
                let registry = Arc::new(registry);
                let mut connected = Vec::new();
                let mut failed = Vec::new();
                for server in configs {
                    let prefix = format!("{}__", server.name);
                    let names: Vec<String> = registry
                        .tool_names()
                        .into_iter()
                        .filter(|n| n.starts_with(&prefix))
                        .collect();
                    if names.is_empty() {
                        failed.push((server.name.clone(), "no tools discovered".to_string()));
                        continue;
                    }
                    connected.push(server.name.clone());
                    self.connected_servers.insert(server.name.clone());
                    if !deferred {
                        for name in names {
                            if let Some(def) = registry.get_tool_def(&name).await {
                                self.add_tool(
                                    McpToolWrapper::new(name, def, Arc::clone(&registry))
                                        .into_dynamic(),
                                )
                                .await;
                            }
                        }
                    }
                }
                if deferred && !connected.is_empty() {
                    let set = DeferredMcpToolSet::from_registry(Arc::clone(&registry)).await;
                    self.deferred_names
                        .extend(set.stubs.iter().map(|s| s.prefixed_name.clone()));
                    self.deferred_sets.push(set);
                    self.handle
                        .add_tool(ToolSearchTool::new_multi(
                            self.deferred_sets.clone(),
                            Arc::clone(&self.activated_set),
                        ))
                        .await;
                }
                if failed.is_empty() {
                    McpReloadResult::Success {
                        new_servers: connected,
                    }
                } else {
                    McpReloadResult::PartialFailure { connected, failed }
                }
            }
            Err(error) => McpReloadResult::PartialFailure {
                connected: Vec::new(),
                failed: configs
                    .iter()
                    .map(|s| (s.name.clone(), error.to_string()))
                    .collect(),
            },
        }
    }

    async fn activate_deferred(&mut self) {
        let names: Vec<String> = self
            .activated_set
            .lock()
            .tool_names()
            .into_iter()
            .filter(|name| !self.activated.contains(*name))
            .map(str::to_string)
            .collect();
        let mut pending = Vec::new();
        for name in names {
            if let Some(set) = self
                .deferred_sets
                .iter()
                .find(|set| set.get_by_name(&name).is_some())
            {
                if let Some(tool) = set.activate(&name) {
                    pending.push((name, tool));
                }
            }
        }
        let changed = !pending.is_empty();
        for (name, tool) in pending {
            self.activated.insert(name);
            self.add_tool(tool).await;
        }
        self.deferred_names
            .retain(|name| !self.activated.contains(name));
        if changed {
            self.refresh_preamble().await;
        }
    }

    async fn rescan_skills(&mut self, config: &SkillsConfig) -> Vec<String> {
        let fresh = crate::skills::load_skills(config);
        let existing: HashSet<_> = self.skills.iter().map(|s| s.name.clone()).collect();
        let mut added = Vec::new();
        for skill in fresh {
            if !existing.contains(&skill.name) {
                for tool in crate::skills::skills_to_tools(std::slice::from_ref(&skill)) {
                    self.add_tool(tool).await;
                }
                added.push(skill.name.clone());
                self.skills.push(skill);
            }
        }
        if !added.is_empty() {
            self.refresh_preamble().await;
        }
        added
    }

    async fn reload_mcp(&mut self) -> McpReloadResult {
        let content = match tokio::fs::read_to_string(&self.config_path).await {
            Ok(content) => content,
            Err(error) => {
                tracing::warn!("MCP config reload failed: {error}");
                return McpReloadResult::Disabled;
            }
        };
        let config: Config = match toml::from_str(&content) {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!("MCP config reload failed: {error}");
                return McpReloadResult::Disabled;
            }
        };
        if !config.mcp.enabled {
            return McpReloadResult::Disabled;
        }
        let new_configs: Vec<_> = config
            .mcp
            .servers
            .iter()
            .filter(|server| !self.connected_servers.contains(&server.name))
            .cloned()
            .collect();
        let result = self
            .connect_mcp(&new_configs, config.mcp.deferred_loading)
            .await;
        if !new_configs.is_empty() {
            self.refresh_preamble().await;
        }
        result
    }
}

#[derive(Clone)]
struct RuntimeHook(Arc<Mutex<RuntimeState>>);

impl AgentHook for RuntimeHook {
    async fn on_completion_call(
        &self,
        _ctx: &HookContext,
        _event: CompletionCallEvent<'_>,
    ) -> CompletionCallAction {
        let state = self.0.lock().await;
        if state.preamble_dirty {
            CompletionCallAction::patch(RequestPatch::new().preamble(state.preamble.clone()))
        } else {
            CompletionCallAction::continue_run()
        }
    }

    async fn on_invalid_tool_call(
        &self,
        _ctx: &HookContext,
        event: &InvalidToolCallContext,
    ) -> Option<InvalidToolCallAction> {
        let feedback = if !event
            .available_tools
            .iter()
            .any(|name| name == &event.tool_name)
        {
            format!(
                "Unknown tool '{}'. Available tools: {}",
                event.tool_name,
                event.available_tools.join(", ")
            )
        } else if !event
            .allowed_tools
            .iter()
            .any(|name| name == &event.tool_name)
        {
            format!(
                "Tool '{}' is not allowed by the current tool choice.",
                event.tool_name
            )
        } else {
            format!(
                "Invalid arguments for tool '{}'. Check its JSON schema.",
                event.tool_name
            )
        };
        Some(InvalidToolCallAction::retry(feedback))
    }

    async fn on_tool_call(&self, _ctx: &HookContext, event: ToolCallEvent<'_>) -> ToolCallAction {
        let args = match serde_json::from_str(event.args) {
            Ok(args) => args,
            Err(error) => return ToolCallAction::skip(format!("Invalid tool arguments: {error}")),
        };
        let security = Arc::clone(&self.0.lock().await.security);
        match security.authorize(event.tool_name, &args).await {
            Ok(()) => ToolCallAction::run(),
            Err(reason) => ToolCallAction::skip(reason),
        }
    }

    async fn on_tool_result(
        &self,
        _ctx: &HookContext,
        event: ToolResultEvent<'_>,
    ) -> ToolResultAction {
        if !event.raw_result.is_success() {
            return ToolResultAction::keep();
        }
        let mut state = self.0.lock().await;
        if event.tool_name == "tool_search" {
            state.activate_deferred().await;
        } else if event.tool_name == "shell" {
            if let Ok(args) = serde_json::from_str::<serde_json::Value>(event.args) {
                if let Some(command) = args.get("command").and_then(|v| v.as_str()) {
                    static SKILL_INSTALL: LazyLock<regex::Regex> = LazyLock::new(|| {
                        regex::Regex::new(r"(?:npx\s+)?skills\s+(add|install)\b")
                            .expect("valid skill command pattern")
                    });
                    if SKILL_INSTALL.is_match(command) {
                        let config = state.config.skills.clone();
                        state.rescan_skills(&config).await;
                    }
                }
            }
        } else if event.tool_name == "file_edit" || event.tool_name == "file_write" {
            if let Ok(args) = serde_json::from_str::<serde_json::Value>(event.args) {
                if args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .is_some_and(|path| {
                        normalized_path(Path::new(path)) == normalized_path(&state.config_path)
                    })
                {
                    state.reload_mcp().await;
                }
            }
        }
        ToolResultAction::keep()
    }
}

pub struct Agent {
    rig: rig::agent::Agent,
    state: Arc<Mutex<RuntimeState>>,
    memory: Option<Arc<dyn Memory>>,
    history: Vec<Message>,
    max_messages: usize,
    max_turns: usize,
    debug: bool,
}

impl Agent {
    pub async fn new(
        model: ModelHandle,
        tools: Vec<DynamicTool>,
        memory: Option<Arc<dyn Memory>>,
        config: Config,
        skills: Vec<Skill>,
        system_info: Option<String>,
        security: Arc<SecurityManager>,
        config_path: PathBuf,
    ) -> Self {
        let handle = ToolServer::new().run();
        tools::register_builtin_tools(&handle).await;
        let mut state = RuntimeState {
            handle: handle.clone(),
            skills,
            system_info,
            config: config.clone(),
            config_path,
            security,
            deferred_names: Vec::new(),
            deferred_sets: Vec::new(),
            activated_set: Arc::new(parking_lot::Mutex::new(crate::mcp::ActivatedToolSet::new())),
            activated: HashSet::new(),
            connected_servers: HashSet::new(),
            preamble: String::new(),
            preamble_dirty: false,
        };
        for tool in tools {
            state.add_tool(tool).await;
        }
        if config.mcp.enabled {
            if let McpReloadResult::PartialFailure { failed, .. } = state
                .connect_mcp(&config.mcp.servers, config.mcp.deferred_loading)
                .await
            {
                for (name, error) in failed {
                    tracing::warn!("MCP server {name}: {error}");
                }
            }
        }
        state.refresh_preamble().await;
        state.preamble_dirty = false;
        let preamble = state.preamble.clone();
        let state = Arc::new(Mutex::new(state));
        let rig = AgentBuilder::from_model_handle(model)
            .preamble(&preamble)
            .temperature(config.provider.temperature)
            .default_max_turns(config.behavior.max_iterations)
            .add_hook(RuntimeHook(Arc::clone(&state)))
            .tool_server_handle(handle)
            .build();
        Self {
            rig,
            state,
            memory,
            history: Vec::new(),
            max_messages: config.memory.max_messages,
            max_turns: config.behavior.max_iterations,
            debug: config.behavior.debug,
        }
    }

    async fn enriched_prompt(&self, user_message: &str) -> String {
        if let Some(memory) = &self.memory {
            if let Ok(entries) = memory.query(user_message, 5, None).await {
                if !entries.is_empty() {
                    let context = entries
                        .into_iter()
                        .map(|entry| entry.content)
                        .collect::<Vec<_>>()
                        .join("\n");
                    return format!("[Relevant context]\n{context}\n\n{user_message}");
                }
            }
        }
        user_message.to_string()
    }

    pub async fn turn(&mut self, user_message: &str) -> Result<TurnResult> {
        let prompt = self.enriched_prompt(user_message).await;
        if self.debug {
            eprintln!(
                "[debug] turn start history={} streaming=false",
                self.history.len()
            );
        }
        let response = self
            .rig
            .runner(prompt)
            .history(self.history.clone())
            .max_turns(self.max_turns)
            .max_invalid_tool_call_retries(self.max_turns.saturating_sub(1))
            .tool_concurrency(1)
            .run()
            .await?;
        Ok(self.finish(response))
    }

    pub async fn turn_streamed(
        &mut self,
        user_message: &str,
        mut on_chunk: impl FnMut(StreamOutputEvent),
    ) -> Result<TurnResult> {
        let prompt = self.enriched_prompt(user_message).await;
        if self.debug {
            eprintln!(
                "[debug] turn start history={} streaming=true",
                self.history.len()
            );
        }
        let mut stream = self
            .rig
            .runner(prompt)
            .history(self.history.clone())
            .max_turns(self.max_turns)
            .max_invalid_tool_call_retries(self.max_turns.saturating_sub(1))
            .tool_concurrency(1)
            .stream()
            .await;
        let mut final_response = None;
        let mut visible = false;
        while let Some(item) = stream.next().await {
            match item? {
                MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text)) => {
                    visible = true;
                    on_chunk(StreamOutputEvent::Content(text.text));
                }
                MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ToolCall {
                    tool_call,
                    ..
                }) => {
                    let args = crate::console::args_summary(
                        &tool_call.function.name,
                        &tool_call.function.arguments,
                    );
                    on_chunk(StreamOutputEvent::Progress(format!(
                        "{}\n",
                        crate::console::format_tool_pending(&tool_call.function.name, &args)
                    )));
                }
                MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
                    tool_result,
                    ..
                }) => {
                    on_chunk(StreamOutputEvent::Progress(format!(
                        "  {} {} result\n",
                        crate::console::dim_label("[tool]"),
                        crate::console::tool_name(&tool_result.name)
                    )));
                }
                MultiTurnStreamItem::ToolExecutionCommitted { .. } if visible => {
                    on_chunk(StreamOutputEvent::Clear);
                    visible = false;
                }
                MultiTurnStreamItem::ModelTurnRetried { .. } if visible => {
                    on_chunk(StreamOutputEvent::Clear);
                    visible = false;
                }
                MultiTurnStreamItem::FinalResponse(response) => final_response = Some(response),
                _ => {}
            }
        }
        let response = final_response
            .ok_or_else(|| anyhow::anyhow!("stream ended without a final response"))?;
        Ok(self.finish(response))
    }

    fn finish(&mut self, response: rig::agent::PromptResponse) -> TurnResult {
        let prior_len = self.history.len();
        let count = response.messages.as_ref().map_or(0, |messages| {
            messages
                .iter()
                .skip(prior_len)
                .filter_map(|message| match message {
                    Message::Assistant { content, .. } => Some(
                        content
                            .iter()
                            .filter(|item| matches!(item, AssistantContent::ToolCall(_)))
                            .count(),
                    ),
                    _ => None,
                })
                .sum()
        });
        if let Some(messages) = response.messages {
            self.history = messages;
            self.trim_history();
        }
        if self.debug {
            eprintln!("[debug] turn complete tool_calls={count}");
        }
        TurnResult {
            response: response.output,
            tool_calls_count: count,
        }
    }

    fn trim_history(&mut self) {
        let max = self.max_messages;
        if max == 0 || self.history.len() <= max {
            return;
        }
        let starts: Vec<usize> = self
            .history
            .iter()
            .enumerate()
            .filter_map(|(index, message)| match message {
                Message::User { content }
                    if content
                        .iter()
                        .any(|part| matches!(part, UserContent::Text(_))) =>
                {
                    Some(index)
                }
                _ => None,
            })
            .collect();
        if let Some(&latest) = starts.last() {
            let cutoff = starts
                .iter()
                .copied()
                .find(|&start| start >= self.history.len() - max)
                .unwrap_or(latest);
            if cutoff > 0 {
                self.history.drain(..cutoff);
            }
        }
    }

    pub fn history(&self) -> &[Message] {
        &self.history
    }
    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    pub async fn rescan_skills(&self, config: &SkillsConfig) -> Vec<String> {
        self.state.lock().await.rescan_skills(config).await
    }

    pub async fn reload_mcp(&self) -> McpReloadResult {
        self.state.lock().await.reload_mcp().await
    }
}

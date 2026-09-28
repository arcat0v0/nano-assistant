# Runtime and tool execution

`na chat` and the interactive CLI/TUI build one Rig 0.42 Agent per session. Rig owns provider message conversion, streaming events, tool-call IDs/results, and the multi-turn model/tool exchange. The application keeps only session concerns: the system prompt, Markdown memory, complete-turn history trimming, dynamic tool registration, and output rendering. `behavior.max_iterations` limits Rig turns; `memory.max_messages` limits retained history without separating a tool result from its originating call.

Compatibility: tool execution requires a model with native tool/function calling; the XML fallback is removed. Deploy the native-tool-aware `nana-hub` alongside clients using `free/*`: an older text-only Hub cannot handle native tool-call and result history.

Builtin tools are registered in Rig's live tool server. Skill and knowledge tools use dynamic Rig tools. MCP servers retain the configured stdio, HTTP, or SSE transport; with deferred loading, `tool_search` discovers tools and activates them in the same registry for the next model request. Installing a skill triggers a rescan; editing the active `config.toml` through `file_edit` or `file_write` reloads newly configured MCP servers. The policy hook checks all tool calls before execution, including activated tools. `direct`, `confirm`, and `whitelist` preserve their configured meanings; a denied tool call has no tool side effect and is visible to the model.

In streamed mode, completed Rig tool-call and tool-result events render brief progress on stderr, while assistant text stays on stdout. Confirmation prompts include the actual tool name and arguments/path, including skill and MCP tools. Whitelist matching still evaluates only the original shell command string.

`file_edit` requires exactly one nonempty `old_string` match. A missing or repeated match produces a structured tool error and leaves the file unchanged, so the model can revise its request. Builtin skill source files cannot be edited or overwritten. Shell and PTY execution remain local.

For `free/<slug>`, the Hub transport sends `<slug>` to `/v1/chat/completions` with the existing Ed25519 request signature and exact signed JSON body. Hub routes native tool calls but does not execute them; subsequent requests include the corresponding tool-call IDs and local results. The free route currently accepts text only and rejects image input. Use an isolated Hub, database, Redis, and mock upstream when validating this flow; production credentials are not required.

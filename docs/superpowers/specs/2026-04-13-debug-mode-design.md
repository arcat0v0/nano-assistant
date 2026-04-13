# DEBUG Mode Design

**Date:** 2026-04-13

## Goal

为 `nano-assistant` 增加一个“最全模式”的 DEBUG 开关，帮助调试 nana 的执行过程。DEBUG 信息统一输出到终端 `stderr`，不写文件，不改变正常响应的 `stdout` 渲染。

## Scope

本次仅实现单一布尔开关：

- CLI: `--debug`
- Config: `[behavior].debug = false`
- 优先级：CLI `--debug` > 配置文件 > 默认值

本次不做：

- 日志落盘
- 多级别日志（basic/full/trace）
- `RUST_LOG`/`tracing-subscriber` 体系化接入
- 完整 dump system prompt、完整对话历史、完整工具输出正文

## User Experience

用户可以通过以下方式开启 DEBUG：

```bash
na --debug "帮我看看当前目录"
na chat --debug
```

或在配置文件中：

```toml
[behavior]
debug = true
```

开启后，`stderr` 会输出高信号调试信息，包括：

- provider、model、security mode、streaming、max_iterations
- 每轮 agent iteration 开始/结束
- system prompt 初始化、history 条数
- 发起 LLM 调用前的摘要
- LLM 响应后的文本长度、tool call 数量
- 每个 tool call 的名称和参数 JSON 摘要
- 每个 tool result 的成功/失败、输出长度、错误摘要
- 达到最大迭代、provider/tool 错误等异常路径

正常回答仍走现有 `stdout`/渲染逻辑。

## Design

### 1. 配置层

在 `src/config/schema.rs` 的 `BehaviorConfig` 中新增：

- `debug: bool`

默认值为 `false`。

### 2. CLI 层

在 `src/cli/mod.rs` 的 chat 命令增加：

- `--debug`

`commands.rs` 中增加解析逻辑，将 CLI 值与配置值合并，得到最终 `debug_enabled`。

### 3. Runtime Debug Context

为避免全局 logger 改造，本次采用轻量本地方案：

- 在 agent 内部持有一个简单 `debug_enabled` 布尔值
- 新增若干内部辅助方法，统一向 `stderr` 输出 `[debug] ...`

这能覆盖 agent 主循环、LLM 调用、tool 执行等关键路径，改动最小，风险最低。

### 4. Agent 侧观测点

在 `src/agent/loop_.rs` 增加调试输出点：

- turn/turn_streamed 开始时输出运行配置摘要
- 首次注入 system prompt 时输出长度/状态
- 每轮 iteration 开始时输出序号与 history 大小
- `call_llm` 前输出消息数、工具数、model、temperature
- 收到响应后输出文本长度和 tool call 数
- `execute_tools` 前对每个 call 输出工具名和参数摘要
- `execute_tools` 后输出结果状态、输出长度
- 遇到错误和超出最大迭代时输出明确调试信息

### 5. 输出原则

DEBUG 输出只打摘要，不打完整正文，避免：

- shell 输出刷屏
- file_read 巨量文件内容污染终端
- 泄露过长 prompt/history

必要时可做简单截断，例如保留参数 JSON 的前若干字符。

## Files

- Modify: `src/config/schema.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/cli/commands.rs`
- Modify: `src/agent/loop_.rs`
- Update: `README.md`
- Add tests where practical in existing CLI/agent test modules

## Risks

- `stderr` 调试输出可能与流式进度提示交错
- 原有 `-v` 与 `--debug` 语义会并存，需要保持边界清晰
- Native tools 与 XML tools 两种 dispatcher 都要能拿到一致摘要

## Acceptance Criteria

- 未开启 DEBUG 时，行为与当前版本保持一致
- `na --debug "..."` 能看到 agent + tool 关键路径摘要
- 配置文件 `behavior.debug = true` 生效
- CLI `--debug` 能覆盖配置文件
- 单命令模式和交互模式均可用

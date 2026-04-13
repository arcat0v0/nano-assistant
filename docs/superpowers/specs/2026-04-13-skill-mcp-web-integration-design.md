# Skill / MCP / Web Integration Design

> nano-assistant 扩展设计：接入 skills.sh 生态、MCP 协议、内置 Web 工具、Domain Skills

## 1. 背景与目标

nano-assistant 当前是一个轻量 Rust CLI AI agent，拥有 6 个内置工具和 SKILL.md/SKILL.toml 格式的 skill 系统。本次扩展的目标是：

1. 兼容 skills.sh 生态（读取 `~/.agents/skills/` 目录）
2. 内置 Web Fetch 和 Web Search 工具
3. 实现 MCP client，接入 Exa、Context7、Grep.app 等 MCP server
4. 集成丰富的 domain skills（操作系统、软件、安全）

## 2. 设计原则

- **分层递进**：4 层实现，每层独立可测试、可交付
- **混合架构**：核心网络能力用 Rust 实现，domain knowledge 用 SKILL.md
- **最小依赖**：仅新增 `html2text` 一个 crate，MCP 复用现有 reqwest/tokio
- **非侵入兼容**：skills.sh 的 skill 原样读取，无需转换格式

## 3. Layer 1 — 基础设施层

### 3.1 Skill 搜索路径扩展

**改动文件**：`src/skills/mod.rs`、`src/config/schema.rs`

在 `SkillsConfig` 中新增 `extra_paths` 字段：

```toml
[skills]
enabled = true
# extra_paths 用于添加额外的自定义路径（可选）
# ~/.agents/skills 是硬编码的默认搜索路径，无需在此配置
extra_paths = ["/opt/my-custom-skills"]
```

加载流程（优先级从高到低）：

1. 加载主目录 `~/.config/nano-assistant/skills/`（最高优先级）
2. 扫描 `~/.agents/skills/`（硬编码默认路径，只要目录存在就自动扫描，不受 `extra_paths` 控制）
3. 依次扫描 `extra_paths` 中的自定义目录（最低优先级）
4. 同名 skill 以先加载的为准，后续同名 skill 被跳过
5. 所有路径的 skill 统一经过 `audit_skill_directory_with_options()` 安全审计

格式兼容：nano-assistant 已支持 SKILL.md（YAML frontmatter + markdown），和 skills.sh 格式一致。

预估改动量：~30-50 行。

### 3.2 内置 web_fetch 工具

**新增文件**：`src/tools/web_fetch.rs`

| 属性 | 值 |
|------|-----|
| 工具名 | `web_fetch` |
| 参数 | `url`（必填）、`max_length`（可选，默认 100KB） |
| 超时 | 30 秒 |
| 重定向 | 最多 5 次 |
| User-Agent | `nano-assistant/{version}` |
| 输出限制 | 截断到 1 MiB（与其他工具一致） |

处理逻辑：

- HTML 响应：使用 `html2text` crate 转为可读文本
- JSON / 纯文本：直接返回
- 二进制：返回错误提示（不支持）

### 3.3 内置 web_search 工具

**新增文件**：`src/tools/web_search.rs`

| 属性 | 值 |
|------|-----|
| 工具名 | `web_search` |
| 参数 | `query`（必填）、`max_results`（可选，默认 10） |
| 搜索引擎 | DuckDuckGo HTML (`https://html.duckduckgo.com/html/`) |
| 请求方式 | POST，form-urlencoded |
| 超时 | 15 秒 |
| 无需 API key | 免费无限制 |

输出格式：

```
1. [标题](URL)
   摘要文本...

2. [标题](URL)
   摘要文本...
```

从 HTML 中解析搜索结果（标题、URL、摘要），使用 `html2text` 或正则提取。

### 3.4 新增依赖

| Crate | 版本 | 用途 | 体积影响 |
|-------|------|------|---------|
| `html2text` | latest | HTML → 纯文本转换 | 纯 Rust，极小 |

`reqwest`（HTTP 客户端）和 `tokio`（异步运行时）已有，无需新增。

### 3.5 工具注册

在 `src/tools/mod.rs` 中与现有工具并列注册，成为第 7、8 个内置工具。

## 4. Layer 2 — MCP Client 层

### 4.1 模块结构

从 ZeroClaw (`../zeroclaw/src/tools/`) 移植，放在 `src/mcp/`：

| 文件 | 来源 | 行数 | 职责 |
|------|------|------|------|
| `mod.rs` | 新建 | ~20 | 模块声明和 re-export |
| `protocol.rs` | `mcp_protocol.rs` | ~240 | JSON-RPC 2.0 类型：`JsonRpcRequest`、`JsonRpcResponse`、`McpToolDef`、错误码 |
| `transport.rs` | `mcp_transport.rs` | ~1280 | `McpTransportConn` trait + 三种实现 |
| `client.rs` | `mcp_client.rs` | ~420 | `McpServer`（单连接管理）、`McpRegistry`（多 server 注册） |
| `tool.rs` | `mcp_tool.rs` | ~230 | `McpToolWrapper` 适配 nano-assistant `Tool` trait |
| `deferred.rs` | `mcp_deferred.rs` | ~550 | `DeferredMcpToolStub`、`DeferredMcpToolSet`、`ActivatedToolSet` |
| `tool_search.rs` | `tool_search.rs` | ~370 | `ToolSearchTool` — LLM 可调用的工具发现工具 |

### 4.2 传输层

三种 MCP 传输，全部复用现有依赖：

**Stdio**（本地进程）：
- `tokio::process::Command` 启动子进程
- stdin/stdout 通信，JSON-RPC 行分隔
- 最大行 4 MB，超时 30 秒
- `kill_on_drop(true)` 确保进程清理

**HTTP**（无状态 POST）：
- `reqwest::Client` 发送 JSON-RPC POST
- 支持 `Mcp-Session-Id` 跨请求维持会话
- 超时 120 秒

**SSE**（Server-Sent Events）：
- 后台任务维持持久 SSE 连接接收响应
- HTTP POST 发送请求到 message URL
- 支持 endpoint 自动发现（SSE stream 中的 `event: endpoint`）
- 请求/响应通过 `oneshot::channel` 匹配

### 4.3 适配改动

从 ZeroClaw 移植时需要调整的点：

1. **Tool trait 对齐**：`McpToolWrapper::execute()` 返回值映射到 nano-assistant 的 `ToolResult` 结构
2. **去掉 `approved` 字段**：ZeroClaw 有 ApprovalManager，nano-assistant 用 security mode（direct/confirm/whitelist），剥离审批逻辑
3. **客户端标识**：`initialize` 握手时 `clientInfo.name` = `"nano-assistant"`
4. **日志**：两者都用 `tracing`，无需改动

### 4.4 配置格式

在 `config.toml` 新增 `[mcp]` section：

```toml
[mcp]
enabled = true
deferred_loading = true

[[mcp.servers]]
name = "context7"
transport = "stdio"
command = "npx"
args = ["-y", "@upstash/context7-mcp@latest"]

[[mcp.servers]]
name = "exa"
transport = "stdio"
command = "npx"
args = ["-y", "exa-mcp-server"]
env = { "EXA_API_KEY" = "your-key" }

[[mcp.servers]]
name = "grep-app"
transport = "stdio"
command = "npx"
args = ["-y", "@anthropics/grep-app-mcp"]
```

每个 server 支持字段：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | String | 是 | 工具名前缀 |
| `transport` | "stdio" / "http" / "sse" | 否 | 默认 stdio |
| `command` | String | stdio 必填 | 启动命令 |
| `args` | Vec<String> | 否 | 命令参数 |
| `url` | String | http/sse 必填 | 服务 URL |
| `env` | Map<String, String> | 否 | 环境变量 |
| `headers` | Map<String, String> | 否 | HTTP 请求头 |
| `tool_timeout_secs` | u64 | 否 | 默认 180s，上限 600s |

### 4.5 Agent Loop 集成

**启动阶段**（`Agent::new()` 或类似入口）：

```
1. 读取 config.mcp
2. McpRegistry::connect_all() 连接所有 server（失败非致命）
3. if deferred_loading:
     注册 tool_search 工具
     生成 <available-deferred-tools> prompt 段
   else:
     注册所有 McpToolWrapper 为常规工具
```

**工具执行阶段**（`execute_tool()`）：

```
1. 在静态注册表中查找（内置工具 + skill 工具）
2. 如未找到，在 ActivatedToolSet 中查找（延迟激活的 MCP 工具）
3. 支持后缀匹配：LLM 可能省略 server 前缀，如 query_docs → context7__query_docs
4. 执行并返回结果
```

**系统 prompt 注入**（deferred_loading = true）：

```
## Available Deferred Tools
The following MCP tools are available but not yet activated.
Call tool_search to activate them before use.

<available-deferred-tools>
context7__resolve-library-id
context7__query-docs
exa__search
exa__get-contents
grep-app__search
</available-deferred-tools>
```

**并行执行限制**：当 tool_search 在批量调用中时，强制顺序执行（避免激活竞态）。

### 4.6 无新依赖

所有 MCP 通信复用：
- `reqwest`：HTTP/SSE
- `tokio`：异步运行时、子进程管理
- `serde_json`：JSON-RPC 序列化
- `async-trait`：异步 trait

## 5. Layer 3 — Domain Skills

### 5.1 复用现成 skill（从 skills.sh 安装）

通过 `npx skills add <package> -g -y` 安装到 `~/.agents/skills/`：

| Skill | 安装命令 | 覆盖领域 |
|-------|---------|---------|
| arch-linux-triage | `npx skills add github/awesome-copilot@arch-linux-triage -g -y` | Arch Linux 诊断管理 |
| debian-linux-triage | `npx skills add github/awesome-copilot@debian-linux-triage -g -y` | Debian/Ubuntu |
| fedora-linux-triage | `npx skills add github/awesome-copilot@fedora-linux-triage -g -y` | Fedora |
| centos-linux-triage | `npx skills add github/awesome-copilot@centos-linux-triage -g -y` | CentOS/RHEL |
| secure-linux-web-hosting | `npx skills add xixu-me/skills@secure-linux-web-hosting -g -y` | Linux 服务器部署 |
| docker | `npx skills add bobmatnyc/claude-mpm-skills@docker -g -y` | Docker 全面指南 |
| kubernetes | `npx skills add bobmatnyc/claude-mpm-skills@kubernetes -g -y` | K8s 管理 |
| nginx-configuration | `npx skills add aj-geddes/useful-ai-prompts@nginx-configuration -g -y` | Nginx 配置 |

安装后 nano-assistant 自动读取（Layer 1 实现了路径扩展）。

find-skills 已安装在 `~/.agents/skills/find-skills/`，自动可用。

### 5.2 自写 skill

放在项目 `skills/` 目录，安装时复制到 `~/.config/nano-assistant/skills/`。

#### database-admin（~8-10 KB）

```yaml
---
name: database-admin
description: 数据库管理综合指南，覆盖 PostgreSQL、MySQL/MariaDB、Redis、SQLite
version: 0.1.0
tags: [database, postgresql, mysql, redis, sqlite, admin]
---
```

覆盖内容：
- 各数据库安装与初始化（按发行版区分包管理器）
- 用户与权限管理
- 备份与恢复策略
- 性能调优（连接池、索引、查询优化）
- 常用运维命令速查
- 故障排查流程
- 复制与高可用基础配置

#### server-security（~6-8 KB）

```yaml
---
name: server-security
description: 服务器安全最佳实践，覆盖 SSH 加固、防火墙、fail2ban、审计、SSL/TLS
version: 0.1.0
tags: [security, ssh, firewall, ufw, nftables, fail2ban, ssl, hardening]
---
```

覆盖内容：
- SSH 加固 checklist（密钥认证、端口变更、禁止 root 登录）
- 防火墙配置（ufw 和 nftables，含常用规则模板）
- fail2ban 安装配置与自定义 jail
- 自动安全更新（unattended-upgrades / dnf-automatic）
- 审计日志（auditd 配置）
- SSL/TLS 证书管理（certbot / acme.sh）
- 端口与服务最小化原则
- 安全审计 checklist 与应急响应流程

#### container-orchestration（~5-6 KB）

```yaml
---
name: container-orchestration
description: 容器编排补充指南，覆盖 Docker Compose、Podman rootless、网络与卷管理
version: 0.1.0
tags: [docker, podman, compose, container, orchestration]
---
```

覆盖内容：
- Docker Compose 多服务编排（网络、卷、环境变量、健康检查）
- Podman rootless 部署（与 Docker 命令对照）
- 容器网络模型（bridge、host、macvlan）
- 卷管理策略（named、bind mount、tmpfs）
- 镜像构建最佳实践（多阶段构建、层缓存、.dockerignore）
- 日志与监控（docker logs、journald 集成）

## 6. Layer 4 — 增强层（后续迭代）

- Deferred loading prompt 优化（按 server 分组、按相关性排序）
- Skill 质量评估和智能推荐
- 更多 MCP server 扩展（按需添加）
- Skill 发布到 skills.sh

## 7. 不在范围内

- 浏览器类 MCP（agent-browser、crawl4ai、playwright）
- MCP resource 读取（仅做 tool 调用）
- skills.sh 发布流程
- 前端 / UI 相关 skill
- MCP server 端实现（仅做 client）

## 8. 实现顺序与依赖

```
Layer 1（基础设施）
├─ 1a. Skill 搜索路径扩展（无依赖）
├─ 1b. web_fetch 工具（依赖 html2text）
└─ 1c. web_search 工具（依赖 html2text）

Layer 2（MCP Client）— 依赖 Layer 1 完成
├─ 2a. protocol + transport 模块（纯移植）
├─ 2b. client + registry 模块
├─ 2c. tool wrapper + deferred loading
├─ 2d. tool_search 工具
└─ 2e. Agent loop 集成 + config 扩展

Layer 3（Domain Skills）— 依赖 Layer 1 完成（与 Layer 2 可并行）
├─ 3a. 安装现成 skills.sh skill
├─ 3b. 自写 database-admin skill
├─ 3c. 自写 server-security skill
└─ 3d. 自写 container-orchestration skill

Layer 4（增强）— 依赖 Layer 2 + 3 完成
└─ 后续迭代
```

## 9. 文件变更汇总

### 新增文件

| 文件 | 层级 | 说明 |
|------|------|------|
| `src/tools/web_fetch.rs` | L1 | web_fetch 内置工具 |
| `src/tools/web_search.rs` | L1 | web_search 内置工具 |
| `src/mcp/mod.rs` | L2 | MCP 模块入口 |
| `src/mcp/protocol.rs` | L2 | JSON-RPC 2.0 协议类型 |
| `src/mcp/transport.rs` | L2 | Stdio/HTTP/SSE 传输实现 |
| `src/mcp/client.rs` | L2 | McpServer + McpRegistry |
| `src/mcp/tool.rs` | L2 | McpToolWrapper |
| `src/mcp/deferred.rs` | L2 | 延迟加载机制 |
| `src/mcp/tool_search.rs` | L2 | tool_search 内置工具 |
| `skills/database-admin/SKILL.md` | L3 | 数据库管理 skill |
| `skills/server-security/SKILL.md` | L3 | 服务器安全 skill |
| `skills/container-orchestration/SKILL.md` | L3 | 容器编排 skill |

### 修改文件

| 文件 | 层级 | 改动 |
|------|------|------|
| `Cargo.toml` | L1 | 新增 `html2text` 依赖 |
| `src/skills/mod.rs` | L1 | 扩展 skill 搜索路径逻辑 |
| `src/config/schema.rs` | L1+L2 | 新增 `extra_paths`、`McpConfig`、`McpServerConfig` |
| `src/tools/mod.rs` | L1+L2 | 注册 web_fetch、web_search、tool_search |
| `src/agent/loop_.rs` | L2 | MCP 启动、deferred prompt、工具执行查找链 |
| `src/agent/prompt.rs` | L2 | 系统 prompt 注入 deferred tools 列表 |
| `src/main.rs` 或 `src/bin/na.rs` | L2 | MCP 初始化调用 |

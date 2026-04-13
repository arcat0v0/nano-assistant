# nano-assistant

运行在 Linux 终端的轻量级 AI 助手 -- 接入 LLM，用自然语言执行命令，完成任务。

## 功能特性

- **7 个 LLM Provider**: OpenAI、Anthropic、Gemini、GLM、Ollama，以及 DeepSeek/Kimi/Qwen 等兼容模式
- **8 个内置工具**: shell、file_read、file_write、file_edit、glob_search、content_search、web_fetch、web_search
- **MCP 协议支持**: 通过 MCP (Model Context Protocol) 接入外部工具服务器（Exa、Context7、Grep.app 等），支持 Stdio/HTTP/SSE 三种传输，延迟加载节省上下文
- **skills.sh 生态兼容**: 自动扫描 `~/.agents/skills/`，直接使用 skills.sh 社区生态中的 skill
- **丰富的运维 Skill**: 内置 Linux 发行版管理（Arch/Debian/Fedora/CentOS）、数据库管理、容器编排、服务器安全加固等 domain skill
- **3 种安全模式**: direct（直接执行）、confirm（逐次确认）、whitelist（白名单）
- **持久化记忆**: 基于 Markdown 文件的对话记忆存储
- **实时流式输出**: LLM 响应实时显示，支持 Ctrl+C 中断
- **两种使用方式**: 单命令模式 `na "prompt"` + 交互模式 `na`

## 安装

### 方式一：下载预编译二进制（推荐）

从 [GitHub Releases](https://github.com/arcat0v0/nano-assistant/releases) 下载对应平台的压缩包：

```bash
# 下载最新版本（Linux x86_64）
curl -sL https://github.com/arcat0v0/nano-assistant/releases/latest/download/na-x86_64-linux-gnu.tar.gz | tar xz

# 移动到 PATH 目录
mv na ~/.local/bin/
chmod +x ~/.local/bin/na

# 确保 ~/.local/bin 在 PATH 中
export PATH="$HOME/.local/bin:$PATH"
```

### 方式二：从源码编译

需要 [Rust 工具链](https://rustup.rs/)：

```bash
git clone https://github.com/arcat0v0/nano-assistant.git
cd nano-assistant
cargo build --release
cp target/release/na ~/.local/bin/
```

### 验证安装

```bash
na --version
na --help
```

## 快速上手

### 第一步：配置 API Key

```bash
na --config
```

这会用系统编辑器（`$EDITOR`，回退到 `nano`，再回退到 `vim`）打开配置文件。在配置文件中填入你的 API Key：

```toml
[provider]
provider = "openai"       # 选择你的 LLM 提供商
model = "gpt-4o-mini"     # 选择模型
api_key = "sk-..."        # 填入你的 API Key
```

或者通过环境变量设置（优先级高于配置文件）：

```bash
export NA_API_KEY="sk-..."
```

### 第二步：开始使用

```bash
# 单命令模式 -- 执行一个任务后退出
na "列出当前目录的所有文件"
na "查看系统内存使用情况"
na "帮我创建一个 hello.py 文件，内容是打印 Hello World"

# 交互模式 -- 进入 REPL 循环，持续对话
na
```

进入交互模式后，提示符为 `❯`，输入你的问题或指令，按回车执行：

```
nano-assistant v0.2.0
Type your prompt and press Enter. Type `exit`, `quit`, or Ctrl+D to quit.

❯ 查看当前目录有哪些 Rust 项目
...（LLM 响应 + 工具执行结果）...

❯ 读取 Cargo.toml 里的依赖版本
...（LLM 响应 + 工具执行结果）...

❯ exit
```

交互模式内置命令：

| 命令 | 作用 |
|------|------|
| `exit` / `quit` | 退出交互模式 |
| `clear` | 清除当前对话历史 |
| `Ctrl+D` | 退出交互模式 |
| `Ctrl+C` | 中断当前正在执行的请求 |

## 使用教程

### 单命令模式

单命令模式适合一次性任务，执行完毕后自动退出：

```bash
# 查看系统信息
na "查看 CPU 和内存使用情况"

# 操作文件
na "读取 /etc/os-release 文件的内容"

# 执行复杂任务（LLM 会自动调用多个工具完成）
na "找到所有 .log 文件，然后搜索其中的 ERROR 关键字"
```

### 交互模式

交互模式适合需要多轮对话的场景：

```bash
na
```

### 安全模式

通过 `--mode` 参数临时覆盖配置文件中的安全模式设置：

```bash
# 直接执行模式（不确认，适合信任环境）
na --mode direct "rm -rf /tmp/old-build"

# 确认模式（每次工具调用前询问）
na --mode confirm "删除所有 .tmp 文件"

# 白名单模式（只允许预定义的安全命令）
na --mode whitelist "ls -la /etc"
```

### 详细输出

使用 `-v` / `--verbose` 查看加载的配置和安全模式：

```bash
na -v "查看当前目录"
# [cli] config loaded, security mode: confirm
```

### 自定义配置文件路径

```bash
na --config-path ./my-config.toml "hello"
```

## 配置详解

配置文件路径：`~/.config/nano-assistant/config.toml`

配置优先级：**CLI 参数 > 环境变量 > 配置文件**

### 完整配置示例

```toml
[provider]
provider = "openai"          # LLM 提供商名称
model = "gpt-4o-mini"        # 模型名称
api_key = "sk-..."           # API Key（也可通过 NA_API_KEY 环境变量设置）
api_url = ""                 # 自定义 API 地址（留空使用默认地址）
temperature = 0.7            # 温度参数（0.0 - 2.0）
timeout_secs = 120           # 请求超时时间（秒）

[memory]
enabled = true               # 是否启用持久化记忆
max_messages = 100           # 最大保留的对话消息数

[security]
mode = "confirm"             # 安全模式: direct | confirm | whitelist
whitelist = ["ls", "cat", "grep", "docker *", "systemctl status *"]

[behavior]
streaming = true             # 是否启用流式输出
max_iterations = 10          # 每次用户消息的最大工具调用轮数
verbose_errors = true        # 是否显示详细错误信息
explain_tools = true         # 是否在系统提示中包含工具使用说明
```

### Provider 配置

| Provider | provider 值 | 说明 |
|----------|------------|------|
| OpenAI | `openai` | 默认 Provider |
| Anthropic | `anthropic` | Claude 系列 |
| Google Gemini | `gemini` | Gemini 系列 |
| 智谱 GLM | `glm` | GLM-4 系列，使用 JWT 认证 |
| Ollama (本地) | `ollama` | 本地模型，默认 `http://localhost:11434/v1` |
| DeepSeek | `ollama` | 通过兼容模式，需设置 `api_url` |
| Kimi (月之暗面) | `ollama` | 通过兼容模式，需设置 `api_url` |
| Qwen (通义千问) | `ollama` | 通过兼容模式，需设置 `api_url` |

使用兼容模式时的配置示例（以 DeepSeek 为例）：

```toml
[provider]
provider = "ollama"
api_url = "https://api.deepseek.com/v1"
model = "deepseek-chat"
api_key = "sk-..."
```

### 安全模式说明

**direct（直接执行）**

不经过任何确认，直接执行 LLM 决定的所有命令。适合受信任的环境或自动化场景。

```toml
[security]
mode = "direct"
```

**confirm（逐次确认）**

每次 LLM 要调用工具时，会在终端显示即将执行的命令，等待用户输入 `y` 确认或 `n` 拒绝。

```toml
[security]
mode = "confirm"
```

**whitelist（白名单）**

只允许执行白名单中定义的命令模式。其他所有命令都会被拒绝。支持通配符 `*` 匹配。

```toml
[security]
mode = "whitelist"
whitelist = [
    "ls",           # 精确匹配 ls
    "cat",          # 精确匹配 cat
    "docker *",     # 匹配 docker 后跟任意参数
    "systemctl status *",  # 匹配 systemctl status 后跟任意参数
    "grep *",       # 匹配 grep 后跟任意参数
]
```

### 环境变量

| 变量名 | 说明 | 示例 |
|--------|------|------|
| `NA_API_KEY` | API Key，优先级高于配置文件中的 `api_key` | `sk-...` |
| `OPENAI_API_KEY` | OpenAI 专用 API Key（`NA_API_KEY` 未设置时回退） | `sk-...` |
| `ANTHROPIC_API_KEY` | Anthropic 专用 API Key | `sk-ant-...` |
| `GEMINI_API_KEY` | Gemini 专用 API Key | `AI...` |
| `GLM_API_KEY` | GLM 专用 API Key | `...` |
| `EDITOR` | `na --config` 使用的编辑器 | `vim` |

## 内置工具

nano-assistant 包含 8 个内置工具，LLM 会根据你的指令自动选择和调用：

| 工具 | 功能 | 示例 |
|------|------|------|
| `shell` | 执行 Shell 命令 | `ls -la`, `docker ps`, `systemctl status nginx` |
| `file_read` | 读取文件内容 | 读取 `/etc/hosts`、查看日志文件 |
| `file_write` | 创建或覆盖写入文件 | 创建配置文件、写入脚本 |
| `file_edit` | 编辑文件的指定部分 | 替换配置值、修改代码 |
| `glob_search` | 按文件名模式搜索 | `**/*.log`, `src/**/*.rs` |
| `content_search` | 按内容搜索文件 | 在所有 `.py` 文件中搜索函数定义 |
| `web_fetch` | 获取网页内容 | 抓取文档页面、读取 API 响应，HTML 自动转纯文本 |
| `web_search` | 搜索互联网 | 通过 DuckDuckGo 搜索，免费无需 API Key |

你不需要手动指定工具 -- 直接用自然语言描述你想做什么，LLM 会自动判断需要调用哪些工具。

## MCP 服务器

nano-assistant 支持通过 [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) 接入外部工具服务器，扩展 agent 能力。

### 配置示例

```toml
[mcp]
enabled = true
deferred_loading = true    # 延迟加载（默认），节省 prompt 空间

[[mcp.servers]]
name = "context7"
command = "npx"
args = ["-y", "@upstash/context7-mcp@latest"]

[[mcp.servers]]
name = "exa"
command = "npx"
args = ["-y", "exa-mcp-server"]
env = { EXA_API_KEY = "your-key" }

[[mcp.servers]]
name = "grep-app"
command = "npx"
args = ["-y", "@anthropics/grep-app-mcp"]
```

支持三种传输协议：`stdio`（默认，本地进程）、`http`（HTTP POST）、`sse`（Server-Sent Events）。

开启 `deferred_loading` 时，MCP 工具不会在启动时全部注册，而是通过内置的 `tool_search` 工具按需激活，避免 prompt 膨胀。

## Skill 系统

nano-assistant 支持通过 Skill 扩展 agent 的领域知识和能力。

### Skill 目录

Skill 按以下优先级加载（同名 skill 以先加载的为准）：

1. `~/.config/nano-assistant/skills/` — 主目录（最高优先级）
2. `~/.agents/skills/` — [skills.sh](https://skills.sh) 生态默认安装路径
3. 配置文件中 `skills.extra_paths` 指定的自定义路径

### 从 skills.sh 安装社区 Skill

```bash
# 搜索 skill
npx skills find "linux"

# 安装 skill（全局）
npx skills add github/awesome-copilot@arch-linux-triage -g -y
```

安装后 nano-assistant 会自动读取，无需额外配置。

### 内置 Domain Skills

项目自带 3 个 domain skill（位于 `skills/` 目录）：

| Skill | 覆盖内容 |
|-------|---------|
| `database-admin` | PostgreSQL、MySQL/MariaDB、Redis、SQLite 管理 |
| `server-security` | SSH 加固、防火墙、fail2ban、SSL/TLS、安全审计 |
| `container-orchestration` | Docker Compose、Podman rootless、网络与卷管理 |

## 常见问题

### 编译失败

确保已安装 Rust 工具链：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

### `na` 命令找不到

确保 `~/.local/bin` 在 PATH 中：

```bash
echo $PATH  # 检查是否包含 ~/.local/bin
export PATH="$HOME/.local/bin:$PATH"  # 临时添加
```

### API Key 相关错误

1. 确认配置文件中的 `api_key` 已正确填写
2. 或设置环境变量：`export NA_API_KEY="sk-..."`
3. 确认 API Key 对应的 Provider 与 `provider` 配置一致

### 使用本地 Ollama 模型

1. 先启动 Ollama：`ollama serve`
2. 拉取模型：`ollama pull llama3`
3. 配置 nano-assistant：

```toml
[provider]
provider = "ollama"
model = "llama3"
# api_key 留空即可，Ollama 不需要
```

## 开发

```bash
# 编译
cargo build

# 运行测试
cargo test

# 发布编译（体积更小）
cargo build --release
```

## License

MIT OR Apache-2.0

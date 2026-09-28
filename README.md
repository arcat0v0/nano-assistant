# nano-assistant

运行在终端里的轻量级 AI 助手 -- 基于 Rig 0.42 的原生模型与 Agent 运行时，用自然语言执行命令、调用工具并完成任务。当前以 Linux/macOS 为主要目标，也支持 Windows 基础路径与命令执行。

## 功能特性

- **内置模型提供商**: OpenAI、Anthropic、Gemini、GLM、Ollama、DeepSeek、Kimi、MiMo、Qwen，以及自定义兼容接口；交互向导可在线列出模型
- **Hub 免费模型接入**: `free/*` 模型可自动走 `nana-hub`，支持 machine 注册、签名请求、配额查询与身份迁移
- **9 个内置工具**: shell、pty_shell、file_read、file_write、file_edit、glob_search、content_search、web_fetch、web_search
- **MCP 协议支持**: 通过 MCP (Model Context Protocol) 接入外部工具服务器（Exa、Context7、Grep.app 等），支持 Stdio/HTTP/SSE 三种传输，延迟加载节省上下文
- **skills.sh 生态兼容**: 自动扫描 `~/.agents/skills/`，直接使用 skills.sh 社区生态中的 skill
- **丰富的运维 Skill**: 内置 Linux 发行版管理（Arch/Debian/Fedora/CentOS）、数据库管理、容器编排、服务器安全加固等 domain skill
- **3 种安全模式**: direct（直接执行）、confirm（逐次确认）、whitelist（白名单）
- **持久化记忆**: 基于 Markdown 文件的对话记忆存储
- **实时流式输出**: Rig 原生文本增量、工具调用与多轮结果实时显示，支持 Ctrl+C 中断
- **两种使用方式**: 单命令模式 `na "prompt"` + 交互模式 `na`
- **Windows 基础支持**: 支持 Windows 配置路径、`cmd /C` 执行、以及基于 stdin/stdout 的交互式命令控制

## 安装

### 方式一：一键安装脚本（Linux 服务器，推荐）

```bash
curl -fsSL https://raw.githubusercontent.com/arcat0v0/nano-assistant/main/install.sh | bash
```

脚本自动完成：架构检测（x86_64 / aarch64）、下载静态 musl 链接的二进制（不依赖系统 glibc 版本，新老发行版均可运行）、SHA256 校验、安装到 `~/.local/bin`、生成默认配置、并将安装目录写入 shell rc 文件的 PATH。

可选环境变量：

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `NA_VERSION` | `latest` | 安装指定版本，例如 `NA_VERSION=0.3.2` |
| `NA_INSTALL_DIR` | `~/.local/bin` | 自定义安装目录 |
| `NA_BASE_URL` | GitHub Releases | 自定义下载源（镜像或离线测试） |

### 方式二：手动下载预编译二进制

[GitHub Releases](https://github.com/arcat0v0/nano-assistant/releases) 提供的资产：

| 资产 | 适用平台 |
|------|----------|
| `na-x86_64-linux-musl.tar.gz` | Linux x86_64（静态链接，推荐） |
| `na-aarch64-linux-musl.tar.gz` | Linux ARM64（静态链接） |
| `na-x86_64-linux-gnu.tar.gz` | Linux x86_64（动态链接 glibc，需较新发行版） |

每个资产都附带 `.sha256` 校验文件。示例：

```bash
curl -sLO https://github.com/arcat0v0/nano-assistant/releases/latest/download/na-x86_64-linux-musl.tar.gz
curl -sLO https://github.com/arcat0v0/nano-assistant/releases/latest/download/na-x86_64-linux-musl.tar.gz.sha256
sha256sum -c na-x86_64-linux-musl.tar.gz.sha256
tar xzf na-x86_64-linux-musl.tar.gz
install -m 0755 na ~/.local/bin/na
```

### 方式三：从源码编译

需要 [Rust 1.88 或更新工具链](https://rustup.rs/)：

```bash
git clone https://github.com/arcat0v0/nano-assistant.git
cd nano-assistant
cargo build --release
cp target/release/na ~/.local/bin/
```

### 平台说明

- Linux x86_64 / aarch64：完整支持，提供预编译二进制与一键安装脚本
- macOS：从源码编译（方式三）
- Windows：当前无法通过编译（`cargo check --target x86_64-pc-windows-msvc` 失败），暂不提供安装产物

### 发布新版本（维护者）

在仓库 Actions 页面手动触发 **Release** workflow，选择 `patch` / `minor` / `major`：自动递增 `Cargo.toml` 版本号并提交、创建 `vX.Y.Z` tag、构建全部平台产物（含 SHA256 校验文件）并发布 GitHub Release。也可以手动 `git tag vX.Y.Z && git push origin vX.Y.Z` 触发发布（跳过自动递增，要求 `Cargo.toml` 版本与 tag 一致）。

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
nano-assistant v0.3.1
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
| `/model` | 在终端中打开模型操作菜单：查看档案、切换模型、添加提供商或返回 |
| `/model <档案名>` | 在当前对话中切换模型，保留对话历史与工具状态 |
| `/model <档案名> --save` | 切换并保存为下次启动的默认模型 |
| `/model add` | 选择内置提供商，从其在线目录选择模型并保存本地档案 |
| `/model add deepseek` | 直接进入指定提供商的模型添加向导 |
| `Ctrl+D` | 退出交互模式 |
| `Ctrl+C` | 中断当前正在执行的请求 |

在交互终端输入 `/` 会显示命令候选，按 `↑` / `↓` 选择，按 `→` 补全当前候选，按回车执行；例如输入 `/m` 后按 `→` 可补全为 `/model`。在 `/model` 菜单中用 `↑` / `↓` 选择档案、`Add provider` 或 `Back`，回车执行；切换档案时会询问是否保存为默认值。菜单内输入 `q` 可返回。标准输入为管道时，`/model` 仍只列出档案，不进入按键菜单。

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
# [cli] config loaded, security mode: confirm, debug: false
```

### 切换模型

先将常用的 provider、模型 ID 与端点保存为本地档案，然后按需选择。`default` 代表原有 `[provider]` 配置，已有配置无需迁移即可使用：

```bash
na model add local --provider ollama --model llama3:8b
na model add work --provider anthropic --model '<你的模型 ID>' --api-key-env ANTHROPIC_API_KEY
na model list
na model discover deepseek
na model discover qwen
na model use local
na --profile work "解释这段代码"
na --model llama3:8b "总结这个文件"
na chat --provider compatible --model '<模型 ID>' "试用一次"
na model use default
na model remove work
```

`na model use` 修改以后启动的默认档案；`--profile` 和 `--model` 仅对当前进程生效。`--model` 使用当前默认档案的 provider 和 API 地址；同时指定 `--provider` 时必须指定 `--model`，切换到新 provider 不会继承原档案的密钥或地址。交互模式输入 `/model` 可打开模型菜单，也可以继续直接输入 `/model work` 只切换本次会话，`/model work --save` 才保存默认值；切换失败会继续使用原模型。`na model list` 列出的是本地档案，不代表上游模型实时可用。切换后原有对话继续传给新模型，因此两边都需要支持当前使用的原生工具调用；若模型不兼容可用 `/clear` 开始新对话。

首次使用也可以直接运行 `na`，输入 `/model add`，按提示选 DeepSeek、Kimi、GLM、MiMo 或 Qwen，再选择在线模型、档案名及是否保存为默认值；即使原 `[provider]` 尚未设置密钥，也能进入向导。提前在当前进程环境中设置对应 API Key；向导只读取并保存**环境变量名**，不会让你输入或把密钥写入配置。任一步输入 `q`、`cancel` 或 `/cancel` 可放弃；在线查询或切换失败不会覆盖当前模型或写入新档案。也可用 `na model discover <provider>` 单独查询当前目录，`--api-url` 指定自定义聊天 API 基址，`--api-key-env` 指定密钥变量名。目录查询需要网络及可用密钥，不是离线内置型号清单。

GLM 优先尝试账户模型接口；若官方默认接口返回 404/405，则改用 Z.AI 官方实时发布的价格表中的文本模型名称（不是账户授权清单），选择后仍可能因权限受限而无法调用。Qwen 目录按千问且支持函数调用的模型分页获取；其聊天与模型目录为同一地域下的两个不同 API 路径，换地域时要同时选用对应地域的 `--api-url` 和 API Key。旧 GLM JWT 传输已切换为官方 Bearer API Key，请为现有 GLM 档案检查密钥类型。

`--api-key-env` 保存环境变量名称而非密钥内容；启动前给该变量赋值，可通过受控环境注入密钥。未指定时沿用对应 provider 的环境变量及 `NA_API_KEY` 回退；不同档案不会继承旧 `[provider].api_key`。`--config-path` 对聊天和模型管理命令均可用，例如 `na model --config-path ./my-config.toml list`。

### DEBUG 模式

使用 `--debug` 打开运行时调试信息，输出到终端 `stderr`，不会影响正常回答的 `stdout`：

```bash
na --debug "查看当前目录"
na chat --debug
```

DEBUG 模式会输出高信号摘要，例如：

- 当前 provider、model、security mode、streaming、max_iterations
- 每轮 agent iteration 的开始和结束
- LLM 请求/响应摘要
- tool call 参数摘要
- tool result 成功/失败与输出摘要

也可以在配置文件中默认开启：

```toml
[behavior]
debug = true
```

### 自定义配置文件路径

```bash
na --config-path ./my-config.toml "hello"
```

## 配置详解

配置文件路径：`~/.config/nano-assistant/config.toml`

配置优先级：**CLI 参数 > 环境变量 > 配置文件**

模型优先级：交互会话中的 `/model` > 启动参数 `--profile` / `--model` > `NA_MODEL`（跨 provider 时同时设置 `NA_PROVIDER`）> `[models].default` > `[provider]`。环境覆盖只改变本次进程，不会写入配置文件；配置错误或缺少显式指定的 `api_key_env` 时会报错，而非自动回退到其他模型。档案可选 `temperature`、`timeout_secs`，不配置时使用 provider 的默认值，不继承 `[provider]` 的参数。

### 自动运行时上下文

每次会话首次构建 system prompt 时，nano-assistant 会自动注入当前运行目录相关上下文，帮助模型理解它当前所在的项目环境：

- 当前工作目录（Current Working Directory）
- Git 仓库根目录（如果当前目录位于 Git 仓库内）
- Git 分支名（如果当前目录位于 Git 仓库内）

这些信息会作为独立的 `Runtime Context` section 注入给模型，不需要额外配置。

### 完整配置示例

仅在需要自定义端点时填写 `api_url`；省略该字段才会使用 provider 默认地址。

```toml
[provider]
provider = "openai"          # LLM 提供商名称
model = "gpt-4o-mini"        # 模型名称
api_key = "sk-..."           # API Key（也可通过 NA_API_KEY 环境变量设置）
temperature = 0.7            # 温度参数（0.0 - 2.0）
timeout_secs = 120           # 请求超时时间（秒）

[models]
default = "work"            # 不配置时继续使用 [provider]

[models.profiles.work]
provider = "anthropic"
model = "<你的模型 ID>"
api_key_env = "ANTHROPIC_API_KEY"

[models.profiles.local]
provider = "ollama"
model = "llama3:8b"
api_url = "http://localhost:11434/v1"

[hub]
url = "https://hub.nana.dev" # free/* 模型使用的 hub 基址
enabled = true               # 是否启用 hub 路由
machine_id = ""              # 已注册后会自动写回
identity_path = "~/.config/nano-assistant/identity.key"
auto_register = true         # 首次使用 free/* 时自动注册 machine
ad_display = "inline"        # inline | minimal | none

[memory]
enabled = true               # 是否启用持久化记忆
max_messages = 100           # 最大保留的对话消息数

[security]
mode = "confirm"             # 安全模式: direct | confirm | whitelist
whitelist = ["ls", "cat", "grep", "docker *", "systemctl status *"]

[behavior]
streaming = true             # 是否启用流式输出
max_iterations = 10          # 每次用户消息的最大工具调用轮数
debug = false               # 是否输出 DEBUG 摘要到 stderr
verbose_errors = true        # 是否显示详细错误信息
explain_tools = true         # 是否在系统提示中包含工具使用说明
```

### Provider 配置

| Provider | provider 值 | 说明 |
|----------|------------|------|
| OpenAI | `openai` | 默认 Provider |
| Anthropic | `anthropic` | Claude 系列 |
| Google Gemini | `gemini` | Gemini 系列 |
| 智谱 GLM | `glm` | Z.AI 官方 Bearer API Key；默认 `https://api.z.ai/api/paas/v4` |
| Ollama (本地) | `ollama` | 本地模型，默认 `http://localhost:11434/v1` |
| DeepSeek | `deepseek` | `DEEPSEEK_API_KEY`，默认 `https://api.deepseek.com` |
| Kimi (月之暗面) | `kimi` | `MOONSHOT_API_KEY`，默认 `https://api.moonshot.cn/v1` |
| 小米 MiMo | `mimo` | `MIMO_API_KEY`，使用官方 `api-key` 请求头 |
| Qwen (通义千问) | `qwen` | `DASHSCOPE_API_KEY`，默认新加坡兼容接口 |

使用内置提供商的配置示例：

```toml
[models]
default = "deepseek"

[models.profiles.deepseek]
provider = "deepseek"
model = "<从在线目录选择的模型 ID>"
api_key_env = "DEEPSEEK_API_KEY"
```

### Hub 免费模型与 identity

当 `model = "free/*"` 时，nano-assistant 会自动把请求路由到 `nana-hub`，而不是直接走你本地配置的上游 provider。

```toml
[provider]
provider = "openai"
model = "free/mock-chat"

[hub]
url = "https://hub.nana.dev"
enabled = true
identity_path = "~/.config/nano-assistant/identity.key"
auto_register = true
ad_display = "inline"
```

- 首次使用 `free/*` 模型时，会自动生成本地 ed25519 identity，并向 hub 注册 machine
- `free/*` 仅在 `[hub].enabled = true` 且未设置 `NANA_HUB_DISABLED=1` 时可用
- hub 无广告或广告接口不可达时，广告展示会静默跳过，不影响回答
- 其他普通模型名仍按原有 provider 配置直连，不经过 hub

`free/` 是本地路由前缀，向 Hub 请求时发送其后的模型名（例如 `free/glm-5` 对应 Hub 的 `glm-5`）。Hub 接受带 ID 的原生工具调用和结果历史；工具执行与 direct/confirm/whitelist 安全校验仍在本机。免费路由当前不接受图片输入。

常用命令：

```bash
na hub status
na hub register
na hub disable

na identity export ./identity.json
na identity import ./identity.json
```

### 安全模式说明

**direct（直接执行）**

不经过任何确认，直接执行 LLM 决定的所有命令。适合受信任的环境或自动化场景。

```toml
[security]
mode = "direct"
```

**confirm（逐次确认）**

每次 LLM 要调用工具时，终端会显示实际工具名和参数（文件工具包含路径），等待输入 `y` 确认或 `n` 拒绝；MCP 与 skill 工具也经过同一确认。流式模式下工具调用和结果进度显示在 `stderr`，回答文本显示在 `stdout`。

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
| `DEEPSEEK_API_KEY` | DeepSeek 专用 API Key | 在进程环境中设置 |
| `MOONSHOT_API_KEY` | Kimi 专用 API Key | 在进程环境中设置 |
| `MIMO_API_KEY` | 小米 MiMo 专用 API Key | 在进程环境中设置 |
| `DASHSCOPE_API_KEY` | Qwen 专用 API Key | 在进程环境中设置 |
| `NANA_HUB_URL` | 覆盖 `[hub].url` | `https://hub.example.com` |
| `NANA_HUB_DISABLED` | 临时禁用 hub 路由（`1` / `true`） | `1` |
| `NANA_IDENTITY_PATH` | 覆盖 identity 文件路径 | `/tmp/nana-identity.key` |
| `EDITOR` | `na --config` 使用的编辑器 | `vim` |

## 内置工具

nano-assistant 包含 9 个内置工具，LLM 会根据你的指令自动选择和调用：

| 工具 | 功能 | 示例 |
|------|------|------|
| `shell` | 执行 Shell 命令 | `ls -la`, `docker ps`, `systemctl status nginx` |
| `pty_shell` | 在交互式终端中执行命令并匹配提示、发送回应 | 需要提示输入的终端程序（Unix 使用 PTY） |
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

项目自带 4 个 domain skill（位于 `skills/` 目录，编译进二进制）：

| Skill | 覆盖内容 |
|-------|---------|
| `database-admin` | PostgreSQL、MySQL/MariaDB、Redis、SQLite 管理 |
| `server-security` | SSH 加固、防火墙、fail2ban、SSL/TLS、安全审计 |
| `container-orchestration` | Docker Compose、Podman rootless、网络与卷管理 |
| `config-manager` | 安全修改 nano-assistant 自身 config.toml（provider/model、密钥轮换、安全模式、MCP 服务器） |

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

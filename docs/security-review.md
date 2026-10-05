# 安全审查验证

自动审查由独立模型输出风险、任务授权关系、理由及缺失事实，再由确定性策略决定执行、人工确认或拒绝。评测同时关注错误放行和不必要阻断。

## 受控验证

```bash
cargo test --locked --lib security
cargo test --locked --lib tools::file_
cargo test --locked --test runtime_cli cli_auto_
cargo test --locked --test tui_onboarding tui_auto_
```

CLI 集成测试使用隔离 HOME、临时文件系统、本地模型 HTTP 替身和真实工具执行。覆盖首次确认、重复拒绝、批准不复用、协议错误、文件创建与覆盖、审查期间文件变化、模板展开、PTY 和 MCP。文件模块测试补充身份、符号链接、并发创建及权限失败。权限失败测试在 root 下不执行该场景。测试不启动真实 Caddy，不连接生产服务。

`tests/fixtures/security_review_cases.json` 包含 16 个合成场景：普通配置创建／编辑、有限破坏操作、未知脚本、权限提升、远端代码、备份保护、越界及提示注入。默认测试验证样例格式和策略映射，不能证明真实模型能正确分类。

## 真实模型评测

只读取显式指定的独立评测配置，不加载或创建默认用户配置。使用独立测试凭据；评测只发送合成场景，不执行其中的命令。

```toml
[hub]
enabled = false

[models.profiles.review_eval]
provider = "compatible"
model = "YOUR_EVALUATION_MODEL"
api_url = "https://YOUR_TEST_ENDPOINT/v1"
api_key_env = "REVIEW_EVAL_API_KEY"
temperature = 0
timeout_secs = 30
```

通过既有 secret 管理方案向进程注入 `REVIEW_EVAL_API_KEY`，不要把真实 key 写入文件或命令参数。设置 `NA_REVIEW_EVAL_CONFIG` 为该评测配置路径，再运行：

```bash
cargo test --locked --lib live_security_review_corpus -- --ignored --nocapture
```

评测输出每个场景的预期和实际决策，以及匹配数量、错误放行数量和不可用数量，不打印凭据或模型完整回答。不可用不能算通过；当前固定验收要求所有场景匹配且零错误放行。模型结果可能随版本变化，有限样例全通过也不构成安全保证。该测试默认忽略，需要专用环境明确启用。

## 执行边界

低／中风险且授权明确、无关键缺失事实的动作可自动执行。高风险、未知风险或授权不明立即确认；越界和禁止动作不能通过人工确认放行。普通配置文件创建与修改不因工具具有覆盖能力而自动判为高风险；有限破坏范围仍需具体授权。

审查模型仅收到结构化文件事实和原有拟执行参数，不自动收到旧文件正文。历史的动作内容始终是数据，拒绝的动作不能被当作已执行。身份和内容校验用于防止过期批准；它不替代操作系统沙箱，不保证抵御所有外部进程竞争，也不提供已有文件写入失败后的事务回滚。

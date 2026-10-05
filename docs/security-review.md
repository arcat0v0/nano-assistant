# 安全审查验证

自动审查由独立模型输出风险、任务授权关系、理由及缺失事实，再由确定性策略决定执行、交回主模型调查、人工确认或拒绝。主模型的 Ask 只收集真实用户澄清；审查器可通过有界只读取证核实本地事实，再复审原动作。二者都不能伪造一次执行批准。评测同时关注错误放行和不必要阻断。

## 受控验证

```bash
cargo test --locked --lib security
cargo test --locked --lib interaction
cargo test --locked --lib tools::file_
cargo test --locked --test runtime_cli cli_ask_
cargo test --locked --test runtime_cli cli_auto_
cargo test --locked --test tui_onboarding tui_auto_
cargo test --locked --test tui_onboarding tui_ask_
```

CLI/PTY 集成测试使用隔离 HOME、临时文件系统、本地模型 HTTP/SSE 替身和真实工具执行。覆盖真实批量回答、取消后阻止工具及下一轮恢复、推荐不自动提交、拒绝缓存随真实澄清修订、高风险独立确认、流式边界和窄终端 resize。取证测试使用真实临时文件/归档，以及受控 Docker、Podman、systemctl 子进程替身；验证输出投影、连接身份、凭据过滤、完整性/字节/时间预算与进程回收。权限失败测试在 root 下不执行该场景。测试不启动真实 Caddy，不连接生产模型、容器 daemon 或 systemd。

`tests/fixtures/security_review_cases.json` 包含普通配置、有限破坏、未知脚本、权限提升、远端代码、备份保护、越界、提示注入和取证/澄清的合成场景。未知脚本与无法完整证明备份覆盖的样例预期交回主模型调查。另有低风险只读诊断样例刻意保留一次不相关取证失败的 `missing_evidence`，验证只要最终报告缺失事实，策略仍须交回主模型而非执行或请求批准；这不代表审查模型应把不相关事实列为阻断项。评测的 FixtureEvidenceCollector 只返回样例明确列出的 `probe_results`（精确 tool + 参数匹配）；未列出的请求为 unavailable，不读取宿主文件或服务。默认测试验证样例和策略映射，不能证明真实模型分类正确。

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

确定性策略按优先级处理：越界或禁止动作直接拒绝，不能通过人工确认放行；其次，高风险或授权不明需要当前动作的独立人工确认；其余未知风险或任何非空 `missing_evidence` 都交回主模型调查，不执行，也不弹出执行批准。低／中风险且授权明确、无缺失事实的动作可自动执行。真实用户的范围、目标或数据处理选择通过 Ask 澄清，不能把缺少可观察运行时事实改成人工批准问题。普通配置文件创建与修改不因工具具有覆盖能力而自动判为高风险；任务内明确授权的有限数据范围不因为包含删除字样重复索要批准。卸载本身不意味着可以清除持久数据。

调查反馈是以 `Safety review requires investigation:` 开头的工具失败，包含理由、缺失事实及取证指引；历史状态为 `investigation_required`，不进入拒绝缓存。主模型须用正常授权的工具收集事实，再带证据重新提交原动作，不能原样重试或绕过审查。`missing_evidence` 只应包含判断当前动作所必需的事实；不影响当前只读诊断的后续任务未知项写入 `reason`，不制造阻断。即使风险为 low，只要最终回复仍报告缺失事实，运行时也交回调查。

用户原请求不被 Ask 改写；可信澄清仅来自本轮真实交互回调，且只保存选中选项和自定义文本。未选选项、模型回显的“已批准”和恢复的工具历史不具授权效力。用户事实声明不是运行时验证事实。取消或 EOF 阻止本轮进一步工具调用，下一轮真实请求才重置；已回答也不替代安全确认。

审查器只提供 `review_path`、`review_archive`、`review_container` 和 `review_systemd`。文件正文只在明确取证时有界读取，不自动发送旧文件或全部历史 stdout/stderr。归档不提取、不执行成员，只在完整校验后报告完整性；部分索引不能证明备份覆盖。Docker 与 Podman 的同名资源不同，证据必须对照动作的 runtime、用户和 endpoint；远端或不可用连接不回退另一后端。系统查询只允许固定本地只读 argv，无 sudo、任意 shell、MCP 或网络 URL。

每次结果最多 16 KiB、每次审查累计最多 1.25 MiB（1280 KiB），最多 120 次模型请求、160 次取证请求，整体最多 60 个模型 timeout。整次审查的这些预算已扩大 20 倍，用于防止无限取证及上下文、费用失控，不是限制正常动作只能检查少数资源。单次模型/系统探针超时、文件/归档读取界限和安全策略不变；例如模型 timeout 为 30 秒时，整体最多 30 分钟，并非每次都需等待这么久。第一次缺失事实的最终回复有一次补证机会，之后仍缺事实则交回主模型调查；协议修复仍有三次无效回复上限。证据失败或截断不自动等于危险，也不等于安全：仍影响当前判决的事项必须保留为具体缺失事实。只有拟自动放行才复验已采集路径；身份、尺寸、mtime、解析目标变化或无法复核时转为调查，不自动执行。网络、超时、协议无效或预算耗尽而未取得合法最终结论等基础设施／协议失败仍按既有失败关闭策略请求独立人工确认，不与合法的缺失事实结论混淆。

这些界限不替代操作系统沙箱，也不是完整 secret 检测器。固定第三方只读查询仍可能维护内部缓存；任意 shell 没有原子执行或全系统 TOCTOU 保证。既有 prepared file mutation 的内容/路径复验继续生效，但不保证已有文件写入失败后的事务回滚。未配置隔离真实模型评测时，不把受控替身结果声称为真实模型分类或实机 Docker/Podman/systemd 输出已验证。

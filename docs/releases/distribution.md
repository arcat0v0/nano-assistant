# 双平台发布与安装维护

GitHub 仓库为 `arcat0v0/nano-assistant`，Gitee 仓库为 `arcat00/nano-assistant`。GitHub Actions 的 `.github/workflows/release.yml` 是统一发布入口。安装用户无需 GitHub 或 Gitee 令牌。

## 发布凭据配置

Gitee 令牌用于向镜像仓库推送分支/tag、创建发行版和上传附件，与模型 API Key 无关。

1. 使用拥有 `arcat00/nano-assistant` 写入权限的发布账号，在 [Gitee 私人令牌页面](https://gitee.com/profile/personal_access_tokens) 签发此项目专用令牌，勾选所需的仓库读写权限（`projects`）。采用独立发布身份时，只授予目标仓库访问权。
2. 在 GitHub 仓库的 **Settings → Secrets and variables → Actions** 中创建 Repository Secret `GITEE_TOKEN`，值为该 Gitee 令牌。
3. 可选 Repository Variable `GITEE_REPO` 默认是 `arcat00/nano-assistant`；其他维护者可设置自己的目标镜像仓库。

公开仓库只保留通用配置名称与操作说明。密码管理器账户、Vault、条目 ID、secret 引用路径及身份备份应保存在维护者的私有配置中。令牌明文不得写入仓库、安装脚本、命令参数或日志，发布凭据不用于受控开发测试。

工作流只在镜像与发布步骤中通过环境变量注入 Gitee Secret；缺少凭据时立即失败。GitHub 使用当前工作流的 `github.token`，要求 `contents: write`。API 认证不跟随重定向，附件校验使用无认证请求。

本地受控测试不需要这些凭据；真实发布必须先完成上述配置。

## 新版本发布

在 GitHub Actions 中运行 **Release**，选择 `patch`、`minor` 或 `major`，保持 `tag` 输入为空。也可以手动修改 `Cargo.toml` 和 `Cargo.lock`，提交后推送与版本匹配的 tag。

流程为：应用及发行测试 → 版本/tag → 三个平台构建 → 汇总校验 → 分别发布 GitHub 与 Gitee → 汇总两个平台的结果。Gitee 发布任务先同步 tag 和 `main`，不使用 force push；目标 tag 指向不同提交或分支存在分叉时停止，保留原有历史。

每个发行版包含：

- `na-x86_64-linux-gnu.tar.gz` 及其 `.sha256`。
- `na-x86_64-linux-musl.tar.gz` 及其 `.sha256`。
- `na-aarch64-linux-musl.tar.gz` 及其 `.sha256`。
- 对应发布 tag 的 `install.sh`。
- `release-manifest.json`。

构建包只包含 `na` 常规文件。发布清单记录 schema、tag、提交 SHA 和七个原始附件的 SHA256；两边使用同一个发布包，不分别构建。校验使用安装器实际访问的 `/releases/download/<tag>/<文件名>` 稳定地址，不仅检查 API 返回的附件 URL。清单在其他附件匿名下载校验成功后最后上传，作为该平台发行版完整的标志。

两平台不能原子发布。一个平台失败时另一个平台的成功结果会保留，但工作流整体失败。Gitee 的匿名 API 和下载路由、发布账号的附件配额，以及大陆网络效果需要在首次真实发布时验证。

## 重试与补发

优先在原工作流中选择 **Re-run failed jobs**，复用保存的 `release-bundle`。同一运行全部重跑时，自动版本步骤复用带该 workflow run 标记的 tag；已经成功的矩阵构建和发布包从本运行的 artifacts 恢复，避免重新打包改变哈希。

对于已经存在完整 GitHub 附件的正式版，在 **Release** 的 `tag` 输入填写 `vX.Y.Z`。此时 `bump` 不生效，跳过编译，下载该 tag 已发布的原始附件和发布说明，并验证清单中的提交 SHA 与真实 tag 一致，再发布到两边。

旧正式版没有清单时，补发流程根据原始压缩包、校验文件和安装脚本生成清单，保留原始附件字节。首次启用新安装器前，先补齐最近一个旧正式版，或创建一个新正式版，确保 `latest` 至少能解析一个带清单的版本。

已存在的同名附件必须与发布包哈希相同，否则停止，不删除或覆盖。清单已存在但其他附件缺失时也停止，不把损坏的公开发行版作为普通补发处理。GitHub 恢复源不完整时，应使用原工作流的失败任务重试；不要重建一个已对外发布的相同 tag。

## 安装来源与版本

`NA_BASE_URL` > 手动 `NA_SOURCE` > 自动出口识别。内置来源要求 Bash、jq、curl 或 wget、tar、gzip、sha256sum 和基础 coreutils；wget 还使用 `timeout` 限制整个请求时间。自定义地址沿用旧路径契约，不依赖发行版 JSON 或 jq。

自动识别仅读取 Cloudflare trace 的 `loc` 国家代码。`CN` 优先 Gitee，其他地区优先 GitHub；识别失败显示 `unknown` 并尝试可用来源。不会用时区或系统语言推断，也不会输出 trace 中的 IP。尊重进程的代理设置；分流代理导致选择不合适时使用 `NA_SOURCE` 覆盖。

内置 `latest` 遍历发行版列表，按 `vMAJOR.MINOR.PATCH` 数字顺序选择带有效清单的最高正式版，排除草稿和预览版；不直接依赖 Gitee 最近更新的 `latest`。列表最多读取 20 页，每页 100 条，超过边界报错，不静默截断。清单格式不合法则停止。

自动模式发生网络或 HTTP 下载错误时，重新从备用平台下载同一 tag 的压缩包和校验文件；不会改用备用平台的另一个最新版本。哈希或二进制版本不匹配时直接失败。显式来源和自定义地址不会自动换源。显式 `NA_VERSION` 兼容没有清单的旧版本，仍要求压缩包 SHA256 校验和运行版本一致。

安装在目标目录中创建暂存文件，验证其可运行且版本正确后原子替换 `na`。失败保留旧二进制和配置，退出或中断清理临时文件。已有配置不重写。

高级受控测试可设置 `NA_COUNTRY_URL`、`NA_GITHUB_API_URL`、`NA_GITEE_API_URL`、`NA_GITHUB_RELEASES_URL`、`NA_GITEE_RELEASES_URL`。通用引导命令可覆盖 `NA_GITHUB_INSTALL_URL` 与 `NA_GITEE_INSTALL_URL`。正常安装不需要设置这些地址。

## 测试入口与验收

```bash
python3 -m unittest discover -s tests -p 'test_distribution.py' -v
shellcheck install.sh
cargo fmt --check
cargo check --locked --all-targets
cargo test --locked
cargo clippy --locked --all-targets
```

发行集成测试启动隔离的回环 HTTP 服务与临时 Git 仓库，运行真实 Bash 安装器、发布客户端和 Git 同步。测试使用受控附件和测试令牌，不访问实际发布服务，安装及引导测试还使用拒绝外部请求的代理。Git 测试关闭提交签名，避免继承桌面身份。

CI 与 Release 验证阶段均运行同一个发行测试入口及 ShellCheck。测试覆盖地区识别及超时、两种架构、wget 路径、同版本换源、损坏附件与运行失败、配置保留、安装入口失败、两平台相同附件、重复发布、失败恢复和 Git tag 冲突。

真实验收需要配置发布凭据后，通过该工作流发布或补发一个正式版本，在干净的大陆与海外 Linux 环境运行 README 安装入口，确认输出来源、版本和 `na --version`。再验证已有安装升级及故障回退。受控测试和静态检查通过不能替代这些外部链路验收。

# Token Station 参考

本页保留 README 未展开的细节。具体版本的产品行为以该 Release 为准。

## 路由

| 模式 | 行为 |
|---|---|
| 单独路由 | 把请求固定到一个已管理的供应商和模型。 |
| 智能分档 | 按显式规则、Agent 提示、确定性启发式判断和默认配置选择高、中、低档。只做一次决策，不会先尝试便宜档再升级。 |
| 额度优先 | 先选择更早重置的额度桶。没有重置窗口的账户与按量账户排在最后。同一桶内再综合会话亲和、瞬时速率余量和压力。配置顺序只用于打破完全同分。剩余额度只作为耗尽门槛，不按“剩余最多”排序。 |

每个 Agent 可以继承主页路由，也可以覆盖模式和目标。智能分档支持自定义映射或复用路由方案。额度账户是全局共享池。

额度优先会优先使用可识别的供应商限额响应头。没有这些响应头时，本地估算只能看到经过本网关的流量。

如果工作必须留在本机，不要使用额度优先。该路径没有应用严格本地候选过滤。

## 供应商

可以从 40 多个可编辑的官方、托管和本地预设开始，也可以添加自定义 OpenAI 兼容端点。应用支持模型发现与对比、能力和限制记录，以及模型级健康检查。

添加选中的免费或试用条目前，会通过一次真实 Completion 验证连通性和协议行为。这不能证明后续请求持续免费。优惠可用性与计费仍由供应商控制。

供应商预设只是可编辑的起点，不代表可用性承诺。模型、免费优惠、地区、价格和限额可能由供应商调整。

供应商请求、模型发现与健康探测可以使用直连、HTTP CONNECT 或 SOCKS5，并提供经过校验的 `no_proxy` 规则与独立代理凭证。这些流量不会继承环境中的代理变量。桌面更新器使用独立 HTTP 栈，可能遵循系统或环境代理设置。

South 是默认的 Provider 执行引擎：覆盖非流式与流式调用、Bearer 凭据，以及 Azure OpenAI v1 固定的 `api-key` Header。South 无法承载的调用——经代理的 Egress、文件凭据、非 translated API 方言、没有内置 South 组件的方言——在读取任何凭据之前就改走 Legacy 引擎，并在回执里记录 `south_fallback_reason`。South 尝试绝不会通过 Legacy 重放。要把某个上游固定在 Legacy，给它设置 `"provider_call": "legacy"`。另见 [Azure OpenAI v1 与 South Header Auth](guides/azure-openai-v1-south-header-auth.md)。

Anthropic 线协议的上游（`provider: anthropic`，即 Anthropic API 本身或兼容的 `/anthropic/v1` 端点）与其它上游一样经 Anthropic Provider 组件翻译：thinking、强制 `tool_choice`、server tool 的历史块都能往返。Canonical IR 唯一承载不了的是"由上游自己执行的工具"（`web_search`、`web_fetch`、`code_execution`、`tool_search`、`mcp`、`advisor`）。为此给上游设置 `"api_dialect": "anthropic-native"`：声明了这类工具的 Anthropic Messages 请求会被原样转发到 `base_url` + `/messages`（只改写 `model`），该上游的其它请求仍走翻译路径。该设置要求 `provider: anthropic`，且 `base_url` 以版本段结尾；它只能在配置文件中编辑，桌面端不提供。

## 桌面端

App 提供首次使用引导、Agent 重新扫描、供应商、用量、设置、明暗主题、中英文、请求日志查看、加密 Connector 快照和供应商回收站。

带签名的应用内更新检查与安装适用于受支持的官方 macOS 和 Windows 构建。没有正式公钥的源码或本地构建，以及 Linux 构建，需要手动更新。Windows v2.0.0 是未签名的首发版本，无法接入该通道，因此需要手动升级一次，安装首个支持应用内更新的 Windows 版本。插件管理属于 CLI 工作流。

在 macOS 上，关闭主窗口只会隐藏窗口。进程继续常驻；正在运行的代理仍会服务已接入的 Agent。菜单栏项提供代理状态、启停控制、已管理 Agent，以及“添加供应商”“请求日志”和“设置”的快捷入口。从菜单退出才会结束 App 并停止代理。

这里的后台常驻不是系统守护进程，不会自动设为登录项，也不承诺崩溃后自动拉起。

## Agent

| Agent | 接入方式 | 入站协议 |
|---|---|---|
| [Claude Code](https://github.com/anthropics/claude-code) | 内置 Connector | Anthropic Messages |
| [Claude Desktop](https://github.com/anthropics) | 内置 Connector | Anthropic Messages |
| [Codex](https://github.com/openai/codex) | 内置 Connector | OpenAI Responses |
| [Gemini CLI](https://github.com/google-gemini/gemini-cli) | 内置 Connector | Gemini |
| [Grok Build](https://github.com/xai-org/grok-cli) | 内置 Connector | OpenAI Chat Completions |
| [Kimi Code](https://github.com/MoonshotAI/kimi-code) | 内置 Connector | OpenAI Chat Completions |
| [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) | 内置 Connector（上游处于开发者预览） | OpenAI Chat Completions |
| [Hermes Agent](https://github.com/NousResearch/hermes-agent) | 内置 Connector | OpenAI Chat Completions |
| [OpenClaw](https://github.com/openclaw/openclaw) | 内置 Connector | OpenAI Chat Completions |
| [WorkBuddy](https://www.workbuddy.ai/) | 内置 Connector | OpenAI Chat Completions |
| [OpenCode](https://github.com/anomalyco/opencode) | 内置 Connector | OpenAI Chat Completions |
| [Cursor](https://github.com/cursor/cursor) | 仅 macOS 专用接入 | OpenAI 兼容端点 |

Claude Desktop 目前没有公开的产品仓库。该链接指向 Anthropic 官方 GitHub 组织页。

DeepSeek Harness 扫描同时覆盖常规 `dsh` 命令，以及 `npx @deepseek-ai/dsh web` 生成的有界 npm 缓存目录。Token Station 只读取已有缓存入口。扫描时不会运行 `npx`，也不会安装软件包。

Grok Build 使用 `~/.grok/config.toml`；设置 `GROK_HOME` 后使用 `$GROK_HOME/config.toml`。Kimi Code 使用 `~/.kimi-code/config.toml`；设置 `KIMI_CODE_HOME` 后使用 `$KIMI_CODE_HOME/config.toml`。DeepSeek Harness 使用 `~/.dsh/settings.yaml` 和 `~/.dsh/.credentials.yaml`；设置 `DSH_HOME` 后，两个文件都位于该目录。这三个 Connector 都通过内置 `agent-openai` 入站适配器转发 OpenAI Chat Completions。

对十一种内置 Connector，点击“一键接入”即代表同意立即应用一份边界明确的计划。Token Station 会在需要时启动网关，并在首次接入后展示已改动字段。Connector 的可用平台取决于对应 Agent 和操作系统，并不代表 Token Station 已为该平台发布安装包。

Cursor 使用仅限 macOS 的独立接入路径。请先退出 Cursor。Token Station 会私有备份相关 SQLite 记录，写入 OpenAI 兼容端点、虚拟 Key 和启用标记，回读校验失败时恢复原值。完成后重新启动 Cursor，并选择支持自定义 OpenAI Key 路径的模型。在 Windows 与 Linux 的凭证和数据库路径具备等价后端前，这两个平台不会宣称支持 Cursor 独立接入。该路径不受标准 Connector 的字段归属和应用内断开流程管理。

## 必须留在本机的工作负载

1. 添加 Ollama 或其他 Base URL Host 经校验属于回环标识的 Provider，并确认这个本地运行时本身不会把请求转发到云服务。
2. 使用单独路由或智能分档，并开启严格本地路由。
3. 关闭云端回退，并确认所选模型使用这个本地端点。
4. 将出站方式设为直连。如果必须配置 HTTP 或 SOCKS 代理，请把准确的回环 Host 加入 `no_proxy`，并确认运行态流量确实绕过代理。

Provider 标签和模型名称本身不能证明请求留在本机。回环端点也只能证明第一跳网络位置。

## 安全

| 边界 | 当前行为 |
|---|---|
| 监听地址 | 拒绝非回环监听地址。默认开启本地鉴权，每次安装生成独立虚拟 Key。 |
| 桌面端请求正文 | 桌面网关会把客户端输入和最终返回给客户端的输出保存为仅当前用户可读的明文 JSON，供“请求日志”查看。清理策略默认使用 7 天保留阈值，并以 1000 个请求文件为清理目标。输入与输出各自上限为 256 KiB，截断时会标记。不会捕获请求头和 Host 注入的供应商凭证，但请求或响应正文自身包含的 Secret 会作为正文内容一并保留。启动时和持续写入期间会定期清理。 |
| 收据日志与指标 | 轮转的 `requests.log` 收据和 `metrics.sqlite` 不包含 Prompt 或 Response 正文。CLI 网关默认不启用独立正文存储。 |
| 云端路由 | 云供应商会收到路由给它的请求。Token Station 无法替代该供应商的数据保留、日志或训练政策。 |
| 供应商凭证 | 默认存储为明文 `secrets.json`，并设置为仅当前用户可读。以同一个操作系统用户运行的其他进程仍可能读取。也支持环境变量和独立文件来源。凭证值不会进入日志、错误或沙箱插件。 |
| 插件沙箱 | WASM 适配器不能直接访问网络、文件系统、环境变量、参数、继承的标准输入输出或明文凭证，并受内存与调用时限约束。 |
| 出站授权 | 在附加凭证前，Host 会确认目标 Origin、路径边界和凭证槽位与已配置供应商一致。 |
| Agent 配置 | 内置 Connector 使用 Revision 与字段归属检查、私有 AES-256-GCM 快照、原子私有写入和恢复流程。快照密钥保存在仅当前用户可读的本地文件中，不是操作系统 Keychain 条目。Cursor 使用上文所述的独立 SQLite 路径。 |

私有文件权限可以把本地状态与其他系统账户隔离。请求正文历史和默认凭证存储都没有静态加密。可以使用[请求正文清理脚本](../scripts/cleanup-request-bodies.sh)，按指定保留天数删除更早的文件。这个 Bash 脚本默认指向 macOS App 数据目录。其他类 Unix 位置需要传入 `--data-dir`。如果凭证保管要求不同，请改用环境变量或单独管理的密钥文件。

## 构建与校验

下列命令块使用 POSIX 兼容 Shell。在 Windows 上，请通过 Git Bash 运行仓库脚本，或把环境变量赋值方式改写为 PowerShell 语法。

桌面开发需要 Rust Stable（MSRV 1.96）、Node.js 22.23.1、npm、`wasm32-wasip2` Target，以及 Tauri 平台依赖。

```bash
git clone https://github.com/ballast-ai/token-station.git
cd token-station
rustup target add wasm32-wasip2
npm --prefix apps/desktop ci
npm --prefix apps/desktop run tauri:dev
```

请使用仓库提供的 `tauri:dev` 命令。它会先构建并内嵌全部官方 WASM 包（4 个 Agent 适配器 + 2 个 South Provider 组件），再启动 Tauri。只开发前端时，可以使用 `npm --prefix apps/desktop run dev`。

```bash
scripts/build-desktop.sh --local
```

在 macOS 上，这个受保护流程会构建、审计、安装、校验并启动本地 App：

```bash
scripts/install-local-desktop.sh
```

```bash
cargo build -p token-station-cli
./target/debug/token-station-cli --help
```

常规 Debug 或 Release Profile 的 Cargo 构建都不会内嵌官方包。本地启动网关时需要提供外部插件目录。官方打包使用 `scripts/build-release.sh <target-triple>`。

Tauri Crate 被排除在根 Cargo Workspace 之外，因此它的 Rust 检查必须单独执行。

```bash
scripts/check-rust-format.sh
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

scripts/prepare-desktop-test-plugins.sh
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
RUSTDOCFLAGS="-D warnings" cargo doc --manifest-path apps/desktop/src-tauri/Cargo.toml --no-deps

npm --prefix apps/desktop run test:coverage
npm --prefix apps/desktop run build
```

完整矩阵见 [CI Workflow](../.github/workflows/ci.yml)。

## 仓库结构

```text
apps/cli/                    原生 CLI 与本地网关
apps/desktop/                React 与 Tauri 桌面 App
crates/                      共享路由、协议、存储和安全 Crate
plugins/official/            官方 WASM 包：4 个北向 Agent 适配器 + 2 个 South Provider 组件
                             （来自 token-station-south）
docs/guides/                 Agent 接入指南
scripts/                     构建、发布、校验和维护脚本
```

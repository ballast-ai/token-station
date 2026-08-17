<div align="center">
  <img src="apps/desktop/public/icon.png" alt="Token Station 图标" width="112" />

# Token Station

### 面向 AI Agent 与 LLM 供应商的本地路由控制台

让 Claude Code、Codex、Gemini CLI、WorkBuddy 等 Agent 统一连接一个仅允许回环监听、默认开启鉴权的本地网关。你可以把流量固定到指定供应商和模型，也可以按任务复杂度分档，或更合理地使用自己 API 账户的额度周期与本地模型。

[![Release](https://img.shields.io/github/v/release/ballast-ai/token-station?display_name=tag&sort=semver)](https://github.com/ballast-ai/token-station/releases/latest) [![CI](https://github.com/ballast-ai/token-station/actions/workflows/ci.yml/badge.svg)](https://github.com/ballast-ai/token-station/actions/workflows/ci.yml) [![License](https://img.shields.io/github/license/ballast-ai/token-station)](LICENSE)

[下载](https://github.com/ballast-ai/token-station/releases/latest) · [快速开始](#快速开始) · [文档](docs/README.md) · [提交问题](https://github.com/ballast-ai/token-station/issues) · [English](README.md)
</div>

> 当前源码可能领先于最新公开版本。每个 Release 页面列出的安装包、架构、签名、校验值和升级说明，才是该版本发布产物的事实来源。

## Token Station 能做什么

Token Station 在 AI Agent 与模型供应商之间保留一个本地网关。路由决策、供应商配置、额度估算和用量元数据由本机控制，请求则会发往你实际选中的本地或云端上游。

| 关注点 | 当前行为 |
|---|---|
| 本地网关 | Rust 代理只允许回环监听，默认在 `127.0.0.1:8787` 开启鉴权 |
| 路由 | 单独路由、智能分档和额度优先，支持全局默认与 Agent 独立覆盖 |
| 入站协议 | Anthropic Messages、OpenAI Chat Completions、OpenAI Responses 和 Gemini |
| 上游 | 40 多个可编辑的托管与本地预设、精选免费或试用目录，以及自定义 OpenAI 兼容端点 |
| 控制入口 | Tauri 桌面 App 与原生 CLI，共用同一套 Rust 内核 |
| 本地状态 | 供应商配置、凭证、路由规则、额度估算、请求收据、指标，以及桌面端请求正文历史 |

## 工作原理

<p align="center">
  <picture>
    <source media="(max-width: 600px)" srcset="docs/assets/token-station-architecture-zh-mobile.svg">
    <img src="docs/assets/token-station-architecture-zh.svg" alt="Token Station 请求路由架构" width="720">
  </picture>
</p>

1. 已接入的 Agent 使用自己的原生协议请求回环网关，网关默认要求本地鉴权。
2. Token Station 应用该 Agent 的路由，选择一个供应商与模型，并在需要时转换协议。
3. Rust Host 校验目标地址与凭证槽位，再按需解析、注入凭证并转发请求。
4. 响应会被转换回 Agent 所需协议，路由、耗时、用量和额度元数据保存在本机。

网关只接受回环监听地址。路由到云供应商的请求仍会离开设备，并受该供应商的数据政策约束。

## 当前能力

- **三种路由模式。** 单独路由把请求固定到一个已管理的供应商和模型。智能分档按照显式规则、Agent 提示、确定性启发式判断和默认配置选择高、中、低档；它只做一次路由决策，不会先尝试便宜档位再静默升级。额度优先先选择更早的重置桶，没有重置窗口的账户与按量账户排在最后；同一个桶内再综合会话亲和、瞬时速率余量和压力，配置顺序只用于打破完全同分。剩余额度只作为耗尽门槛，不会按照“剩余最多”排序。
- **Agent 独立路由。** 每个 Agent 可以继承主页路由，也可以覆盖模式和目标。智能分档支持自定义映射或复用路由方案。额度账户仍是全局共享池，不是每个 Agent 一套独立余额。
- **供应商与模型管理。** 从 40 多个可编辑的官方、托管和本地预设开始，也可以添加自定义 OpenAI 兼容端点；支持模型发现与对比、能力和限制记录，以及模型级健康检查。添加选中的免费或试用条目前，会通过一次真实 Completion 验证连通性和协议行为；这不能证明后续请求持续免费，优惠可用性与计费仍由供应商控制。
- **用量与诊断。** 按时间、Agent、供应商和模型筛选，查看请求数、Token、估算成本、成功率、P95 延迟、额度状态、路由尝试和协议转换。Agent 预算目前只用于提示，不会阻断流量。
- **受控的供应商出站。** 供应商请求、模型发现与健康探测可以使用直连、HTTP CONNECT 或 SOCKS5，并提供经过校验的 `no_proxy` 规则与独立代理凭证。这些流量不会静默继承环境中的代理变量。桌面更新器使用独立 HTTP 栈，可能遵循系统或环境代理设置。
- **可逆的 Agent 接入。** 内置 Connector 使用边界明确的变更计划、字段归属检查、私有加密快照、原子写入和恢复流程。断开时只移除 Token Station 拥有的字段，不删除无关配置。
- **沙箱化 WASM 适配器。** 5 个官方适配器覆盖四种入站协议与 OpenAI 兼容供应商。适配器不能直接访问网络、文件系统、环境变量、参数、标准输入输出或明文凭证；受限操作由 Rust Host 托管，并限制内存与调用时间。
- **以桌面端为主的操作体验。** App 提供首次使用引导、Agent 重新扫描、供应商、用量、设置、明暗主题、中英文、请求日志查看、加密 Connector 快照、供应商回收站和安全模式只读导出。带签名的应用内更新检查与安装只适用于受支持的官方 macOS 构建；没有正式公钥的源码或本地构建，以及 Windows 和 Linux，需要手动更新。插件管理目前属于 CLI 工作流，并不是已挂载的桌面页面。

供应商预设只是可编辑的起点，不代表可用性承诺。模型、免费优惠、地区、价格和限额可能由供应商调整。

## macOS 后台常驻与状态栏

在 macOS 上，关闭主窗口只会隐藏窗口，不会退出 Token Station。App 进程会继续常驻；如果本地代理正在运行，已接入的 Agent 仍可继续发起请求。

菜单栏状态项提供以下能力：

- 查看当前代理状态和监听地址；
- 启动、停止或重试代理；
- 查看已管理 Agent 数量，并直接进入对应路由页面；
- 快速打开“添加供应商”“请求日志”和“设置”；
- 重新打开现有窗口，或退出 Token Station。

点击 Dock 图标，或从菜单栏选择“打开 Token Station”，会重新打开现有窗口。使用菜单中的“退出 Token Station”，才会结束 App 进程并停止本地代理。

这里的后台常驻仅表示关闭窗口后桌面进程仍然存活。它不是系统守护进程，不会自动设为登录项，也不承诺重启系统后自动启动或崩溃后自动拉起。

## 支持的 Agent

| Agent | 接入方式 | 入站协议 |
|---|---|---|
| <a href="https://github.com/anthropics/claude-code"><img src="docs/assets/agents/claude-code.svg" width="20" height="20" alt=""> Claude Code</a> | 内置 Connector | Anthropic Messages |
| <a href="https://github.com/anthropics"><img src="docs/assets/agents/claude-desktop.svg" width="20" height="20" alt=""> Claude Desktop</a> | 内置 Connector | Anthropic Messages |
| <a href="https://github.com/openai/codex"><img src="docs/assets/agents/codex.svg" width="20" height="20" alt=""> Codex</a> | 内置 Connector | OpenAI Responses |
| <a href="https://github.com/google-gemini/gemini-cli"><img src="docs/assets/agents/gemini-cli.svg" width="20" height="20" alt=""> Gemini CLI</a> | 内置 Connector | Gemini |
| <a href="https://github.com/xai-org/grok-cli"><img src="docs/assets/agents/grok-build.svg" width="20" height="20" alt=""> Grok Build</a> | 内置 Connector | OpenAI Chat Completions |
| <a href="https://github.com/MoonshotAI/kimi-code"><img src="docs/assets/agents/kimi-code.svg" width="20" height="20" alt=""> Kimi Code</a> | 内置 Connector | OpenAI Chat Completions |
| <a href="https://github.com/deepseek-ai/deepseek-harness"><img src="docs/assets/agents/deepseek-harness.svg" width="20" height="20" alt=""> DeepSeek Harness</a> | 内置 Connector（上游处于开发者预览） | OpenAI Chat Completions |
| <a href="https://github.com/NousResearch/hermes-agent"><img src="apps/desktop/public/agents/hermes.png" width="20" height="20" alt=""> Hermes Agent</a> | 内置 Connector | OpenAI Chat Completions |
| <a href="https://github.com/openclaw/openclaw"><img src="docs/assets/agents/openclaw.svg" width="20" height="20" alt=""> OpenClaw</a> | 内置 Connector | OpenAI Chat Completions |
| <a href="https://www.workbuddy.ai/"><img src="apps/desktop/public/agents/workbuddy.png" width="20" height="20" alt=""> WorkBuddy</a> | 内置 Connector | OpenAI Chat Completions |
| <a href="https://github.com/anomalyco/opencode"><img src="docs/assets/agents/opencode.svg" width="20" height="20" alt=""> OpenCode</a> | 内置 Connector | OpenAI Chat Completions |
| <a href="https://github.com/cursor/cursor"><img src="docs/assets/agents/cursor.svg" width="20" height="20" alt=""> Cursor</a> | macOS 与 Windows 专用接入 | OpenAI 兼容端点 |

Claude Desktop 目前没有公开的产品仓库；该链接指向 Anthropic 官方 GitHub 组织页。

DeepSeek Harness 扫描同时覆盖常规 `dsh` 命令，以及上游推荐的 `npx @deepseek-ai/dsh web` 所生成的有界 npm 缓存目录。Token Station 只读取已有缓存入口，扫描时不会运行 `npx`，也不会安装软件包。

Grok Build 使用 `~/.grok/config.toml`；设置 `GROK_HOME` 后，使用 `$GROK_HOME/config.toml`。Kimi Code 使用 `~/.kimi-code/config.toml`；设置 `KIMI_CODE_HOME` 后，使用 `$KIMI_CODE_HOME/config.toml`。DeepSeek Harness 使用 `~/.dsh/settings.yaml` 和伴随文件 `~/.dsh/.credentials.yaml`；设置 `DSH_HOME` 后，两个文件都位于该目录。三个 Connector 都通过内置 `agent-openai` 入站适配器转发 OpenAI Chat Completions。

对十一种内置 Connector，点击“一键接入”即代表同意立即应用一份边界明确的计划。Token Station 会在需要时启动网关，并在首次接入后展示已改动字段。Connector 的可用平台取决于对应 Agent 和操作系统，并不代表 Token Station 已为该平台发布安装包。

Cursor 在 macOS 与 Windows 上使用独立接入路径。请先退出 Cursor。Token Station 会私有备份相关 SQLite 记录，以事务方式写入 OpenAI 兼容端点、虚拟 Key 和启用标记，回读校验失败时恢复原值。完成后重新启动 Cursor，并选择支持自定义 OpenAI Key 路径的模型。该路径不受标准 Connector 的字段归属和应用内断开流程管理。

## 下载与安装

你可以从 **[GitHub Releases](https://github.com/ballast-ai/token-station/releases/latest)** 下载已发布版本，也可以自行构建当前源码。

发布产物与具体版本绑定。Tauri 或 Rust 配置中存在某个平台 Target，不代表该平台一定发布了安装包或 CLI 归档。请在所选 Release 页面核对操作系统、CPU 架构、签名或公证状态、校验值与最低系统版本。

如果某个版本提供未签名或未公证的 macOS 测试 DMG，请先核对发布的 SHA-256，并遵循 macOS 安装提示。不要全局关闭 Gatekeeper。如果需要基于已审阅源码生成的本地产物，请自行构建；如果必须使用 Developer ID 签名且经过公证的二进制，只能选择明确提供此类产物的 Release。

## 快速开始

你至少需要一个供应商 API Key，或一个本地模型端点。Token Station 不会把 Claude、Codex 等 Agent 的订阅和 OAuth 会话导入成供应商账户。

1. 打开 Token Station，选择“添加供应商”。从预设中选择，或填写自定义 OpenAI 兼容端点，然后添加模型；只有端点要求鉴权时才需要配置凭证。
2. 等待启动时的 Agent 扫描完成。打开“主页”左侧固定首行，选择单独路由、智能分档或额度优先，配置目标后应用全局路由。
3. 从主页侧栏选择已发现的 Agent。如果缺少某个安装，点击“重新扫描”即可，无需重启 App。对内置 Connector，点击“一键接入”会在需要时启动网关，并应用边界明确的配置改动。Cursor 请遵循上文的独立接入提示。
4. 从已接入的 Agent 发起一次请求。按照默认设置，它会使用本地鉴权连接 `127.0.0.1:8787`。
5. 进入“用量”，查看实际路由、Token、延迟、失败、额度状态、估算成本，以及可用的已保留请求正文。

额度优先会优先使用可识别的供应商限额响应头。没有权威响应头时，本地估算只能看到经过本网关的流量，无法知道同一个凭证在其他客户端的消耗。

### 必须留在本机的工作负载

如果某类工作绝不能发送到云供应商：

1. 添加 Ollama 或其他 Base URL Host 经校验属于回环标识的 Provider，并确认这个本地运行时本身不会把请求转发到云服务。
2. 使用单独路由或智能分档，并开启严格本地路由。
3. 关闭云端回退，并确认所选模型使用这个本地端点。
4. 将出站方式设为直连；如果必须配置 HTTP 或 SOCKS 代理，请把准确的回环 Host 加入 `no_proxy`，并确认运行态流量确实绕过代理。

不要为这类工作使用额度优先。它当前的路由路径没有应用严格本地候选过滤。Provider 标签和模型名称本身不能证明请求留在本机，回环端点也只能证明第一跳网络位置。

## 安全与数据边界

| 边界 | 当前行为 |
|---|---|
| 监听地址 | 拒绝非回环监听地址。默认开启本地鉴权，每次安装生成独立虚拟 Key。 |
| 桌面端请求正文 | 桌面网关会把客户端输入和最终返回给客户端的输出保存为仅当前用户可读的明文 JSON，供“请求日志”查看。清理策略默认使用 7 天保留阈值，并以 1000 个请求文件为清理目标；输入与输出各自上限为 256 KiB，截断时会标记。不会捕获请求头和 Host 注入的供应商凭证，但请求或响应正文自身包含的 Secret 会作为正文内容一并保留。启动时和持续写入期间会定期清理。 |
| 收据日志与指标 | 轮转的 `requests.log` 收据和 `metrics.sqlite` 不包含 Prompt 或 Response 正文。CLI 网关默认不启用独立正文存储。 |
| 云端路由 | 云供应商会收到路由给它的请求。Token Station 无法替代该供应商的数据保留、日志或训练政策。 |
| 供应商凭证 | 默认存储为明文 `secrets.json`，并设置为仅当前用户可读。以同一个操作系统用户运行的其他进程仍可能读取。也支持环境变量和独立文件来源。凭证值不会进入日志、错误或沙箱插件。 |
| 插件沙箱 | WASM 适配器不能直接访问网络、文件系统、环境变量、参数、继承的标准输入输出或明文凭证，并受内存与调用时限约束。 |
| 出站授权 | 在附加凭证前，Host 会确认目标 Origin、路径边界和凭证槽位与已配置供应商一致。 |
| Agent 配置 | 内置 Connector 使用 Revision 与字段归属检查、私有 AES-256-GCM 快照、原子私有写入和恢复流程。快照密钥保存在仅当前用户可读的本地文件中，不是操作系统 Keychain 条目。Cursor 使用上文所述的独立 SQLite 路径。 |

私有文件权限可以把本地状态与其他系统账户隔离，但请求正文历史和默认凭证存储都没有静态加密。可以使用[请求正文清理脚本](scripts/cleanup-request-bodies.sh)，按指定保留天数删除更早的文件。这个 Bash 脚本默认指向 macOS App 数据目录；其他类 Unix 位置需要传入 `--data-dir`。如果凭证保管要求不同，请改用环境变量或单独管理的密钥文件。

## 从源码构建与运行

下列命令块使用 POSIX 兼容 Shell。在 Windows 上，请通过 Git Bash 运行仓库脚本，或把环境变量赋值方式改写为 PowerShell 语法。

### 桌面 App

桌面开发需要 Rust Stable，MSRV 为 1.95；还需要 Node.js 22.23.1、npm、Rust 的 `wasm32-wasip2` Target，以及开发环境文档列出的 Tauri 平台依赖。

```bash
git clone https://github.com/ballast-ai/token-station.git
cd token-station
rustup target add wasm32-wasip2
npm --prefix apps/desktop ci
npm --prefix apps/desktop run tauri:dev
```

请使用仓库提供的 `tauri:dev` 命令。它会先构建并内嵌 5 个官方 WASM 适配器，再启动 Tauri。只开发前端时，可以使用 `npm --prefix apps/desktop run dev`。

使用以下命令构建经过审计的本地 Bundle：

```bash
scripts/build-desktop.sh --local
```

在 macOS 上，仓库还提供一次完成构建、审计、安装、校验和启动的受保护流程：

```bash
scripts/install-local-desktop.sh
```

### CLI

CLI 只要求 Rust Stable，MSRV 为 1.95；具体平台工具链要求见开发环境文档。

```bash
cargo build -p token-station-cli
./target/debug/token-station-cli --help
```

常规 Debug 或 Release Profile 的 Cargo 构建都不会内嵌 5 个官方适配器。本地启动网关时需要提供外部插件目录。官方打包使用 `scripts/build-release.sh <target-triple>`，该流程会构建适配器，并在满足发布凭证要求后开启内置插件 Feature。

<details>
<summary><strong>核心本地门禁</strong></summary>

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

CI 还会执行依赖策略与漏洞检查、Rust 覆盖率与 MSRV 门禁、桌面安全与发布检查，以及平台专用任务。完整权威矩阵见 [CI Workflow](.github/workflows/ci.yml)。

</details>

## 仓库结构

```text
apps/cli/                    原生 CLI 与本地网关
apps/desktop/                React 与 Tauri 桌面 App
crates/                      共享路由、协议、存储和安全 Crate
plugins/official/            5 个官方 WASM 适配器
docs/product/                用户文档
docs/guides/                 Agent 接入指南
docs/contributing/           架构与开发文档
scripts/                     构建、发布、校验和维护脚本
```

## 文档

- [Agent 接入指南](docs/guides/)

## 参与贡献

欢迎提交问题和边界清晰的 Pull Request。请先阅读贡献流程。内部设计记录与这个公开仓库分开管理。

## 许可证

本项目采用 [Apache License 2.0](LICENSE)。

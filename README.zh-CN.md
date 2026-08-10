<div align="center">
  <img src="apps/desktop/public/icon.png" alt="Token Station 图标" width="112" />

# Token Station

### 面向 AI Agent 与 LLM 供应商的本地路由控制台

让 Claude Code、Codex、Gemini CLI 等 Agent 统一连接一个本地网关，再按任务复杂度或账户剩余额度，把每次请求路由到你的 API 供应商或本地模型。

[![Release](https://img.shields.io/github/v/release/ballast-ai/token-station?display_name=tag&sort=semver)](https://github.com/ballast-ai/token-station/releases/latest) [![CI](https://github.com/ballast-ai/token-station/actions/workflows/ci.yml/badge.svg)](https://github.com/ballast-ai/token-station/actions/workflows/ci.yml) [![Platform](https://img.shields.io/badge/public%20desktop-macOS%20Apple%20Silicon-lightgrey.svg)](https://github.com/ballast-ai/token-station/releases/latest) [![License](https://img.shields.io/github/license/ballast-ai/token-station)](LICENSE)

[下载](https://github.com/ballast-ai/token-station/releases/latest) · [文档](docs/README.md) · [提交问题](https://github.com/ballast-ai/token-station/issues) · [English](README.md)
</div>

## 为什么需要 Token Station？

不同 Agent 使用不同的配置文件和请求协议，不同模型供应商又有各自的端点、模型、额度和重置周期。静态配置切换只能在 Agent 启动前选定一个供应商。Token Station 持续运行在本机请求链路中，因此可以为每次请求单独选择模型，同时把路由决策留在你的机器上。

- 复杂任务交给高能力模型，常规任务交给便宜模型或本地模型。
- 优先消耗即将重置的套餐额度，再回退到按量计费供应商。
- 每个 Agent 可以使用独立路由，也可以统一继承全局策略。
- 路由规则、供应商凭证、用量元数据和成本估算都由本机控制。

## 工作原理

```text
Claude Code / Codex / Gemini CLI / 其他 Agent
                         │
                         ▼
              127.0.0.1:8787 + 本地鉴权
                         │
                  Agent WASM 适配器
                         │
                         ▼
        ┌────────── 本地路由器 ──────────┐
        │ 三档路由：高 / 中 / 低         │
        │ 额度优先：重置时间 + 剩余额度  │
        └────────────────────────────────┘
                         │
               Provider WASM 适配器
                   ┌─────┴─────┐
                   ▼           ▼
               云端 BYOK   Ollama / 本地模型
```

网关只允许绑定回环地址。路由到云供应商的请求仍然会离开设备，并受该供应商的数据政策约束。严格本地路由只允许经过校验的回环 Provider。

## 核心能力

- **两种路由模式。** 三档路由根据显式关键词、请求能力和确定性复杂度分数，把任务分到高、中、低三档模型；额度优先路由会优先选择更值得在重置前消耗的账户。
- **每个 Agent 独立配置。** Agent 可以跟随全局路由、使用独立路由，或挂载一份可复用的路由方案。
- **丰富的供应商目录。** 桌面端内置 40 多个可编辑预设，覆盖官方 API、托管推理服务和 Ollama，另有免费或试用额度目录，也支持自定义 OpenAI 兼容端点。
- **沙箱化 WASM 插件架构。** 5 个官方适配器覆盖 Anthropic Messages、OpenAI Chat Completions、OpenAI Responses、Gemini 和 OpenAI 兼容供应商。适配器不能直接访问网络、文件系统、环境变量或明文凭证；需要权限的操作统一由 Rust 宿主校验。
- **用量与成本可见。** 查看请求数、Token、延迟、错误、额度周期和成本估算，Token Station 的请求日志和指标库不保存 Prompt 或 Response 正文。
- **故障处理。** 异常上游会被临时摘除并进入冷却；供应商诊断会分别报告 DNS、TLS、HTTP、鉴权、模型访问和生成失败。
- **可逆的 Connector 接入。** 八种内置 Connector 会生成边界明确的变更计划，只写归属字段，保留私有备份；断开时只移除 Token Station 注入的字段，不删除无关配置。
- **Rust 原生内核，桌面端与 CLI 共用。** 本地网关、路由器、凭证解析、指标和插件宿主都由 Rust 实现。Tauri 桌面端与原生 CLI 复用同一套内核，不重复实现路由逻辑。

供应商预设只是可编辑的起点，不代表可用性承诺。模型、免费额度、地区和限额可能由供应商随时调整。

## 支持的 Agent

| Agent | 接入方式 | 入站协议 |
|---|---|---|
| Claude Code | 内置 Connector | Anthropic Messages |
| Claude Desktop | 内置 Connector | Anthropic Messages |
| Codex | 内置 Connector | OpenAI Responses |
| Gemini CLI | 内置 Connector | Gemini |
| Hermes Agent | 内置 Connector | OpenAI Chat Completions |
| OpenClaw | 内置 Connector | OpenAI Chat Completions |
| WorkBuddy | 内置 Connector | OpenAI Chat Completions |
| OpenCode | 内置 Connector | OpenAI Chat Completions |
| Cursor | macOS 与 Windows 专用接入 | OpenAI 兼容端点 |

对于八种内置 Connector，点击“一键接入”会生成并立即应用边界明确的计划，首次接入会在写入后展示改动字段。Cursor 使用独立路径：先退出 Cursor，再由专用接入备份并修改本地 SQLite 中的两个设置。Cursor 不在 Connector 字段归属和应用内断开流程内，也可以选择手动配置。

## 下载与安装

请从 **[GitHub Releases](https://github.com/ballast-ai/token-station/releases/latest)** 下载当前版本。

### 当前公开产物

| 目标 | 状态 |
|---|---|
| macOS 桌面端，Apple Silicon | 已提供 `token-station_*_aarch64.dmg` 资产 |
| macOS CLI，Apple Silicon | 已提供 `token-station-cli-*-aarch64-apple-darwin.tar.gz` 资产 |
| macOS Intel 桌面端 | 当前公开 Release 未提供 |
| Windows 桌面端 | 已有构建和安装器测试，当前公开 Release 未提供安装包 |
| Linux 桌面端 | 已有构建流程，当前公开 Release 未提供安装包 |

请使用 macOS 11.0 或更新版本。可执行文件的部署目标是 11.0，但当前 App bundle 元数据仍显示 10.13，后续需要校正。桌面端和 CLI 独立版本化，因此资产版本号可能不同。确认 Mac 芯片时，可以打开“苹果菜单 > 关于本机”，也可以运行 `uname -m`，结果为 `arm64` 即 Apple Silicon。

当前 DMG 尚未完成 Developer ID 签名和 Apple 公证，macOS 可能阻止首次启动。把 Token Station 拖入“应用程序”后，右键点击 App，选择“打开”，再确认一次“打开”。如果 Gatekeeper 仍保留隔离属性，可以运行：

```bash
xattr -dr com.apple.quarantine /Applications/token-station.app
```

不要为了安装 Token Station 而全局关闭 Gatekeeper。

当前 Release 也没有可供独立校验完整性的校验和或签名资产。如果你的威胁模型要求可验证二进制，请从源码构建，或等待签名版本，不要移除隔离属性。

## 快速开始

你至少需要一个供应商 API Key，或一个本地模型端点。Token Station 不会把 Claude、Codex 等 Agent 的订阅或 OAuth 会话导入成供应商账户。额度优先会优先使用可识别的供应商限额响应头；没有权威响应头时，只能根据本网关观察到的流量估算已配置额度，无法看到同一 Key 在其他客户端的消耗。

1. 打开 Token Station，选择“添加供应商”。从预设中选择，或填写自定义 OpenAI 兼容端点，然后添加模型和 API Key。
2. 进入“路由”，选择三档路由或额度优先，配置模型或账户，再点击“保存并应用”。这会启动或热更新本地代理。
3. 进入“Agent”并扫描本机安装。对内置 Connector，点击“一键接入”就会立即写入，随后展示本次改动；Cursor 请遵循上面的专用接入提示。
4. 从已接入的 Agent 发起一次请求。它会连接到带鉴权的回环网关 `127.0.0.1:8787`。
5. 进入“用量”，确认路由结果、Token、延迟、失败、额度状态和成本估算。

如果某类工作绝不能离开设备，请添加 Ollama 等本地 Provider 并开启严格本地路由。不要只根据模型名称判断请求是否留在本机。

## 安全与数据边界

| 边界 | 当前行为 |
|---|---|
| 监听地址 | 客户端拒绝非回环监听地址。默认开启本地鉴权，每次安装生成独立虚拟 Key。 |
| 请求正文 | Token Station 转发期间，请求与响应正文会存在于内存，但不会被写入 Token Station 自己的请求日志或指标库。自动测试会扫描这些存储中的测试标记。 |
| 云端路由 | 云供应商会收到路由给它的请求。Token Station 无法替代该供应商的数据保留、日志或训练政策。 |
| 供应商凭证 | 默认写入明文 `secrets.json`，仅允许当前用户账户读取；以同一用户身份运行的其他进程仍可能读取。也支持环境变量和独立文件。凭证值不会进入日志、错误或沙箱插件。 |
| 插件沙箱 | WASM 适配器没有网络、文件系统、环境变量、参数或继承的标准输入输出，并受内存和调用时限约束。 |
| 出站授权 | 在把凭证加入请求前，Host 会确认目标地址和凭证名称与用户配置的供应商一致。 |
| Agent 配置 | 点击“一键接入”即代表同意写入。内置 Connector 使用边界明确的计划、revision 复核、私有备份或字段归属、原子写入和恢复流程，首次改动在写入后展示。Cursor 使用上文所述的独立 SQLite 路径。 |

私有文件权限可以把默认凭证存储与其他系统账户隔离，但凭证没有静态加密。如果你的威胁模型要求其他保管方式，请改用环境变量或单独管理的密钥文件。

## 常见问题

<details>
<summary><strong>Token Station 是云网关吗？</strong></summary>

不是。路由器和代理运行在你的机器上，不需要 Token Station 账号。只有当选中的上游是你配置的云供应商，或你明确触发公开目录、价格或版本查询时，流量才会访问对应的云服务。

</details>

<details>
<summary><strong>它和切换供应商配置有什么区别？</strong></summary>

配置切换器在 Agent 运行前选定一个供应商。Token Station 把本地网关留在请求链路中，可以按每次请求选择供应商和模型，也能为不同 Agent 使用不同策略，而不需要复制路由引擎。

</details>

<details>
<summary><strong>Token Station 会保存 Prompt 或 Response 吗？</strong></summary>

不会写入它自己的请求日志或指标数据库。这些存储只包含路由决策、闭集状态值、耗时、用量、配置名称和成本估算。请求转发期间，正文仍会存在于进程内存；云供应商也可能按自己的政策保留你发送的内容。

</details>

<details>
<summary><strong>可以使用 Claude 或 Codex 的订阅额度吗？</strong></summary>

不能自动使用。Agent 订阅和 OAuth 会话不是 Token Station 的供应商账户。请配置自己的供应商 API Key、受支持的免费或试用 API，或本地模型端点。

</details>

<details>
<summary><strong>断开 Agent 时会发生什么？</strong></summary>

对八种内置 Connector，断开流程会移除 Token Station 拥有的配置字段，让 Agent 回到官方配置路径，同时保留无关设置。Cursor 的专用 SQLite 接入目前不具备这套受管断开流程。

</details>

<details>
<summary><strong>可以只用本地模型吗？</strong></summary>

可以。添加 Ollama 或其他回环 OpenAI 兼容 Provider，标记为本地，再开启严格本地路由。云端回退是另一项需要单独开启的选项。

</details>

## 文档

- [Agent 接入指南](docs/guides/)

## 本地开发

需要 Rust 1.95 或更新版本、Node.js 22.23.1、npm，以及 Rust 的 `wasm32-wasip2` Target。Tauri 的平台依赖见开发环境文档。

```bash
git clone https://github.com/ballast-ai/token-station.git
cd token-station
rustup target add wasm32-wasip2
npm --prefix apps/desktop ci
npm --prefix apps/desktop run tauri:dev
```

请使用仓库提供的 `tauri:dev`，不要直接调用 Tauri。这个入口会先构建并内嵌桌面网关所需的 5 个官方 WASM 适配器。

<details>
<summary><strong>质量门禁</strong></summary>

```bash
scripts/check-rust-format.sh
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
npm --prefix apps/desktop run test:coverage
npm --prefix apps/desktop run build
```

</details>

## 参与贡献

欢迎提交问题和边界清晰的 Pull Request。请先阅读贡献流程。涉及用户可见界面、交互、状态、契约或发布行为的改动，必须先在 `docs/design/` 编写设计文档，再补测试和实现。

## 项目状态

Token Station 仍处于早期公开发布阶段。本地网关、桌面控制面、两种路由模式、内置 Agent Connector、供应商目录、用量视图和恢复链路已经实现。公开分发范围小于源码兼容矩阵：当前桌面 Release 只提供 macOS Apple Silicon 版本，DMG 还没有 Apple 签名和公证。

## 许可证

本项目采用 [Apache License 2.0](LICENSE)。

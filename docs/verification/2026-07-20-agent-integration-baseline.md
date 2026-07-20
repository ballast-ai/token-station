# Token Station Agent 兼容工程 Task 0 基线

> 取证日期：2026-07-20（Asia/Shanghai）
> 状态：Task 0 基线已建立
> 对应实施计划：[`2026-07-20-agent-discovery-version-compatibility.md`](../superpowers/plans/2026-07-20-agent-discovery-version-compatibility.md)
> 取证方式：仓库、本机命令与官方来源只读检测

## 1. 取证边界

本次只执行：

- Git 提交、工作区与 `router-core` 只读检查；
- 已安装 Agent 的可执行文件定位和版本命令；
- 官方仓库/文档契约核对；
- 现有测试清单、桌面 Rust 测试、前端 build 和工具可用性检查；
- Cargo deny/audit 离线检查。

本次没有：

- 读取 `~/.claude`、`~/.codex`、`~/.config/opencode`、`~/.openclaw` 或 `~/.hermes` 配置内容；
- 安装、升级、修复或卸载任何 Agent；
- 修改、启动或停止用户 Agent 进程；
- 修改 `apps/**`、`crates/**` 或 `plugins/**` 业务代码；
- 修改 `crates/router-core/**`；
- 运行真实模型请求或写入用户配置。

## 2. 仓库实施起点

- 仓库：`/Users/liuwenhao/Desktop/公司/token-station`
- 分支：`feat/claude-code-anthropic-adapter`
- 实施起点提交：`b0c96a846d6db712c98248947cb3354c4bb7d157`
- 起点提交说明：`docs: mark interface research approved`
- `origin/HEAD`：`origin/main`
- 与 `origin/main` 的共同祖先：`b0203217d2385ca6debb5e7783b433525ea493e5`
- 相对 `origin/main`：落后 0、领先 52 个提交

本基线固定完整提交 SHA，不使用可能漂移的分支名或短 SHA。

### 2.1 取证时既有工作区

取证时工作区不是干净状态：

- 暂存变更：0；
- 未暂存的已跟踪变更：1，`D AGENTS.md`；
- 未跟踪文件：22，主要包括既有 `.superpowers/` 会话、架构可视化、2026-07-20 设计/计划文档和既有截图。

这些内容属于用户或前序工作，不得被本专项清理、覆盖、暂存或夹带提交。

业务目录状态：

- `apps/**`：无变更；
- `crates/**`：无变更；
- `plugins/**`：无变更。

## 3. `router-core` 冻结基线

`crates/router-core/**` 在起点提交中共有 11 个已跟踪普通文件、无符号链接。索引和工作区与起点提交一致：

- `git diff HEAD -- crates/router-core`：零差异；
- `git diff --cached HEAD -- crates/router-core`：零差异；
- `git status --short -- crates/router-core`：零输出。

以下 SHA-256 取自固定提交中的 Git blob，不依赖当前脏工作区：

| 模式 | SHA-256 | 路径 |
|---|---|---|
| `100644` | `374186ea547661e7d88b6d68691ef762118b7a84b2f1adf85e5315575b01a2e4` | `crates/router-core/Cargo.toml` |
| `100644` | `bc74405dc3495ef8b85ee8936dd247fc67f5895db5c8139047e614797b9441d4` | `crates/router-core/src/config.rs` |
| `100644` | `d645387fb2a235634804acd04ae51b40d84aaf57f4cb9bb25553b14865b87b20` | `crates/router-core/src/decision.rs` |
| `100644` | `efad1a9cbba3552b7fddd45ea00823cf721c8dac2439e450c13c3309626c4e14` | `crates/router-core/src/features.rs` |
| `100644` | `5eb62942312543c10f9b637803f5c080e9f856407f06b83caff3e2881e1699eb` | `crates/router-core/src/health.rs` |
| `100644` | `3f6940557b460cb79880dcee20f6fbe2a9d8fc1103e1720424da8823dd4f69df` | `crates/router-core/src/lexicon.rs` |
| `100644` | `5439a0fd9c904c2a994d83681bb351fe45cd4ad78f0a90538b78133ea75e4c4d` | `crates/router-core/src/lib.rs` |
| `100644` | `f3f27162934afd2e1934fc5e74d34d315ae51a16985ddf3d1896f886b60527b6` | `crates/router-core/src/route.rs` |
| `100644` | `ec1d2c32855fc63df83f325694839c6f0d61dccef0b3997cdc02ca63f6586c03` | `crates/router-core/src/source.rs` |
| `100644` | `4665adec28c57848c23394cc5cbe8c3129f82386e1551f17d57d5fcbb3a16d50` | `crates/router-core/src/test_support.rs` |
| `100644` | `2fad26d701f535abea0d9a824253742e52d037699cb73a56908ac788df9c226c` | `crates/router-core/tests/routing.rs` |

确定性目录摘要：

- 规范化记录：`MODE␠␠SHA256␠␠PATH\n`；
- 排序：按 Git tree 的路径字节序；
- 汇总算法：对 11 条规范化记录组成的字节流计算 SHA-256；
- 目录摘要：`637c90a0988afa23e059ecf991853101abbd60ade57365645c58eed068a84a1a`。

独立复算确认该值一致。模式、内容、路径、增加、删除和可执行位变化都会改变摘要。

红线有两道门：

1. **范围门**：当前 PR/push 的 base 与 head 之间 `crates/router-core/**` 零差异；
2. **最终基线门**：最终 head 相对本节固定提交零差异，目录摘要仍为上述值。

实施期间若目标分支被其他工作合法修改核心目录，本专项必须暂停并请求重新授权建立基线，不能自动接受新摘要。

## 4. 本机 Agent 可执行文件与版本

| Agent | 规范化可执行位置 | 只读命令 | 本机输出摘要 |
|---|---|---|---|
| Claude Code | `$USER_HOME/.local/bin/claude` | `claude --version` | `2.1.211 (Claude Code)` |
| Codex | `/Applications/ChatGPT.app/Contents/Resources/codex` | `codex --version` | `codex-cli 0.145.0-alpha.18` |
| OpenCode | `/opt/homebrew/bin/opencode` | `opencode --version` | `1.18.2` |
| OpenClaw | `$USER_HOME/.local/bin/openclaw` | `openclaw --version` | `OpenClaw 2026.6.11 (e085fa1)` |
| NousResearch Hermes Agent | `$USER_HOME/.local/bin/hermes` | `hermes --version` | `Hermes Agent v0.18.0 (2026.7.1)` |

五种 Agent 当前均可执行。Hermes 输出为多行，并附带运行环境及更新提示；发现器不得假设版本输出只有一行或只有 SemVer。本次没有执行 Hermes 提示的更新命令。

本表只证明当前 PATH/已知入口可运行，不证明不存在 npm、Homebrew、原生、App 内置或其他 Profile 的并行安装。多实例必须由后续 Scanner 完整枚举。

## 5. 官方契约基线

### 5.1 Claude Code

- 官方项目：[Anthropic Claude Code](https://github.com/anthropics/claude-code)；
- 版本命令：`claude --version`，官方还支持 `-v`；
- 用户配置：`~/.claude/settings.json`；项目配置还可能存在 `.claude/settings.json` 和 `.claude/settings.local.json`；
- `CLAUDE_CONFIG_DIR` 可替换默认配置根目录；
- 自定义入口：`ANTHROPIC_BASE_URL`；主要模型协议为 Anthropic Messages；
- 官方网关契约至少包含 `/v1/messages` 和 `/v1/messages/count_tokens`。

证据：[CLI](https://code.claude.com/docs/en/cli-usage)、[Settings](https://code.claude.com/docs/en/settings)、[Environment variables](https://code.claude.com/docs/en/env-vars)、[LLM gateway](https://code.claude.com/docs/en/llm-gateway)。

### 5.2 Codex CLI

- 官方项目：[OpenAI Codex](https://github.com/openai/codex)；
- 版本命令：`codex --version`；
- 用户配置：`$CODEX_HOME/config.toml`，默认 `~/.codex/config.toml`；
- 受信项目还可能有 `.codex/config.toml`；
- 自定义 Provider 使用 `model_provider` 和 `[model_providers.<id>]`；
- 当前 `wire_api` 只支持 Responses，省略时也默认 Responses。

证据：[基础配置](https://developers.openai.com/codex/config-basic)、[高级配置](https://developers.openai.com/codex/config-advanced)、[配置参考](https://developers.openai.com/codex/config-reference)。

### 5.3 OpenCode

- 官方项目：[Anomaly OpenCode](https://github.com/anomalyco/opencode)；
- 版本命令：`opencode --version`，官方还支持 `-v`；
- 全局配置：`~/.config/opencode/opencode.json`；项目配置：`opencode.json`；
- `OPENCODE_CONFIG`、`OPENCODE_CONFIG_DIR`、`OPENCODE_CONFIG_CONTENT` 提供覆盖入口；
- `provider.<id>.options.baseURL` 指定端点；
- `@ai-sdk/openai-compatible` 对应 Chat Completions，`@ai-sdk/openai` 对应 Responses，不能仅凭 Agent 名猜协议。

证据：[CLI](https://opencode.ai/docs/cli/)、[配置](https://opencode.ai/docs/config/)、[Provider](https://opencode.ai/docs/providers/)。

### 5.4 OpenClaw

- 官方项目：[OpenClaw](https://github.com/openclaw/openclaw)；
- 版本命令：`openclaw --version`，官方还支持 `-V`、`-v`；
- 默认状态目录：`~/.openclaw`；默认配置：`~/.openclaw/openclaw.json`；
- `OPENCLAW_HOME`、`OPENCLAW_STATE_DIR`、`OPENCLAW_CONFIG_PATH` 和 `--profile` 会改变有效位置；
- Provider 使用 `models.providers.<id>.baseUrl` 与 `api`；
- 协议可为 `openai-completions`、`openai-responses`、`anthropic-messages` 等，必须显式绑定。

证据：[CLI](https://docs.openclaw.ai/cli)、[环境变量](https://docs.openclaw.ai/help/environment)、[Provider 配置](https://docs.openclaw.ai/gateway/config-tools)。

### 5.5 NousResearch Hermes Agent

- 官方项目已确认是 [NousResearch/hermes-agent](https://github.com/NousResearch/hermes-agent)，不是 Hermes 模型名称；
- 包名为 `hermes-agent`，主入口为 `hermes`；
- 官方文档主命令是 `hermes version`，当前源码还接受 `hermes --version`、`hermes -V`；
- POSIX 默认配置 `~/.hermes/config.yaml`，Windows 默认根目录 `%LOCALAPPDATA%\hermes`；
- `HERMES_HOME` 和 Profile 会改变有效根目录；
- 模型配置包含 `model.provider`、`model.default`、`model.base_url`、`model.api_mode`；
- 当前运行时区分 `chat_completions`、`codex_responses`、`anthropic_messages`。

证据：[CLI 命令](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/reference/cli-commands.md)、[配置](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/configuration.md)、[模型配置](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/configuring-models.md)、[Provider runtime](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/developer-guide/provider-runtime.md)。

Registry 建议使用稳定 ID `nous-hermes-agent`、展示名 `NousResearch Hermes Agent`，避免与 Hermes 模型系列混淆。

## 6. 尚待版本矩阵验证的事实

| Agent | 后续必须验证 |
|---|---|
| Claude Code | 历史版本输出；npm/native/stable/latest 渠道；各平台安装候选；多实例 |
| Codex | 历史输出；npm、Homebrew、App 内置和原生并存；各平台候选位置 |
| OpenCode | Windows 全局配置最终位置；旧版 JSON/JSONC 文件名和格式范围 |
| OpenClaw | 旧版 Profile/环境变量优先级、协议枚举、容器与多实例 |
| Hermes | `version`/`--version`/`-V` 历史覆盖；旧版 Profile、Windows 路径、多实例 |

默认写入范围必须是用户明确选择的用户级配置。存在项目配置、Profile 或隔离根目录时，未选择作用域不得写入。

## 7. 测试、覆盖率与工具基线

### 7.1 Workspace 边界

根 `Cargo.toml` 只有 8 个 workspace member，并明确 exclude `apps/desktop/src-tauri`。因此：

- `cargo test --workspace` 不覆盖桌面 Rust；
- 根 workspace 覆盖率不覆盖桌面 Rust；
- 根 `cargo deny` 和根 `cargo audit` 不能代替桌面 Cargo.lock 审计。

Task 1 必须建立独立 `desktop-rust`、frontend、coverage、desktop deny/audit 门禁。

### 7.2 已执行验证

| 验证 | 结果 |
|---|---|
| `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml` | 23 passed，0 failed，实际执行 |
| `cargo test --workspace -- --list` | 成功列出 305 个测试；本次只列清单，未执行根全量测试 |
| `npm --prefix apps/desktop run build` | TypeScript + Vite build 成功，40 modules transformed |
| 前端测试 | 当前没有 `test`/`test:coverage` 脚本、框架或测试文件 |
| `cargo deny --locked --offline check` | 根 workspace 退出 0，仅重复依赖 warnings |
| 桌面 `cargo deny --offline` | 因本地缓存缺依赖退出 1；未联网下载，规则结果未知 |
| 根 `cargo audit --no-fetch` | 退出 0 |
| 桌面 `cargo audit --no-fetch` | 退出 0，但存在 17 条 allowed warnings |

### 7.3 工具状态

- Rust channel：`stable`，仓库要求 `rust-version = 1.85`，本机 stable 为 `1.96.1`；
- `cargo-deny 0.20.2`：可用；
- `cargo-audit 0.22.2`：可用；
- `cargo-llvm-cov`：未安装；
- `cargo-nextest`：未安装；
- Node.js `22.23.1`、npm `10.9.8`：可用；
- Vitest、Playwright、ESLint：当前前端依赖中不存在。

当前没有真实覆盖率数字，不得声称已达到 80%。`cargo-llvm-cov` 由 Task 1 明确引入后分别测量 workspace 和桌面 Rust，前端使用独立 coverage。

## 8. 已发现安全依赖问题

桌面 `cargo audit --no-fetch` 虽然退出 0，但 allowed warnings 中包含：

- `glib 0.18.5`；
- `RUSTSEC-2024-0429`；
- 类型：unsoundness。

因此“audit exit 0”不等于桌面依赖安全通过。Task 1 必须：

1. 追踪该依赖来自 Tauri/GTK 的具体路径；
2. 评估可升级版本；
3. 至少使用 `cargo audit -D unsound` 形成门禁；
4. 无法立即升级时，只能形成有期限、负责人、风险依据和移除条件的显式例外；
5. 其余 GTK3/unic unmaintained warnings 也必须定义处理政策。

本基线不擅自升级依赖。

## 9. Task 0 结论与 Task 1 输入

Task 0 已建立以下可执行基线：

- 固定仓库起点和既有脏工作区；
- 固定 `router-core` 11 文件和确定性目录摘要；
- 确认本机五种 Agent 均已安装并取得只读版本；
- 锁定五种 Agent 当前官方身份、配置入口和协议事实；
- 明确历史版本、多实例、平台路径仍需 fixtures 和矩阵测试；
- 实际执行桌面 Rust 23 个测试和前端 build；
- 确认根 workspace 不覆盖桌面 crate；
- 确认覆盖率工具和前端测试基础设施缺失；
- 发现桌面 `glib` unsoundness 审计风险。

Task 1 必须先完成：

1. PR/push 范围门和固定基线门；
2. 独立 desktop-rust CI；
3. 前端测试与 coverage；
4. `cargo-llvm-cov` 安装和版本锁定；
5. workspace、desktop Rust、frontend 三套覆盖率基线；
6. desktop deny/audit 与 unsoundness 阻断策略。

在 Task 1 完成前，不开始 Agent 配置写入实现。

## 10. Task 1 基础设施实测补充

2026-07-20 在固定基线之上完成三套独立覆盖率测量；Task 1 只记录事实，尚未启用
Task 12 的最终 fail-under：

| 范围 | 命令 | 测试结果 | 行覆盖率 |
|---|---|---:|---:|
| 主 Rust workspace | `cargo llvm-cov --workspace --coverage-host-only --summary-only` | 全部通过 | 84.38%（6503/7707） |
| desktop Rust | `cargo llvm-cov --manifest-path apps/desktop/src-tauri/Cargo.toml --summary-only` | 23 passed | 59.36%（958/1614） |
| desktop frontend | `npm --prefix apps/desktop run test:coverage` | 4 passed | 27.77%（90/324） |

主 workspace 的真实 WASM 集成测试会启动子 Cargo；必须使用
`--coverage-host-only`，否则 host 的 `-C instrument-coverage` 会传入
`wasm32-wasip2` 并因缺少 `profiler_builtins` 失败。普通 `cargo test --workspace`
仍是完整回归门，覆盖率门不能替代它。

新增的基础设施包括：

- 受信任的 `pull_request_target` 红线 workflow，只读取候选 Git 对象，不执行候选代码；
- event range 与固定基线两道 `router-core` 门，17 个脚本场景通过；
- 独立 desktop Rust、frontend、两份 Rust coverage 和 desktop security CI job；
- Vitest/jsdom/Testing Library 基线，三个既有 Agent 按钮及 IPC 映射测试 4/4；
- `cargo-llvm-cov 0.8.7` 固定版本；
- desktop 独立 cargo-deny 策略，离线实测 advisories/bans/licenses/sources 均通过；
- `cargo audit -D unsound` 以及 `RUSTSEC-2024-0429` 的精确、到期例外登记。

`glib 0.18.5` 的依赖路径为
`token-station-desktop -> tauri 2.11.5 -> gtk 0.18.2 -> glib 0.18.5`。
截至本次核查，Tauri v2 没有升级到 glib 0.20 的官方支持路径；例外于
2026-09-18 到期，并以 Tauri 迁离 gtk/glib 0.18 或官方回补为强制移除条件。

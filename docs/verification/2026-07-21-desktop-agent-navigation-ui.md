# 桌面 App 五 Agent 独立路由验收

- 日期：2026-07-21
- 对象：桌面主页、五个 Agent 分页、设置中心、用量入口、添加供应商页与 Host 层 Agent 独立路由
- 设计基线：`d87d735`
- 验收结论：本地验证完成

## 1. 需求证据矩阵

| 要求 | 实现证据 | 验证边界 | 结果 |
|---|---|---|---|
| Agent 接入不展示第二次确认 | `AgentRoutePage` 在一次点击内连续执行 plan/apply；仍保留后端短时令牌、快照、ownership 和回滚 | 桌面单测与 Rust 事务测试；未覆盖用户真实配置 | 通过 |
| 可恢复 Agent 原始配置 | Agent 页提供“恢复 Agent 原始配置”，沿用 disconnect 事务 | Rust 恢复/断开测试 | 通过 |
| 虚拟 API Key 移入设置，默认打码可复制 | `SettingsHub::VirtualKeyCard` | App 测试验证打码、显示和 Clipboard 调用 | 通过 |
| 仅五个 Agent，无 Gemini | Claude Code、Codex、OpenCode、OpenClaw、Hermes 固定准入列表 | App 导航测试和真实 App 可访问性树 | 通过 |
| 每个 Agent 独立配置路由 | `agent_routes` 仅保存三档 provider/model 引用；Gateway 使用主页 Router 或 Agent Router | CLI config、proxy 和桌面 Rust 测试 | 通过 |
| 主页保留，可一键作为全部 Agent 默认 | `HomePage` 与 `apply_home_route_to_all_agents` | App 与 Rust 命令测试 | 通过 |
| 主页与 Agent 继续使用原三档供应商/模型选择 | 共用唯一 `TierRouteEditor` | 组件测试、真实 App 主页/Codex 页检查 | 通过 |
| 路由表、插件、关于收入设置；用量保持右上角独立入口 | `SettingsHub` 五个子页；`AppShell` 独立用量 SVG 指标 | App 导航测试与可视化分页检查 | 通过 |
| 右上角添加供应商，切换独立页面 | `AddProviderPage` 保留原预设和模型发现能力 | App 测试与可视化页面检查 | 通过 |
| 浅色/深色/跟随系统 | `ThemeProvider` 同步 DOM、localStorage、系统媒体查询和 Tauri 窗口主题 | Theme 单测、浅/深色截图 | 通过 |

## 2. 新鲜自动化证据

| 命令 | 结果 |
|---|---|
| `npm test -- --run` (`apps/desktop`) | 6 个测试文件，65/65 通过 |
| `npm run build` (`apps/desktop`) | TypeScript 与 Vite 生产构建通过 |
| `cargo fmt --all -- --check` | 通过 |
| `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml` | 118 通过、1 个只读真机探测忽略；另 3 个 YAML 回归通过 |
| `cargo test -p token-station-cli --lib` | 63 通过、1 个真实 Keychain 测试忽略 |
| `cargo test -p token-station-cli --test proxy` | 32/32 通过 |
| `cargo clippy -p token-station-cli --all-targets -- -D warnings` | 通过，零告警 |
| `cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings` | 通过，零告警 |
| `npm run tauri build -- --debug` | `.app` 与 `.dmg` 均成功生成 |
| `git diff --check` | 通过 |

DMG 首次打包时发现上一次失败留下的临时可写镜像仍挂载在 `/dev/disk4`。只卸载该明确的临时镜像后重跑，命令退出码为 0，同时生成 `.app` 和 `.dmg`。

## 3. 真实 App 与视觉验收

对本轮新构建的 debug `.app` 完成真实 Tauri WebView 验收：

- 实际 URL 为 `tauri://localhost`；
- 可访问性树实际读取到五个 Agent 及本机发现/接入状态；
- 主页显示三档供应商/模型、Agent 摘要和供应商列表；
- Codex 页显示真实安装路径、接入状态、跟随/独立模式和同一三档编辑器；
- 只执行了只读导航，没有启动代理、接入/断开 Agent 或修改用户配置。

另使用生产 React/CSS 与只读 IPC 夹具完成界面矩阵检查，覆盖浅色主页、深色主页、Codex、添加供应商、用量、虚拟 Key、路由表、插件、外观和关于。

验收图保存在：

- `/Users/liuwenhao/.codex/visualizations/2026/07/21/019f82a5-49b7-7900-98dc-608c66f4111e/token-station-real-app-home-dark.png`
- `/Users/liuwenhao/.codex/visualizations/2026/07/21/019f82a5-49b7-7900-98dc-608c66f4111e/token-station-real-app-codex-dark.png`
- `/Users/liuwenhao/.codex/visualizations/2026/07/21/019f82a5-49b7-7900-98dc-608c66f4111e/token-station-home-light.png`
- `/Users/liuwenhao/.codex/visualizations/2026/07/21/019f82a5-49b7-7900-98dc-608c66f4111e/token-station-settings-dark.png`
- `/Users/liuwenhao/.codex/visualizations/2026/07/21/019f82a5-49b7-7900-98dc-608c66f4111e/token-station-add-provider-light.png`

## 4. IR / WIT / ABI / Router 红线

本轮 Agent 身份命名空间在 CLI Server 中解析并移除，只在 Host 层选择 Router 实例。标准请求路径继续交给原 Adapter 和 Canonical IR。

以下路径零差异：

- `crates/protocol/**`（Canonical IR）；
- `crates/plugin-api/wit/**`（WIT）；
- `crates/plugin-api/**` 与官方 Adapter（ABI）；
- `crates/router-core/**`（路由纯函数与策略）。

最终提交后仍必须以设计基线执行 `scripts/check-router-core-redline.sh d87d735 HEAD`，并且 event range 与 frozen baseline 两道门都必须通过。

## 5. 边界

- 本记录证明本地代码、构建物与只读运行态，不代表生产发布或签名/公证完成。
- 为避免覆盖用户已有 Agent 配置，未在真实安装上执行接入/断开；对应事务、回滚、ownership 和命名空间已由 Rust/CLI 自动化测试覆盖。

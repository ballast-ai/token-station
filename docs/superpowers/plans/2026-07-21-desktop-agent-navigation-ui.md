# 桌面 App 主页与 Agent 独立路由实施计划

## 目标与事实来源

按
[设计规格](../specs/2026-07-21-desktop-agent-navigation-ui-design.md)
重构 Token Station 桌面端，并真实兑现主页默认路由、五个 Agent 独立路由、一键安全接入、独立添加供应商页面、设置收纳、独立用量入口和全局明暗主题。

事实优先级：当前源码与本轮测试 > 已确认设计规格 > 历史文档。实现不得修改协议 IR、WIT、插件 ABI 或 `crates/router-core/**`。

当前基线已经实测：

- `apps/desktop`: Vitest 51/51，通过；
- `apps/desktop`: TypeScript + Vite 生产构建，通过；
- `apps/desktop/src-tauri`: Rust 测试 117 通过、0 失败、1 个真实探测忽略；
- `token-station-cli --test proxy`: 30/30，通过；
- `scripts/check-router-core-redline.sh HEAD^ HEAD`: 两道门均通过。

## 视觉实现基线

### 产品与单一任务

面向在本机同时使用多个 AI 编码 Agent 的工程用户。界面的单一任务是：让用户快速知道“当前请求由哪套三档路由控制”，并安全地改变它。

### 设计 Token

| 角色 | 浅色 | 深色 |
|---|---|---|
| Canvas | `#F4F6FA` | `#111620` |
| Surface | `#FFFFFF` | `#191F2B` |
| Ink | `#182133` | `#EEF2F8` |
| Muted | `#738096` | `#909CB0` |
| Signal blue | `#5368E8` | `#7284FF` |
| Success | `#1B8A5A` | `#41C98A` |

- 标题：`Avenir Next` / `Segoe UI Variable Display` 回退；
- 正文：系统 UI 字体；
- 版本、路径、Key、端点和指标：`SF Mono` / `Cascadia Mono` / `Menlo`。

视觉签名是“Station signal rail”：左侧侧轨用一条克制的信号线连接主页与五个 Agent，当前站点使用短亮条，状态点表达发现/接入状态。除三档语义块外不增加装饰性渐变。

## 全查影响面地图

| 维度 | 文件/符号或系统 | 动作 | 验证 | 状态 |
|---|---|---|---|---|
| 配置契约 | `apps/cli/src/config.rs::ClientConfig` | 增加可选 Agent 三档引用，不修改 `RouterConfig` 或 IR | 旧配置反序列化/序列化不变；新配置正负向测试 | 必须修改 |
| 路由运行态 | `apps/cli/src/gateway.rs`、`server.rs::AppState/chat/models` | 在同一 Gateway 内物化主页 Router 与 Agent Router 映射；解析并移除本地路径命名空间 | CLI proxy 测试覆盖已知/未知/无命名空间 | 必须修改 |
| CLI 调用方 | `apps/cli/src/main.rs` | 构造 Gateway 集合而非单 Gateway | CLI serve/proxy 测试 | 必须修改 |
| 桌面运行态 | `apps/desktop/src-tauri/src/serve_lifecycle.rs` | 使用同一 Recorder 构造主页和 Agent Gateway | Tauri 生命周期测试 | 必须修改 |
| 桌面草稿 | `apps/desktop/src-tauri/src/lib.rs::AppInner/StateView` | 维护主页与 Agent 路由模式、三档草稿及命令 | Tauri 命令正负向测试 | 必须修改 |
| 供应商引用 | `replace_provider_models/remove_provider` | 同时检查或清理 Agent 自定义路由引用 | 删除/更新模型负向测试 | 必须修改 |
| Agent 接入 | `agent_integration/commands.rs::AgentProxyRuntime` | 为五个 Connector 生成稳定命名空间 Base URL | Connector 计划/复验测试 | 必须修改 |
| Agent 安全事务 | `plan_agent_connection/apply_agent_plan` | 保留两阶段后端，前端一次点击连续调用 | 令牌过期、指纹变化、版本阻断零写入 | 必须保留 |
| 前端契约 | `apps/desktop/src/api.ts` | 增加 Agent 路由类型与命令，现有只读数据面不变 | API 单测 | 必须修改 |
| App 壳 | `App.tsx`、新布局组件 | 侧轨、顶栏、页面状态、最近页面记忆 | App 测试、窄窗测试 | 必须修改 |
| 路由编辑器 | 新 `TierRouteEditor` | 主页和 Agent 页面共用现有三档控件 | 组件测试证明同一实现 | 必须修改 |
| Agent 页面 | 新 `AgentRoutePage` | 一键接入、继承/独立、两类恢复 | UI + Tauri 集成测试 | 必须修改 |
| 供应商流程 | 新 `AddProviderPage`、现有 ModelPicker | 右上角进入独立页面，成功返回来源页 | 表单、发现、取消、失败保持测试 | 必须修改 |
| 设置 | `Settings.tsx`、RouterTable/Plugins/About | 通用/虚拟 Key/路由表/插件/外观/关于；用量排除 | 导航与 Key 打码复制测试 | 必须修改 |
| 主题 | 新 `ThemeProvider`、`App.css` | light/dark/system、OS 监听、原生窗口同步 | DOM class、持久化、视觉截图 | 必须修改 |
| 用量 | `Stats.tsx`、顶栏入口 | 保持独立页，不迁入设置 | 导航与数据回归 | 必须修改入口，数据不改 |
| 指标/隐私 | `crates/metrics/**`、记录结构 | 不新增 Agent 字段，不触碰 prompt 红线 | 原有 canary/metrics 测试 | 已检查，不修改 |
| 协议 IR | `crates/protocol/**`、WIT、官方 Adapter ABI | 不增加 Agent 字段；命名空间在宿主先移除 | `git diff` 路径检查 | 明确不在范围 |
| Router 核心 | `crates/router-core/**` | 零改动 | 红线脚本 event/frozen 两道门 | 明确不在范围 |
| 文档 | 设计、桌面交接、配置说明、验收 | 同步最终行为和测试证据 | 文档搜索、diff check | 必须修改 |

## 必须先核实

1. 五个 Connector 当前写入的 Base URL 均为宿主提供字符串，且客户端会在其后追加标准协议路径；必须用 Connector 单测和真实配置证明命名空间可用。
2. 共享 OpenAI 入站的 OpenCode、OpenClaw、Hermes 不能靠协议或进程猜测身份；只能依赖经过校验的路径命名空间。
3. 旧版无命名空间接入必须继续走主页默认路由；升级到独立路由前不能破坏已有连接。
4. 深色主题必须覆盖所有旧页面和弹层，不能只覆盖新壳。

## Task 1：配置扩展与 Gateway 集合

### 配置

在 `ClientConfig` 增加默认空、空时不序列化的 Host 层扩展：

```text
agent_routes.<agent_id>
  mode = inherit | custom
  custom_route.high = optional { upstream, model }
  custom_route.mid  = optional { upstream, model }
  custom_route.low  = optional { upstream, model }
```

这是 CLI Host 配置扩展，不是协议 IR。它只保存三档引用，不复制 Base URL、API Key、模型目录、rules、hints、阈值或权重。旧配置加载和再次保存时不能凭空出现 `agent_routes`。

校验要求：

- Agent ID 长度和字符集受限；
- custom 必须至少存在一档完整的 `(upstream, model)`；
- 每档必须引用已配置 upstream 及该 upstream 的已配置模型；
- inherit 可保留非活动 custom_route，支持恢复草稿。

### Gateway 集合

保持单一 Gateway、插件集合、catalog、凭据源和健康状态；在 Gateway 内新增主页 Router 与 custom Agent Router 映射。Agent Router 由 Host 克隆主页 `RouterConfig`，只替换三档 pool 成员并按当前统一 helper 重建 bands/default。inherit Agent 直接使用主页 Router。

HTTP 路径：

```text
/agents/claude-code/v1/messages
/agents/codex/v1/responses
/agents/opencode/v1/chat/completions
/agents/openclaw/v1/chat/completions
/agents/nous-hermes-agent/v1/chat/completions
```

Server 先解析已知 Agent ID，再把 `/agents/<id>` 前缀移除，把标准路径交给现有 `match_inbound`。无前缀继续使用主页 Gateway；未知命名空间返回 404。鉴权发生在选择 Gateway 之前，所有 Gateway 共用同一虚拟 Key。

### 验证

```bash
cargo test -p token-station-cli config
cargo test -p token-station-cli --test proxy
scripts/check-router-core-redline.sh <base> HEAD
```

## Task 2：桌面路由草稿与命令

### 状态

扩展 `StateView`：

- `agent_routes[agent_id].mode`；
- `agent_routes[agent_id].tiers`；
- `agent_routes[agent_id].config_error`。

默认对五个 Agent 返回 inherit。主页三档仍来自 `router`。

### 命令

新增：

- `set_agent_route_mode(agent_id, mode)`；
- `set_agent_tier(agent_id, slot, upstream, model)`；
- `save_agent_routes()`；
- `apply_home_route_to_all_agents()`。

提取共享的“设置三档、重建 bands、校验供应商模型”纯 helper，让主页和 Agent 使用同一实现。不能复制一套阈值或权重。

供应商删除时清理所有 profile 中的引用；模型集合更新时，主页或任一 Agent 正在使用的模型都必须阻止删除。

## Task 3：Connector 命名空间与一键安全接入

`AgentProxyRuntime` 为每个 Connector 生成对应 Base URL。其指纹必须覆盖全部 namespaced URL，保证计划和运行时绑定变化时旧令牌失效。

前端接入：点击一次后 `planAgentConnection` 成功即调用 `applyAgentPlan`，不展示二次确认弹窗。断开/恢复继续使用 ownership 和快照边界。

必须保留：

- Registry admission；
- 版本/路径/多实例准入；
- 配置指纹与 compatibility sequence；
- 短时 HMAC 令牌；
- 加密快照、原子写入、写后复验、ownership；
- 失败回滚。

## Task 4：共享前端状态与组件

### 组件

- `AppShell`：侧轨、顶栏、内容区；
- `AgentRail`：主页 + 五个 Agent + 扫描；
- `TopActions`：代理、用量、设置、添加供应商；
- `TierRouteEditor`：复用当前三档 DOM 与语义；
- `HomePage`：默认路由、Agent 摘要、供应商列表；
- `AgentRoutePage`：Agent 状态、路由模式和一键接入；
- `AddProviderPage`：现有预设/模型发现表单的页面化；
- `SettingsHub`：设置子导航；
- `ThemeProvider`：主题唯一真源。

### 页面状态

不引入路由依赖；使用受控 `View` 联合类型。记忆最近主页/Agent 页面，但重启后安全地回退到存在的 Registry Agent。

窄窗时侧轨只保留图标和可访问名称；用量、设置、添加供应商不能被隐藏。

## Task 5：设置收纳、虚拟 Key 与主题

- 虚拟 Key 移出主页，设置内默认固定掩码；复制不要求先显示；显示后自动重新打码。
- RouterTable、Plugins、About 作为 SettingsHub 子页，原组件数据源不变。
- Stats 保持独立 View。
- 主题为 light/dark/system，根节点语义 class、localStorage 和系统媒体查询保持一致；调用新增 Tauri 命令同步原生窗口主题。

## Task 6：测试与真实 App 验收

### 自动化

```bash
cd apps/desktop && npm test -- --run
cd apps/desktop && npm run build
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
cargo test -p token-station-cli --test proxy
cargo test --workspace
scripts/check-router-core-redline.sh <base> HEAD
git diff --check
```

### 真实 App

1. `npm run tauri dev` 启动真实 Tauri 窗口；
2. 浅色检查：主页、五个 Agent、添加供应商、用量、设置全部截图；
3. 深色检查同一组页面；
4. 实际切换主页/Agent、添加页返回、Key 复制、启动/停止代理；
5. 对本机已安装 Agent 只执行安全范围内的扫描；涉及真实配置写入时使用测试 HOME 或夹具，不覆盖用户真实配置；
6. 记录窗口尺寸、主题、测试步骤、截图和发现的问题；
7. 修正后重跑视觉与自动化。

## Task 7：完成审计与交付

逐项对照设计规格的 10 项目标、红线、错误状态和测试要求。最终交付必须包含：

- 实现提交；
- 自动化命令和本轮结果；
- 真实 App 测试记录与浅/深色截图；
- 红线脚本结果；
- 明确说明 IR/WIT/ABI 和 `router-core` 零改动；
- 未解决项为零，否则不能声明完成。

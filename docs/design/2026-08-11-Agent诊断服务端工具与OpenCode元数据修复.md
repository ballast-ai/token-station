# Agent 诊断、服务端工具与 OpenCode 元数据修复

## 问题与目标

本轮修复人肉测试台账中的 UX-005、UX-006 和 UX-007，并修复重启 App 后由旧配置中的 `profiles: null` 触发的全局只读锁定。

1. 请求在本地入站归一化阶段失败时，最近请求回执仍把 `invalid_request` 解释成“上游拒绝”，与 `attempts=0` 冲突。
2. Anthropic `web_search_*` 等服务端工具在翻译链路中被改写成空参数函数。请求虽然不再因缺少 `input_schema` 直接失败，但执行者、结果、引用和用量都不存在，界面与调用方可能误以为搜索已经等价执行。
3. OpenCode 一键接入生成的 `tokenstation/auto` 没有上下文窗口、最大输出和价格。OpenCode 因而无法正确计算 Context、Spent，也会在上下文窗口为零时关闭主动自动压缩。
4. 本机实际配置在 `profiles` 没有内容时保存成了显式 `null`。Serde 对“字段缺省”可使用默认空 Map，但对“字段存在且为 null”会报 `invalid type: null, expected a map`，导致 App 重启后整份配置进入只读保护，代理和全局路由一起被阻断。

目标是让三个边界都保持诚实：本地失败显示为本地失败，不能翻译的服务端工具在发往上游前明确拒绝，OpenCode 得到当前有效路由中可证明且安全的模型元数据。

## 范围与非目标

范围包括：最近请求回执诊断、Anthropic Messages 翻译适配器、Provider `/models` 目录解析与缓存、有效 OpenCode 路由的元数据投影、OpenCode Connector 配置、网关 `/models` 输出，以及旧版可选 Map 的安全加载兼容。

配置兼容只覆盖语义本来就是“可省略空 Map”的 `agent_routes`、`profiles` 和 `agent_budgets`。核心 `upstreams`、`router.pools` 等必需结构继续对 `null` 失败关闭，不能用宽松解析掩盖损坏配置。

本轮不在 Token Station 内实现网页搜索执行器，也不把普通 OpenAI-compatible Provider 冒充为 Anthropic 服务端工具 Provider。原生 `anthropic-native` 链路继续原样透传请求。本轮也不承诺在多个候选模型价格不一致时给 `tokenstation/auto` 生成一个虚假的精确价格。

## 安全与数据红线

1. 回执只使用现有有限枚举、阶段、决策是否存在和尝试次数。不得显示请求正文、字段值、tool input、搜索词、搜索结果或凭据。
2. Provider 目录只接受有限大小、有限模型数量和可验证范围内的非负数值。无效或超界元数据按未知处理，不得污染配置。
3. OpenCode 投影只写 Connector 已拥有的 `provider.tokenstation`。不改用户的其他 Provider、顶层模型或主题配置。
4. 路由有多个可达模型时，上下文和输出上限取所有已知候选的最小值。任一候选缺少必要上限时不声称完整 `limit`；价格只有在所有候选完全一致时才写入。
5. 不把 `think` 独立价格错误映射到 OpenCode。当前 OpenCode 模型结构没有独立 reasoning 价格，只有可证明的 input、output、cache read/write 才投影。

## 用户可见行为与失败处理

### 重启后的旧配置兼容

如果旧配置把 `agent_routes`、`profiles` 或 `agent_budgets` 写成 `null`，加载器把它解释成与字段缺省等价的空 Map，不再让整份配置进入只读保护。序列化时空 Map 继续省略；其他本应为对象的核心字段如果是 `null`，仍显示结构错误并保持只读保护。

如果现有 OpenCode 配置的 `provider` 是对象，但旧接入遗留了 `provider.tokenstation: null`，重新一键接入时 Connector 在自己的拥有路径内把该值替换为完整 Provider Map，并保留顶层 `model` 与其他用户字段。

### 最近请求回执

`inbound_normalize` 失败、没有路由决策且没有上游尝试时，诊断显示“本地请求转换失败，请求尚未发往上游”。如果转换记录提供有限原因码，则进一步说明 JSON、字段结构、能力或工具类型。真实发生过上游尝试的 `invalid_request` 仍显示为上游拒绝。

Decision 和 Attempts 的空状态说明请求在哪个更早阶段停止，避免只显示“没有记录”。中英文含义必须一致。

### Anthropic 服务端工具

翻译链路遇到 `web_search_*`、`web_fetch_*`、`code_execution_*`、`tool_search_*`、`mcp_*` 或 `advisor_*` 时，在本地归一化阶段返回 HTTP 400 `capability`，消息明确包含安全的工具类型和“需要原生 Anthropic 服务端工具 Provider”。回执记录 `provider_tool_unsupported` 和有限工具种类，且 `attempts=0`。

`bash_*`、`text_editor_*`、`computer_*` 和 `memory_*` 仍作为客户端执行工具转换成 Canonical function tool。普通自定义函数保持现有严格 `input_schema` 校验。`anthropic-native` 路径继续绕过 Canonical IR，原样保留服务端工具。

### OpenCode

Provider `/models` 目录可读取下列兼容字段：

- 上下文：`context_window` 或 `limit.context`；
- 最大输出：`max_output_tokens` 或 `limit.output`；
- 价格：`cost.input`、`cost.output`、`cost.cache_read`、`cost.cache_write`。

刷新目录后，这些事实写入对应模型能力和价格表。目录缓存版本升级，旧缓存安全失效，不猜测旧数据。

一键接入 OpenCode 时，后端从 OpenCode 当前有效路由收集所有可达候选。所有候选都有上限时，`tokenstation/auto.limit` 写入最小上下文和最小输出；所有候选价格一致时写入 `cost`。缺失或混合价格时省略 `cost`，成功提示说明价格无法精确投影。网关 `/agents/opencode/v1/models` 同步返回模型自身已有的 `context_window`、`max_output_tokens`、`limit` 和 `cost`，供其他兼容客户端使用。

## 响应式、键盘与可访问性

本轮不改变导航和表单布局。诊断继续使用语义化 section、标题和文本，不依赖颜色表达本地或上游层级；键盘读取顺序保持 Diagnosis、Decision、Attempts、Conversions。

## 公开测试边界与验收标准

1. 前端组件测试覆盖本地 `inbound_normalize + attempts=0` 与真实上游 400 的不同诊断，并验证不渲染敏感详情。
2. Anthropic 插件测试覆盖服务端工具能力拒绝、客户端工具继续转换、自定义工具缺 schema 继续拒绝。
3. 网关测试覆盖翻译链路在上游前拒绝服务端工具，并产生 `provider_tool_unsupported/web_search`；原生 Anthropic 路径继续原样透传。
4. 模型目录测试覆盖测试人员提供的 Wecoding 响应形状、嵌套字段、非法数值、缓存版本和去重合并。
5. OpenCode Connector 测试覆盖完整 limit/cost、混合价格省略 cost、现有用户配置只替换拥有路径、重复接入幂等。
6. 网关 `/models` 测试覆盖元数据输出。
7. 全量测试、桌面构建与产物审计通过；安装 `/Applications/token-station.app` 后真实启动，代理可启动，Agent 页面和最近请求页面可打开。
8. Connector 单元测试使用隔离 HOME；真实 App 验收在用户授权的修复范围内重新接入现有 OpenCode 配置，并确认只替换 `provider.tokenstation`、保留顶层模型和其他用户字段。

## 实现落点

- `apps/desktop/src/errors.ts` 与 `apps/desktop/src/components/RecentReceipts.tsx`：收据感知诊断和空状态。
- `plugins/official/agent-anthropic/src/lib.rs`：服务端工具与客户端工具分流。
- `apps/desktop/src-tauri/src/model_catalog.rs` 与 `apps/desktop/src-tauri/src/lib.rs`：模型上限和价格发现、缓存及持久化。
- `apps/desktop/src-tauri/src/agent_integration/commands.rs`、`connectors/mod.rs`、`connectors/opencode.rs`：有效路由元数据计算与 OpenCode 配置投影。
- `apps/cli/src/gateway.rs`：`/models` 元数据。

## 实现状态、真实 App 验收与遗留项

当前状态：实现和验收完成。Rust workspace 全量测试通过；桌面前端 30 个测试文件、287 项测试通过；桌面 Rust 257 项测试通过、1 项按既有条件忽略；生产前端构建、官方本地桌面构建和产物审计通过。

真实 App 验收使用新安装的 `/Applications/token-station.app` 完成：

1. App 用真实旧配置中的 `profiles: null` 启动，没有进入“配置结构不合法”只读保护；保存后该空字段被正常省略。
2. 代理可启动，全局路由入口可用。退出 App 后代理停止；重新打开 App 后仍无结构错误，代理可再次启动并监听 `127.0.0.1:8787`。
3. 刷新 Wecoding 模型目录后，`glm-5.2` 保存了 `context_window=257550`、`max_output_tokens=32768` 和 `input=0.2`、`output=0.6`、`cache_read=0.04`。
4. 真实 OpenCode 配置原有 `provider.tokenstation: null` 被一键接入安全替换为对象，顶层 `model=tokenstation/auto` 保持不变；`models.auto.limit` 与 `cost` 写入上述值。
5. 重启后的代理 `/agents/opencode/v1/models` 返回相同的 `context_window`、`max_output_tokens`、`limit` 和 `cost`。
6. Agent 页面显示 Claude Desktop、OpenCode 和 WorkBuddy 已接入。自适应 thinking 的翻译与原生透传继续由代理集成测试覆盖。

已知遗留项：OpenCode 当前没有独立 reasoning 价格字段；当智能路由候选价格不同，`tokenstation/auto` 的单一静态 `cost` 无法准确表达每次动态选路成本，因此保持未知，不伪造 Spent。测试人员现场的“接近阈值后自动压缩”需要一段真实长会话才能观察，本轮已经修复并验证其前提条件，即非零且正确的上下文窗口和输出上限。

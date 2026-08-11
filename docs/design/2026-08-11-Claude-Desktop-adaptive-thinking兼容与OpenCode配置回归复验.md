# Claude Desktop adaptive thinking 兼容与 OpenCode 配置回归复验

- 日期：2026-08-11
- 状态：已实现、已安装并完成真实 App 验收
- 范围：Claude Desktop 专用 Anthropic 入站、既有 OpenCode/桌面配置草稿回归

## 问题、目标、范围和非目标

Claude Desktop 通过 `/agents/claude-desktop/v1/messages` 接入 Token Station 时，会在普通
请求中发送 `thinking.type = "adaptive"`。原生 Anthropic 上游走
`anthropic-native` 透传，不经过入站归一化；OpenAI-compatible 上游则进入
`agent-anthropic` 的 Canonical IR 翻译路径。当前适配器只接受 `thinking.type =
"disabled"`，因此请求在路由和上游调用前被本地拒绝：

```text
API Error: 400 Anthropic thinking type adaptive is not supported by the configured provider
```

本次目标是让 Claude Desktop 的 adaptive 请求可以使用现有兼容型上游。仅把 adaptive
视为客户端的可选推理偏好；若请求同时提供 `output_config.effort`，继续通过现有
`reasoning_effort` 扩展交给声明支持该参数的 OpenAI-compatible 模型。显式预算型
`thinking.type = "enabled"` 仍然拒绝，因为它承诺具体 thinking 语义和预算，翻译路径
无法等价实现。

用户同时提供的 OpenCode 截图显示
`配置结构不合法: invalid type: null, expected a map`。该问题已在
`2026-08-10-桌面端缺省策略组配置回归修复设计.md` 和当前 `develop` 修复：缺省
`profiles` 不再被可变 JSON 下标插入为 `null`。本轮不重复改写该修复，只重新执行完整
加载链与真实 App 回归，确保最终安装包同时包含它。

真实复现条件为：用户退出 App，重新打开后点击“启动代理”，随后 Agent
页与全局路由同时显示 map/null 错误并无法配置。这与旧版重启准备草稿时把缺省
可选映射写成 `null` 的链路完全一致。

本次不让通用 Anthropic SDK 或 Claude Code 自动降级 adaptive，不声称兼容型上游提供
原生 Anthropic thinking block/signature，也不修改用户的 Claude Desktop 或 OpenCode
配置文件结构。

## 安全与数据红线

1. 不记录、输出或写入请求正文、响应正文、Provider Key 或本地虚拟 Key。
2. adaptive 兼容仅按已识别的 `claude-desktop` Agent 身份启用；公共
   `/v1/messages` 和 Claude Code 继续按原能力边界处理。
3. `enabled`、未知 thinking 类型和 malformed thinking 继续失败关闭。
4. 原生 Anthropic 路由继续逐字节透传，不进入本次降级逻辑。
5. OpenCode 配置回归复验不得自动修改用户磁盘配置；缺省映射保持缺省，真实非法配置
   继续进入只读保护。

## 用户可见行为、状态变化和失败处理

Claude Desktop 连接兼容型上游后，带 adaptive thinking 的文本或工具请求不再收到本地
400，而是继续完成路由和上游调用。上游若支持 `reasoning_effort`，现有参数映射继续
生效；上游未声明该参数时，Provider Adapter 继续按现有能力门禁省略它。

如果 Claude Desktop 发送显式预算型 `enabled`、未知类型或 malformed thinking，用户仍
收到 Anthropic 形状的 400，且 Receipt 显示没有真实上游尝试。原生 Anthropic 上游不受
影响。

OpenCode 页面加载省略 `profiles` / `agent_routes` 的合法配置时不再显示 map/null 结构
错误；真实损坏配置仍显示现有只读保护提示。

## 响应式、键盘操作和可访问性

本次不增加或修改界面控件、布局、焦点顺序、键盘行为和辅助技术语义。错误消失后沿用
现有 Agent 页面交互。

## 公开测试边界与验收标准

1. `agent-anthropic` fixture 证明 `agent_tool = claude-desktop` 且
   `thinking.type = adaptive` 可以归一化，并保留 opaque thinking 配置。
2. 真实 Gateway scoped path 测试证明 Claude Desktop adaptive 请求到达
   OpenAI-compatible 假上游并返回 200。
3. 公共 `/v1/messages` 的 `adaptive` 和显式 `enabled` 仍在上游前返回
   400。
4. 原生 Anthropic passthrough 测试继续证明 thinking 原样透传。
5. OpenCode/桌面配置测试继续证明省略可选映射后可物化为 `ClientConfig`，且磁盘源文件
   未被改写。
6. 运行插件测试、CLI proxy 测试、Desktop Rust/前端测试、workspace 全量测试与构建。
7. 执行 `scripts/install-local-desktop.sh`，校验 bundle id、签名、启动和监听进程。
8. 在真实 App 中确认 OpenCode 页面无 map/null 错误；用 Claude Desktop 专用路径发送
   adaptive 请求，并以 200、上游命中和 Receipt 为最终信号。

## 实现落点、遗留项和发布要求

- `apps/cli/src/gateway.rs`：让 Claude Desktop scoped route 进入插件时携带明确
  `agent_tool` 身份。
- `plugins/official/agent-anthropic/src/lib.rs`：按 Agent 身份验证 thinking；仅允许 Claude
  Desktop adaptive 降级。
- `plugins/official/agent-anthropic/fixtures/` 与 `apps/cli/tests/proxy.rs`：增加公开行为回归。
- OpenCode 代码保持现状，复用并重跑 2026-08-10 的缺省映射回归。

改动影响可执行行为，交付前必须安装本地桌面 App 并做真实链路检查。本次不创建 DMG，
不推送远端。完成后回写本文的实现状态、自动化结果、真实 App 结果和遗留项。

## 实现与验收结果

- Gateway 现在只在 `/agents/claude-desktop/v1/messages` 路径为 Anthropic
  入站标记 `agent_tool = claude-desktop`。`agent-anthropic` 只对该标记允许
  `thinking.type = adaptive`；未标记的公共路径和 `enabled` 依然失败关闭。
- 新增的 Gateway 公开行为测试先在旧实现上复现 400，实现后通过。插件
  25 项检查全部通过，workspace 全量 Rust 测试、Clippy 和格式检查通过。
- Desktop Rust 测试 253 通过、1 忽略；更新器 10 项、安装器 2 项、YAML 回归
  3 项全部通过。前端 30 个测试文件共 285 项通过，生产构建通过。
- `scripts/install-local-desktop.sh` 完成本地 aarch64 构建、内置插件门禁、
  产物审计、签名验证、精确安装与启动。安装 App 的 bundle id 为
  `com.tokenstation.desktop`，进程监听 `127.0.0.1:8787`。
- 真实 App 按“退出 → 重开 → 启动代理 → 全局路由 → OpenCode”复验。
  代理运行于 revision 132，全局三档路由和 OpenCode 路由模式均正常显示，
  未出现 map/null 错误，且未修改任何路由选择。
- 使用 Claude Desktop 已接入配置的 Bearer 认证，向本地专用路径发送
  `adaptive` 与 `output_config.effort = high` 的极小请求。真实上游返回 HTTP
  200，Anthropic 响应包含 `thinking` 和 `text` 内容块；Receipt 记录
  `agent_id = claude-desktop`、`protocol = anthropic-messages`、`attempts = 1` 和
  `status = 200`。密钥和请求/响应正文均未输出。

已知遗留项只有现有前端大 chunk 构建警告和 npm audit 依赖提示，与本次两条
故障无关。本次未创建 DMG，也未推送远端。

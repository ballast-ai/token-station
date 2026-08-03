# WorkBuddy Agent 接入与 OpenAI 协议兼容设计

## 问题与判断

WorkBuddy 5.3.8 已公开提供 OpenAI 兼容自定义模型入口，但 Token Station 的 Agent
Registry 还没有 WorkBuddy，`agent-openai` 的能力清单也不会创建
`/agents/workbuddy/v1/chat/completions` 命名空间。用户现在只能把 WorkBuddy 接到主页的
通用 `/v1/chat/completions`，无法为它设置独立三档路由，也无法从 Agent 页面备份、接入和恢复
配置。

本次目标是在 Agent 区直接加入 WorkBuddy，并让 Token Station 只管理
`~/.workbuddy/models.json` 里的 `tokenstation-auto` 自定义模型。普通聊天、流式响应、
function tool、工具结果、图片输入和思考强度必须沿用现有 OpenAI Chat Completions 主链。

## 范围与非目标

范围包括：

1. Agent Registry、发现规则、兼容目录和桌面导航加入 WorkBuddy。
2. 新增 WorkBuddy 本地连接器，写入完整的
   `/agents/workbuddy/v1/chat/completions` 地址、虚拟 Key 和模型能力声明。
3. `agent-openai` 声明支持 WorkBuddy，并保留 `reasoning_effort`、
   `parallel_tool_calls` 与 `max_completion_tokens` 的可表达语义。
4. 增加公开行为测试、连接器往返测试和真实 WorkBuddy CLI 验收。

本次不接管腾讯官方账号、积分、内置 Auto 模型或 WorkBuddy MCP。也不修改 WorkBuddy
程序包，不拦截 `copilot.tencent.com` 流量，不承诺所有上游模型都支持图片或思考模式。

## 安全与数据红线

Token Station 不得覆盖或删除用户已有的 WorkBuddy 自定义模型。连接时只追加 ID 为
`tokenstation-auto` 的模型，并保留其他数组元素；若同 ID 已存在但并非当前受管记录，连接器
必须拒绝并要求用户先处理冲突。API Key 继续作为敏感路径进入加密快照和 ownership 校验，
不得出现在日志、diff 摘要或成功提示里。

正常断开恢复连接前快照。强制断开必须按模型 ID 过滤 Token Station 项，不能删除整个
`models` 或 `availableModels` 数组。任何解析、校验或写入失败都必须保留原文件。

## 用户可见交互与失败处理

Agent 侧栏新增 WorkBuddy。发现安装后，页面显示版本、配置位置和一键接入按钮。接入成功后，
WorkBuddy 模型列表出现 “Token Station”，模型参数为 `tokenstation-auto`；它指向
`http://127.0.0.1:8787/agents/workbuddy/v1/chat/completions`，并声明工具调用、图片输入和
思考模式。

以下情况必须明确失败：网关没有加载 `agent-openai`、缺少虚拟 Key、`models.json` 不是
合法 JSON、`models` 或 `availableModels` 不是数组、已有同 ID 模型，或 ownership 记录与
当前文件不一致。失败时不得显示“已接入”。

## 响应式、键盘与可访问性

WorkBuddy 复用现有 AgentRoutePage，不新增独立布局。侧栏图标使用 `WB` 回退标记，保留
现有键盘焦点、按钮标签、窄屏布局和状态文字。名称和错误信息必须可由辅助技术读取，不能只靠
颜色表达接入状态。

## 测试边界与验收标准

公开测试至少覆盖：

1. Registry 能解析 WorkBuddy，连接器能力与 `agent-openai`、`origin_v1` 一致。
2. 连接补丁保留已有模型，只追加 Token Station 项；重复 ID、错误数组类型会失败。
3. 强制断开只删除 Token Station 项，保留接入后新增的其他模型。
4. `/agents/workbuddy/v1/chat/completions` 能完成文本和 function tool 往返。
5. `reasoning_effort`、`parallel_tool_calls` 和 `max_completion_tokens` 不在入站层静默丢失。
6. WorkBuddy 自带 CodeBuddy CLI 使用隔离配置完成一次真实工具调用；代理集成测试同时确认
   `/agents/workbuddy/v1/chat/completions` 会选择 WorkBuddy 命名空间并剥离成标准上游路径。

结构化输出仍沿用当前 fail-closed 边界；若 WorkBuddy 发送尚未批准的
`response_format=json_schema/json_object`，Token Station 返回 capability 错误，不伪装成
普通文本成功。

## 实现落点、发布与遗留项

实现落点包括 `agent-registry/builtin-agents.json`、`builtin-compatibility.json`、
`connectors/workbuddy.rs`、连接器动态补丁接口、`agent-openai` manifest/source/fixtures、
代理集成测试和桌面测试。代码验收后必须执行全量测试与构建，再运行
`scripts/install-local-desktop.sh` 更新 `/Applications/token-station.app`，最后在真实桌面 App
检查 WorkBuddy 页面。

已知遗留项是 WorkBuddy 自身可能在版本升级后改变 `models.json` 字段或自定义模型前缀。
连接器必须依靠版本发现和结构校验 fail closed，不能猜测写入。目前只对 macOS WorkBuddy
5.3.8、CodeBuddy CLI 2.115.0 完成真实验收，不对尚未实测的 Linux 或 Windows 安装路径做兼容
声明。

## 实现状态

- 设计：已完成。
- 公开行为测试：已完成。WorkBuddy 注册、连接器保护、命名空间路由和 OpenAI 参数保真均有
  回归测试。
- 代码实现：已完成。Agent 页面新增 WorkBuddy，连接器只追加或过滤
  `tokenstation-auto`。
- 全量测试与构建：已完成。Rust 工作区、桌面 Rust、前端 Vitest 和前端生产构建全部通过。
- 本地 App 更新与真实验收：已完成。`scripts/install-local-desktop.sh` 构建、审计、签名检查、
  替换和启动均成功；真实 App 显示 WorkBuddy 为“可接入”。真实 CodeBuddy CLI 使用隔离
  `models.json` 完成流式文本和 `Read` 工具两轮调用，第二轮请求包含工具结果，最终返回
  `WORKBUDDY_TOOL_E2E_OK`。验收没有创建或修改用户的 `~/.workbuddy/models.json`。

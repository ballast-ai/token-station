# Claude Desktop 网关模型发现兼容设计

日期：2026-08-04  
状态：第二轮已实现；用户确认连接恢复，本轮未独立重做 Claude Desktop Code 界面复验

## 1. 问题

Claude Desktop 1.24012.11 已通过 3P Gateway 配置连接到
`http://127.0.0.1:8787/agents/claude-desktop`，但 Setup 检查显示：

> Gateway returned no usable models. Add entries under Models to test inference without discovery.

网关和鉴权均可达。失败发生在模型发现：Claude Desktop 请求
`/agents/claude-desktop/v1/models?limit=1000`，随后只接受 Claude 系列模型名，或带有
`anthropic_family_tier` 的网关虚拟模型。Token Station 当前对所有调用方返回同一份真实上游目录，
其中的 DeepSeek、GLM 和 Ollama 模型均被 Claude Desktop 过滤，最终得到空目录。

## 2. 目标、范围与非目标

目标是让已接入 Token Station 的 Claude Desktop 能发现并实际选择一个可用模型，再用现有 Agent 路由
完成推理。本次修改 Claude Desktop 命名空间下的模型发现响应、Connector 写入的 Cowork egress 主机
列表和对应测试。

不修改全局 `/v1/models`，不影响 OpenCode、Codex、WorkBuddy 等其他 Agent，也不把真实上游模型
伪装成 Claude Sonnet、Opus 或 Haiku。此次不修改 Claude Desktop 的配置文件结构、凭据或应用选择。

## 3. 安全与数据红线

1. `/agents/claude-desktop/v1/models` 继续要求本地虚拟 Key，不能绕过鉴权。
2. 响应不得包含上游密钥、虚拟 Key、配置路径或账户信息。
3. 未知 Agent 命名空间继续返回 404，不能借模型发现扩大可访问范围。
4. 全局和其他 Agent 的模型目录必须保持现有语义。
5. Claude Desktop 发出的 `model: auto` 只作为路由输入，不能自动启用精确模型绑定。

## 4. 用户可见行为和失败处理

Claude Desktop 请求其专用模型目录时，Token Station 返回单个虚拟模型：

```json
{
  "object": "list",
  "data": [
    {
      "id": "claude-sonnet-4-6",
      "object": "model",
      "owned_by": "token-station",
      "display_name": "Token Station Auto",
      "anthropic_family_tier": "sonnet",
      "is_family_default": true
    }
  ]
}
```

`claude-sonnet-4-6` 是 Claude Desktop Code 模型选择器能够识别的兼容别名，显示名仍为
`Token Station Auto`。它不表示实际选择了 Anthropic Sonnet；Token Station 默认不启用精确模型绑定，
真实上游仍由当前 Agent 路由和能力条件决定。若网关不可达、虚拟 Key 错误或路由无候选，继续使用
现有 401、404 或结构化推理错误，不把这些故障改写成“没有模型”。

Connector 写入 `coworkEgressAllowedHosts` 时只保留 Claude Desktop 接受的 `127.0.0.1` 和
`localhost`。此前写入的裸 IPv6 地址 `::1` 会被 Claude 判为无效 hostname，并在首页持续显示
“Invalid network egress settings”；本轮移除该项，不增加任何外网白名单。

## 5. 响应式、键盘和可访问性

本次不新增界面控件，不改变布局、焦点顺序或键盘操作。Claude Desktop Setup 中现有的模型选择与
测试入口会读取新的目录；Token Station 界面无需新增无障碍状态。

## 6. 公开测试边界与验收标准

1. 带正确虚拟 Key 请求 `/agents/claude-desktop/v1/models?limit=1000`，返回 200 和上述兼容别名。
2. Claude Desktop 目录不泄漏真实上游模型名。
3. 全局 `/v1/models` 与 `/agents/opencode/v1/models` 仍返回配置中的真实模型。
4. 错误虚拟 Key 仍返回 401；未知 Agent 命名空间仍返回 404。
5. 使用 `model: auto` 向 `/agents/claude-desktop/v1/messages` 发起请求时，现有 Agent 路由可以选择并
   调用真实上游。
6. Rust 定向测试、CLI 全量测试和静态检查通过。
7. 执行 `scripts/install-local-desktop.sh` 更新本机 App，并在真实 Claude Desktop Setup 中重新检查；
   不再出现 “Gateway returned no usable models”。
8. Claude Desktop Code 模式模型选择器不再停在 “Models are still loading”，可以真正发送请求。
9. Connector 新生成的 `coworkEgressAllowedHosts` 不含 Claude 判为无效的 `::1`，且仍只允许回环主机。

## 7. 实现落点、遗留项与发布要求

- `apps/cli/src/gateway.rs` 保存 Claude Desktop 专用模型目录，并按 Agent ID 返回目录。
- `apps/cli/src/server.rs` 将解析后的 Agent ID 传给模型目录响应。
- `apps/cli/tests/proxy.rs` 覆盖专用目录、其他目录不变、鉴权边界和 `auto` 推理。
- `人肉测试重大体验问题.md` 记录本次真实 App 问题与验收结果。

实现完成后必须运行本地桌面安装脚本。若 Claude Desktop 仍拒绝目录，保留新 Token Station App，
记录 Claude 版本、响应形状和剩余错误，不通过修改用户凭据或伪造真实模型名绕过。

## 8. 当前验收状态

第一轮已完成：

1. 新增回归先观察到 Claude Desktop 专用目录错误返回两个真实上游模型，随后实现专用 `auto` 目录，
   回归转绿。
2. `cargo test -p token-station-cli` 通过：CLI 单元、集成和文档测试均无失败；Proxy 测试为
   `66 passed, 1 ignored`。
3. `cargo clippy --workspace --all-targets -- -D warnings` 通过。
4. 三个改动 Rust 文件的 `rustfmt --check` 通过；整个工作区的 `cargo fmt --all -- --check` 仍会报告
   `crates/router-core` 中本次未修改的既有格式差异，本轮没有机械改写这些无关文件。
5. `scripts/install-local-desktop.sh` 成功：桌面构建、内置插件检查、产物审计、签名校验、精确替换和
   启动全部通过。实际运行 revision 为 `110`，监听 `127.0.0.1:8787`。
6. 使用 Claude Desktop 当前配置中的本地虚拟 Key 请求真实安装包，专用目录返回一个
   `Token Station Auto`，没有泄漏真实上游模型。
7. Claude Desktop 1.24012.11 的 `Test model discovery` 显示
   `Model discovery — found 1 model auto`，原始 “Gateway returned no usable models” 已消失。

推理连接检查继续到 `/agents/claude-desktop/v1/messages` 后返回 401，原因是当前路由选择的 `wec`
上游没有在 Token Station 本地密钥库保存 `provider_api_key`。密钥库文件的修改时间为
2026-07-31 18:00:42，且只含 `deepseek/provider_api_key`，因此这不是本次安装造成的凭据丢失，也不属于
模型发现兼容缺陷。要完成真实推理，用户需要为 `wec` 重新输入密钥，或明确把 Claude Desktop 路由
切换到已有凭据的供应商；本轮不擅自复制凭据或改变供应商。

第二轮真实 App 验收发现第一轮仍不完整：Claude 主进程日志显示
`Model discovery: 1 found ... picker = 0 (empty)`，Code 模式点击发送提示 `Models are still loading`。
同时 Connector 写入的 `::1` 被 Claude 首页判为无效 egress hostname。第二轮将模型 ID 改为 Claude
选择器识别的兼容别名，并移除无效的 `::1`。相关 Connector 和事务回归已通过，
最新桌面 App 也已重新安装。用户随后确认连接恢复；本轮的独立真实请求验证覆盖了
Claude Code 的 Anthropic 入站和上游推理，没有再操作 Claude Desktop Code 模型选择界面，因此不把
这一项写成独立实测结论。

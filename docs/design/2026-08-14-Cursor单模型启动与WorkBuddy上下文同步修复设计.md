# Cursor 单模型启动与 WorkBuddy 上下文同步修复设计

日期：2026-08-14
状态：实现完成，Cursor 与 WorkBuddy 真实验收通过

## 1. 问题

Cursor 隧道版本已经能够写入 `Token Station Auto`，但接入后仍保留 Cursor 自带模型，
用户可能继续选择这些模型并误以为它们都由 Token Station 控制。接入成功后还需要用户
手动启动 Cursor，未达到一键接入的完整行为。

WorkBuddy 的 `tokenstation-auto` 已经能把请求送入 Token Station。当前已安装 App 写出的
模型名称仍是旧值，路由变化后的上下文限制也缺少端到端验收。WorkBuddy 界面的分母来自
`maxInputTokens`，分子来自 OpenAI 流式响应中的 `usage.prompt_tokens`。Token Station 当前
先输出用量块，再输出 `finish_reason`；WorkBuddy 会被后一个无用量块清空暂存值，因此真实
对话后仍显示 `0 / 257.6K`。

## 2. 目标与范围

- Cursor 接入后只显示并选中 `Token Station Auto`。Cursor 原有模型在接入期间隐藏，断开
  后通过完整备份精确恢复。
- Cursor 配置和隧道事务成功后自动启动 Cursor。Cursor 已经运行时继续拒绝写入，不能
  强制关闭用户进程。
- 本轮不使用调试端口或 CDP 注入。运行中的 Cursor 不执行配置热改写。
- WorkBuddy 继续使用 `tokenstation-auto`，名称显示为 `Token Station Auto`。
- Token Station 路由配置成功切换后，立即按当前有效路由重新计算安全上下文和最大输出，
  再重写受管的 WorkBuddy 模型记录。WorkBuddy 自带文件监听负责热加载，无需重启。
- 多模型路由使用所有候选模型都能承载的最小输入上限和最小输出上限。任何候选模型缺少
  已验证限制时不写入猜测值。
- OpenAI 兼容流按 `finish_reason`、最终 `usage`、`[DONE]` 的顺序结束，使 WorkBuddy 能把
  原始用量写入会话统计。

## 3. 安全与数据边界

- Cursor 原始 `applicationUser` 和 Safe Storage 密钥继续完整备份。隐藏模型只能修改备份
  覆盖范围内的配置。
- Cursor 自动启动只能发生在隧道、Safe Storage、SQLite 和活动记录全部成功后。启动失败
  时返回明确错误，并恢复 Cursor 配置和隧道状态。
- 不读取或输出 Cursor 密钥、Token Station 虚拟 Key和供应商凭据。
- WorkBuddy 刷新只替换 Token Station 管理的 `tokenstation-auto` 记录。用户自建模型保持
  原样。
- WorkBuddy 元数据刷新失败时，路由切换返回错误，不能把旧限制继续显示成已经同步。

## 4. 用户可见行为

### 4.1 Cursor

1. 用户完全退出 Cursor 后点击“一键接入并启动”。
2. Token Station 建立受控 HTTPS 隧道，备份原配置，再写入单一
   `Token Station Auto` 模型。
3. Token Station 启动 Cursor。Cursor 模型选择器只显示并选中
   `Token Station Auto`。
4. 用户断开接入后，原模型列表和原选择得到恢复。

### 4.2 WorkBuddy

1. 接入时写入当前有效路由的安全输入上限和最大输出。
2. 用户在 Token Station 中切换 WorkBuddy 路由后，同一条受管模型记录立即更新。
3. WorkBuddy 运行中通过自身文件监听载入新限制，不要求重启。
4. 完成真实对话后，上下文面板的已用量必须大于零；分母继续使用当前路由写入的
   `maxInputTokens`。

## 5. 实现位置

- `apps/desktop/src-tauri/src/cursor_tunnel.rs`：隐藏 Cursor 原模型，选中唯一的
  `Token Station Auto`，并在事务成功后启动 Cursor。
- `apps/desktop/src/pages/AgentRoutePage.tsx`：把 Cursor 主按钮文案改为“一键接入并启动”。
- `apps/desktop/src/pages/AgentRoutePage.test.tsx`：锁定按钮文案和成功行为。
- `apps/desktop/src-tauri/src/agent_integration/connectors/workbuddy.rs`：统一模型显示名并验证
  热更新保留用户模型。
- `apps/desktop/src-tauri/src/agent_integration/commands.rs` 与 `src/lib.rs`：保证路由切换完成后
  刷新 WorkBuddy 元数据，并把刷新失败返回给调用方。
- `plugins/official/provider-openai-compatible/src/lib.rs`：在同一个上游用量块中先释放延迟的
  `Done`，再输出 `Usage`，保证下游 OpenAI SSE 的最终用量位于结束块之后。

## 6. 公开测试边界

1. Cursor 接入写入后，启用列表只包含 `tokenstation/auto`，原模型进入隐藏列表，所有现有
   模式都选中 `tokenstation/auto`。
2. Cursor 断开后，完整 `applicationUser` 与原始密钥精确恢复。
3. Cursor 自动启动只在事务成功后发生。启动失败会清理隧道并恢复配置。
4. WorkBuddy 接入和路由刷新都写入 `maxInputTokens` 与 `maxOutputTokens`。
5. WorkBuddy 刷新保留用户自建模型，只替换 `tokenstation-auto`。
6. WorkBuddy 多模型路由采用候选模型中最小的已验证限制。
7. OpenAI 兼容流的结束顺序固定为 `Done`、`Usage`，渲染后的 SSE 顺序固定为
   `finish_reason`、`usage`、`[DONE]`。
8. 目标测试通过后先安装实际 App，并在 WorkBuddy 中确认真实对话的已用量大于零。全量
   测试留到真实行为通过后执行。

## 7. 真实 App 验收

1. 执行 `scripts/install-local-desktop.sh` 安装当前工作树构建的 App。
2. 完全退出 Cursor 后点击“一键接入并启动”，确认 Cursor 自动打开。
3. 确认 Cursor 只显示 `Token Station Auto`，并完成一次真实 Agent 请求。
4. 确认 Token Station 请求库新增 `agent_id=cursor` 的成功回执。
5. 切换 WorkBuddy 路由，确认 `~/.workbuddy/models.json` 的输入和输出限制立即变化。
6. 在不重启 WorkBuddy 的情况下发起请求，确认请求继续经过 Token Station。
7. 打开 WorkBuddy 上下文面板，确认已用量不再为零。
8. 断开 Cursor，确认原配置恢复，Cursor 专用隧道停止。

## 8. 非目标与遗留项

- 本轮不接入 OpenCodex Cursor Bridge，也不使用 Cursor 调试端口注入运行时配置。
- Cursor 运行中热切换留给后续实验路线，必须单独设计和验证。
- Quick Tunnel 的稳定性限制继续沿用现有 Cursor 隧道设计。

## 9. 实现与验收记录

已完成以下实现：

- Cursor 接入态只启用 `tokenstation/auto`，原模型写入隐藏列表，现有模式统一选中
  `Token Station Auto`。完整 `applicationUser` 备份继续用于断开恢复。
- Cursor 配置和接管记录写入成功后使用 macOS `open -a Cursor` 启动应用。启动失败会恢复
  数据库、删除接管记录并停止临时隧道。
- Cursor 页面按钮已经改为“一键接入并启动”。
- WorkBuddy 受管模型已经统一显示为 `Token Station Auto`。路由刷新继续使用候选模型的
  最小安全输入和输出限制，并通过 WorkBuddy 自身文件监听热加载。
- WorkBuddy 流式响应已经改为先发送 `finish_reason`，再发送最终 `usage`，最后发送
  `[DONE]`。WorkBuddy 不再被结束块清空暂存用量。
- 发现扫描并发测试已经隔离真实 `/Applications` 中的双 WorkBuddy 安装，生产扫描逻辑
  未改变。

已完成以下验证：

- Cursor 数据库和 WorkBuddy 顶层数组定向测试通过。
- AgentRoutePage 16 个定向测试通过。
- 前端全量测试 376 项通过。
- Rust 库测试 356 项通过，2 项显式压力测试保持忽略；桌面更新、安装脚本和 YAML 集成
  测试 21 项通过。
- TypeScript 和 Vite production build 通过。
- `scripts/install-local-desktop.sh` 已构建、审计、安装并启动
  `/Applications/token-station.app`。bundle id 为 `com.tokenstation.desktop`。
- OpenAI 兼容 Provider 的结束顺序测试、Agent OpenAI 的流式渲染测试和 WorkBuddy
  连接器定向测试已经通过。
- `scripts/install-local-desktop.sh` 已再次构建、审计、安装并启动最终版本。真实代理请求
  按 `finish_reason`、`usage`、`[DONE]` 的顺序结束，最终用量为非零。
- WorkBuddy 5.3.8 已在不重启的情况下使用 `Token Station Auto` 完成真实对话。上下文面板
  已显示 `29.9K / 257.6K`，用量分子恢复正常。磁盘配置保持
  `maxInputTokens=257550`、`maxOutputTokens=32768`。
- 本轮按实际行为优先原则只运行了上述定向测试和安装脚本自带门禁，没有重新运行额外的
  全量测试。

- Quick Tunnel 地址解析已经拒绝 `api.trycloudflare.com`。失效状态现在可以直接重新接入，
  同时保留恢复官方配置入口。Cursor 退出后已通过安装版本重建隧道并自动启动；用户确认
  Cursor 已恢复正常。

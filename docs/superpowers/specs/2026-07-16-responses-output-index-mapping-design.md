# Responses 流式 output_index 映射设计

## 1. 背景与问题

`agent-openai-responses` 将 Canonical `StreamEvent` 渲染为 OpenAI Responses
SSE。当前实现直接把两类事件携带的 `index` 写入 Responses `output_index`：

- `StreamEvent::Delta.index` 是 Chat Completions choice index；
- `StreamEvent::ToolCallDelta.index` 是单个 choice 内的 tool-call index；
- Responses `output_index` 是最终 `response.output` 数组中的全局位置。

文本 choice 0 与首个工具调用 0 同时出现时，当前输出会为 message 和
function-call item 都生成 `output_index = 0`。现有混合流 fixture 也固化了这个错误
结果，因此一致性测试没有识别协议语义冲突。

## 2. 目标与完成条件

本次修复只处理 Responses 流式输出项的全局编号和最终排序。完成条件：

1. 每个 message/function-call output item 获得唯一、连续的 `output_index`。
2. 编号在 output item 首次出现时确定，后续 delta 和 done 事件稳定复用。
3. `response.completed.response.output` 与事件中的 `output_index` 使用同一顺序。
4. 继续逐事件输出，不为了等待最终项目数量而缓冲整条流。
5. `Done` / `Error` 后保持现有按 `stream_id` 清理状态的行为。
6. 不修改 Canonical IR、provider、router 或 release 部件。

## 3. 已确认方案

### 3.1 首次出现顺序分配

每个 `StreamState` 增加 `next_output_index`。文本或工具 output item 第一次出现时：

1. 读取当前 `next_output_index` 作为该 item 的全局编号；
2. 递增计数器；
3. 将编号保存在该 item 的流状态中；
4. `added`、delta、done 事件全部读取保存的编号，不再使用原始 IR index。

原始 IR index 仍作为各自命名空间内的稳定查找键：

```text
text[choice_index]       -> TextStream { output_index, content }
tools[tool_call_index]   -> ToolStream { output_index, identity, arguments }
```

例如事件依次为文本 choice 0、工具调用 0、工具调用 1，则分配结果为：

```text
message(choice=0)       -> output_index 0
function_call(tool=0)   -> output_index 1
function_call(tool=1)   -> output_index 2
```

如果工具先于文本出现，则工具获得 0、文本获得 1。首次出现顺序来自 provider 已解析
出的 Canonical 事件顺序，同一输入下具有确定性。

### 3.2 结束阶段排序

`stream_done` 不再先遍历全部文本、再遍历全部工具。它将两类完成 item 组成
`(output_index, item)` 集合，按 `output_index` 排序后依次生成：

- `response.output_item.done`；
- 最终 `response.completed.response.output` 数组。

这样完成事件、最终数组和此前增量事件保持同一全局顺序。

### 3.3 错误处理

`output_index` 使用 `u32`，计数递增采用受检加法。理论上的编号耗尽返回内部错误，
不发生整数回绕或重复分配。已有身份漂移检测继续生效：同一 tool-call index 的
`id` 或 `name` 在后续分片中改变时仍明确失败。

## 4. 未采用方案

### 4.1 固定偏移或奇偶编号

把文本映射为偶数、工具映射为奇数虽可避免碰撞，但可能产生不连续编号，并与最终
`output` 数组的真实位置不一致，因此不采用。

### 4.2 在 Done 时统一编号

结束时已经知道所有 item 数量，但此前增量事件已经发送，无法回写已发出的
`output_index`。若为此缓冲整条流，会违反流式增量约束，因此不采用。

### 4.3 扩展 Canonical IR

为 `StreamEvent` 增加全局 output item 身份可以从契约层解决，但会修改
`crates/protocol` 和 ABI，超出本任务红线。当前问题可在 Responses 入站适配器内
无损解决，不发起 IR 变更。

## 5. 影响面

| 维度 | 文件或符号 | 动作 | 验证 |
|---|---|---|---|
| 实现 | `plugins/official/agent-openai-responses/src/lib.rs` 的 `StreamState`、文本/工具流状态、`render_stream_event`、`stream_done` | 分配并复用全局编号，按编号完成输出 | 独立 WASM Clippy、插件测试 |
| Fixture | `plugins/official/agent-openai-responses/fixtures/agent.stream.codex.expected.json` | 将混合文本/工具的工具 item 改为 `output_index = 1` | conformance 先红后绿 |
| WASM 回归 | `crates/plugin-runtime/tests/official_plugins.rs` | 验证文本先出现、工具先出现、多个工具、续传和最终排序 | 真实组件加载测试 |
| Gateway 回归 | `apps/cli/tests/proxy.rs` | 验证混合流经完整 provider/agent 管线后索引唯一且最终数组顺序一致 | CLI proxy 测试 |
| 验收 | `docs/verification/2026-07-16-M4-多-Agent-入站验收.md` | 移除该阻塞并记录新证据；其他边界继续保留 | 文档与实际命令逐项核对 |

明确不修改：

- `crates/protocol`；
- `crates/router-core`；
- `crates/release`；
- `plugins/official/provider-*`；
- 路由策略、用户 8787 服务和全局 Agent 配置。

## 6. 测试设计

实施采用红—绿顺序：先把现有错误 fixture 和新增断言改成正确协议期望，确认它们在
旧实现上失败，再修改实现。

必须覆盖：

1. 文本 choice 0 后出现工具 0：编号分别为 0、1。
2. 工具 0 后出现文本 choice 0：编号分别为 0、1。
3. 两个工具调用：各自编号唯一，参数续传保持原编号。
4. 文本在工具出现后继续输出：仍使用首次分配的文本编号。
5. 所有 `response.output_item.added`、delta、done 事件编号一致。
6. `response.completed.response.output[index]` 与对应 item 一致。
7. 两个 `stream_id` 交错时各自从 0 开始且互不串状态。
8. `Done` 和 `Error` 后状态清理不回归。

最终门禁：

```text
cargo fmt --all -- --check
cargo fmt --manifest-path plugins/official/agent-openai-responses/Cargo.toml -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --manifest-path plugins/official/agent-openai-responses/Cargo.toml \
  --target wasm32-wasip2 -- -D warnings
cargo test --workspace
cargo build --release -p token-station-cli
token-station-cli plugin test plugins/official/agent-openai-responses
git diff --check
```

真实 Codex 验收继续使用独立临时端口、配置、数据目录和 `CODEX_HOME`，不占用或
重启用户现有 8787 服务。至少验证普通文本、真实本地 function tool、混合文本与
工具事件的客户端消费，以及请求日志不包含提示词或凭证。

## 7. 交付判定

该阻塞只有在正确 fixture 完成红—绿证明、真实 WASM/CLI 测试通过、真实 Codex
链路通过、红线 diff 为零后，才能从 M4 验收报告中移除。即使本项通过，也只按需求
文档逐条重新审计 M4，不自动把“本地验证完成”升级为已 push、PR-ready、已批准或
已合并。

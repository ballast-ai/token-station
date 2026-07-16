# Responses output_index 映射实施计划

关联设计：
[2026-07-16-responses-output-index-mapping-design.md](../specs/2026-07-16-responses-output-index-mapping-design.md)

## 目标

在不修改 Canonical IR、provider、router-core 或 release 的前提下，让
`agent-openai-responses` 为流式 message/function-call item 分配唯一、连续、稳定的
Responses `output_index`，并保证最终 `response.output` 顺序一致。

## 实施步骤

### 1. 建立失败证据

- 修改混合文本/工具 fixture 的正确期望：message 为 0，function call 为 1。
- 在真实 WASM runtime 测试中新增文本先出现、工具先出现、多个工具和续传断言。
- 在 CLI proxy 测试中断言完整 provider → agent 管线的 added/delta/done/final output
  索引唯一且一致。
- 运行目标测试，保存旧实现失败证据。

### 2. 实现适配器局部编号

- 新增 `TextStream { output_index, content }`。
- 为 `ToolStream` 增加 `output_index`。
- 为 `StreamState` 增加 `next_output_index` 和受检分配函数。
- 文本/工具首次出现时分配编号，后续事件稳定复用。
- `stream_done` 合并两类 item，按全局编号排序后输出 done 事件和最终数组。

### 3. 目标回归

- 运行 Responses fixture/conformance。
- 运行真实 WASM official plugin 测试。
- 运行 CLI proxy 测试。
- 运行独立插件 16 项协议测试。

### 4. 全量与真实链路验收

- 运行 fmt、workspace/wasm clippy、workspace test、rustdoc 和 release build。
- 使用独立端口、数据目录和 `CODEX_HOME` 复测 Codex 普通文本与真实本地 function
  tool；不停止或修改用户 8787 服务。
- 扫描临时日志和 metrics，确认无提示词/凭证落盘，临时端口全部清理。
- 审计红线目录和提交范围。

### 5. 文档与提交

- 更新 M4 验收报告中的测试计数、真实 E2E 证据和剩余边界。
- 仅暂存本轮文件，不包含既有 `agent-anthropic` 修改。
- 本地提交；不 push、不创建 PR，除非用户另行授权。

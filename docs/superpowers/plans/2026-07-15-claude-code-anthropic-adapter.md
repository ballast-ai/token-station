# Claude Code Anthropic 入站适配实施计划

## 目标与边界

按 [设计规格](../specs/2026-07-15-claude-code-anthropic-adapter-design.md)
完成需求文档 M0–M3：首批 Agent 协议盘点、`agent-anthropic` 五族一致性、
`/v1/messages` 宿主集成、Claude Code 假上游和真实 DeepSeek E2E。

不修改 `crates/router-core`、`crates/protocol`、`crates/release`、现有 provider
adapter 语义或路由策略。DeepSeek 只出现在样例配置与 E2E 验收中。

## Task 1：M0 协议盘点与契约缺口

### 交付物

- 新增 `docs/contributing/入站适配器-协议盘点.md`。
- 盘点 Claude Code、Codex、opencode 和需确认名称的 `openclow`。
- 每项记录当前协议、请求入口、Base URL/认证配置、可复用 adapter、
  已知限制、证据日期与来源。
- 新增 `docs/contributing/入站适配器-IR变更请求-thinking.md`，只记录
  Anthropic thinking/redacted_thinking 缺口，不修改 IR。

### 验证与提交

```bash
rg -n "Claude Code|Codex|opencode|openclow|thinking" docs/contributing/入站适配器-*.md
git diff --check
```

```text
docs: 盘点首批 Agent 入站协议
```

## Task 2：`agent-anthropic` 非流式骨架

### 交付物

- 复制 `plugins/official/agent-openai` 包结构为
  `plugins/official/agent-anthropic`，重写身份与协议逻辑。
- manifest 声明 `anthropic-messages`、Claude Code、完整 conformance，且
  network/filesystem 为 `false`、secrets 为空。
- 实现 metadata、healthcheck、supported protocols、match inbound、
  normalize inbound、render response 和 error mapping。
- 支持 system、text blocks、tools、tool_use、tool_result、sampling、usage 和
  stop reason。
- 对无法表达的关键 block 返回明确 capability/invalid request，不静默丢失。
- 添加 normalize/render/error fixtures，并提供最小正确的 text stream 与 empty hint
  fixtures，使该提交的五族 coverage 保持绿色；Task 3 再扩展工具增量和
  StepType 场景。

### 验证与提交

```bash
./target/release/token-station-cli plugin build plugins/official/agent-anthropic
./target/release/token-station-cli plugin test plugins/official/agent-anthropic
```

该提交不允许一致性 coverage 失败。

```text
feat: 实现 Anthropic 非流式入站适配
```

## Task 3：流式、工具增量与 Hint

### 交付物

- 用 `context.stream_id` 作每流隔离键，维护 Anthropic message/content block
  序列；Done/Error 后清理。
- `Delta` -> `text_delta`。
- `ToolCallDelta` -> `content_block_start(tool_use)` + `input_json_delta`。
- `Usage` / `Done` -> `message_delta` / `message_stop`。
- `Error` -> Anthropic SSE error。
- Hint 仅输出契约内 `StepType`，不猜测 prompt 语义。
- 补齐 stream/hint fixtures。

### 验证与提交

```bash
./target/release/token-station-cli plugin build plugins/official/agent-anthropic
./target/release/token-station-cli plugin test plugins/official/agent-anthropic
cargo test -p token-station-conformance
cargo test -p token-station-plugin-runtime
```

```text
feat: 完成 Anthropic 流式与工具适配
```

## Task 4：宿主 `/v1/messages` 集成

### 交付物

- `crates/plugin-runtime/src/agent.rs`：增加 `match_inbound` 透传，不改
  ABI/沙箱权限。
- `apps/cli/src/server.rs`：注册 `POST /v1/messages`，保留现有入口。
- `apps/cli/src/gateway.rs`：接收 method/path/非凭证头，normalize 前调用
  match，生成每请求 render context。
- `.github/CODEOWNERS`：增加 `agent-anthropic` 独立审计行。
- 添加 HTTP 正向、401、协议不匹配、JSON/SSE 回归测试。

### 验证与提交

```bash
cargo test -p token-station-cli
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

```text
feat: 接入 Anthropic Messages 网关入口
```

## Task 5：配置与 Claude Code 接入配方

### 交付物

- 新增不含凭证的 Claude Code + DeepSeek E2E 样例配置。
- 样例只使用现有 `upstreams` / `router.pools` /
  `provider-openai-compatible`，Rust 逻辑不包含 DeepSeek 名称。
- 新增 Claude Code 接入文档：Base URL、本地 virtual key、模型变量、
  thinking/beta 边界、启动与验证命令。

### 验证与提交

```bash
cargo run -p token-station-cli -- --config apps/cli/claude-code-deepseek-config.json upstream list
cargo run -p token-station-cli -- --config apps/cli/claude-code-deepseek-config.json rule list
rg -n "DeepSeek|deepseek" apps crates plugins/official/agent-anthropic
```

第三条命令在 Rust/adapter 逻辑中应无厂商专属分支；配置样例本身会命中。

```text
docs: 新增 Claude Code 与 DeepSeek 接入配方
```

## Task 6：假上游 E2E

验证 Claude Code 真实发出 `/v1/messages`，并覆盖：

1. 非交互普通问答。
2. 多个 SSE 文本增量。
3. 工具调用、结果回传和最终回答。
4. 本地鉴权失败与上游错误。
5. 交错流隔离，无 stream state 泄漏。

假上游只作测试支撑，不进入产品运行配置。

## Task 7：真实 DeepSeek E2E

- 从本机环境或 Keychain 读取 `DEEPSEEK_API_KEY`，不回显值。
- Claude Code 仅获取 token-station 本地 virtual key。
- 验证普通问答、流式文本、一次真实工具调用和本地鉴权错误。
- 验证请求日志不含 prompt、DeepSeek Key 或 virtual key。

## Task 8：最终审计

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p token-station-cli
./target/release/token-station-cli plugin build plugins/official/agent-anthropic
./target/release/token-station-cli plugin test plugins/official/agent-anthropic
git diff origin/main...HEAD -- crates/router-core crates/protocol crates/release plugins/official/provider-openai-compatible
git status --short
git log --oneline origin/main..HEAD
```

验收报告分开声明一致性、假上游 E2E、真实 DeepSeek E2E 和已知
IR/上游能力边界，不以单条 smoke 泛化为全协议兼容。

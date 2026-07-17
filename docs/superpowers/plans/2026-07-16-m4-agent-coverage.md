# M4 多 Agent 入站覆盖实施计划

## 目标与边界

按
[设计规格](../specs/2026-07-16-m4-agent-coverage-design.md)
完成需求文档 M4：

- 新增 OpenAI Responses 入站 adapter，使 Codex 可接入。
- OpenCode 和 OpenClaw 复用现有 `agent-openai`。
- 三个 Agent 都完成普通对话、流式、工具闭环、本地鉴权错误和上游错误的
  真实 E2E。
- 每个 Agent 使用独立配置/端口/数据目录，不干扰用户当前模型厂商测试。

硬红线：

- 不修改 `crates/router-core`。
- 不修改 `crates/protocol`；Responses 语义缺口只写变更请求。
- 不修改 `crates/release`。
- 不修改 provider adapter 或路由策略。
- 不修改用户全局 Agent 配置、当前 `8787` 服务或现有测试数据。
- 不向 `main` 直接提交，不在未授权时 push。
- 不修改 `apps/cli/build.rs`、`scripts/build-release.sh` 或 release workflow 将新插件
  嵌入发布包；本轮以可安装的官方 plugin package 完成 M4，发布内置纳入负责人
  后续决策。

本计划中的“提交”均指当前功能分支的本地提交。

## Task 1：刷新协议盘点与 IR 变更请求

### 文件

- 修改 `docs/contributing/入站适配器-协议盘点.md`。
- 新增 `docs/contributing/入站适配器-IR变更请求-openai-responses.md`。

### 交付物

- 用执行日官方文档/仓库重新确认 Codex、OpenCode 和 OpenClaw 的版本、
  协议、Base URL 与临时配置入口。
- 将需求中 `openclow` 明确记录为已确认的官方项目 OpenClaw。
- 写明现有 IR 可支持的 Responses 子集：message、input/output text、image、
  function call/output、usage、error。
- 单独列出 reasoning summary/content、computer call、hosted web search 等缺口，
  说明不支持时必须明确拒绝，不静默丢失。
- 变更请求只提契约问题与候选表示，不改 `crates/protocol`。

### 验证与提交

```bash
rg -n "Codex|OpenCode|OpenClaw|Responses|reasoning|computer|web search" \
  docs/contributing/入站适配器-*.md
git diff --check
git diff -- crates/protocol crates/router-core crates/release
```

```text
docs: 刷新 M4 Agent 协议与 IR 缺口
```

## Task 2：Responses adapter 非流式骨架与失败用例

### 文件

- 新增 `plugins/official/agent-openai-responses/Cargo.toml`。
- 新增 `plugins/official/agent-openai-responses/Cargo.lock`。
- 新增 `plugins/official/agent-openai-responses/wit/adapter.wit`。
- 新增 `plugins/official/agent-openai-responses/manifest.json`。
- 新增 `plugins/official/agent-openai-responses/README.md`。
- 新增 `plugins/official/agent-openai-responses/src/lib.rs`。
- 新增 `plugins/official/agent-openai-responses/fixtures/agent.normalize.*.json`。
- 新增 `plugins/official/agent-openai-responses/fixtures/agent.render.*.json`。
- 新增 `plugins/official/agent-openai-responses/fixtures/agent.error.*.json`。
- 修改根 `Cargo.toml` 的 `exclude`。
- 修改 `.github/CODEOWNERS`。
- 修改 `crates/conformance/tests/official_plugins.rs`。

### TDD 顺序

1. 先将新 manifest 加入 `official_plugins.rs`，运行用例确认因文件/身份缺失而失败。
2. 创建独立 WASM guest package，声明 `openai-responses`、`codex`、五族 conformance
   和全禁用权限。
3. 先写 normalize/render/error fixtures，再实现：
   - `instructions` / 字符串 `input` / message items。
   - `input_text` / `input_image`。
   - function tool、`function_call`、`function_call_output`。
   - 采样参数、stream 和请求级 extensions。
   - 文本响应、function call 响应、usage 和结束原因。
   - Responses 非流式错误形状。
4. 对无法表达的关键 item 写负向 fixture，断言返回 Capability/InvalidRequest。
5. 为了保持 conformance coverage，先提供最小正确 hint/stream fixture；Task 3 再扩展
   工具分片与状态场景。

### 验证与提交

```bash
cargo test -p token-station-conformance --test official_plugins
cargo build --manifest-path plugins/official/agent-openai-responses/Cargo.toml \
  --target wasm32-wasip2
cargo clippy --manifest-path plugins/official/agent-openai-responses/Cargo.toml \
  --target wasm32-wasip2 -- -D warnings
cargo fmt --manifest-path plugins/official/agent-openai-responses/Cargo.toml -- --check
cargo build --release -p token-station-cli
./target/release/token-station-cli plugin build plugins/official/agent-openai-responses
./target/release/token-station-cli plugin test plugins/official/agent-openai-responses
```

```text
feat: 实现 Responses 非流式入站适配
```

## Task 3：Responses 流式、工具分片与 Hint

### 文件

- 修改 `plugins/official/agent-openai-responses/src/lib.rs`。
- 扩展 `plugins/official/agent-openai-responses/fixtures/agent.stream.*.json`。
- 扩展 `plugins/official/agent-openai-responses/fixtures/agent.hint.*.json`。
- 修改 `crates/plugin-runtime/tests/official_plugins.rs`。

### TDD 顺序

1. 先增加 `responses_agent_package()` 和真实 WASM conformance 用例，确认新包在未实现
   完整流式前失败。
2. 用 `context.stream_id` 建立每流独立状态，使用 response ID、model、内容块与
   function call 索引，不使用全局单流布尔值。
3. 实现 Responses 事件序列：
   - `response.created`。
   - output item/content part added。
   - `response.output_text.delta`。
   - function arguments delta。
   - content part/output item done。
   - usage 和 `response.completed`。
4. 工具 arguments 只按 `stream_id + call_id` 累积，同时持续输出 delta；完成时
   生成 Codex 可消费的完整 function call item。
5. `Done`、`Error`、客户端断开和上游 EOF 都清理该 `stream_id` 状态。
6. 增加两个 stream 交错的隔离/清理测试。
7. Hint 仅允许 `planning` / `edit` / `summarize`，不从 prompt 文本猜测。

### 验证与提交

```bash
./target/release/token-station-cli plugin build plugins/official/agent-openai-responses
./target/release/token-station-cli plugin test plugins/official/agent-openai-responses
cargo test -p token-station-plugin-runtime --test official_plugins
cargo clippy --manifest-path plugins/official/agent-openai-responses/Cargo.toml \
  --target wasm32-wasip2 -- -D warnings
```

```text
feat: 完成 Responses 流式与工具适配
```

## Task 4：Gateway `/v1/responses` 宿主接入

### 文件

- 修改 `apps/cli/src/server.rs`。
- 修改 `apps/cli/src/gateway.rs`。
- 修改 `apps/cli/tests/proxy.rs`。
- 只在实测证明现有 ABI 透传缺失时，才最小修改
  `crates/plugin-runtime/src/agent.rs`。

### TDD 顺序

1. 将 `agent-openai-responses` 加入 `apps/cli/tests/proxy.rs::plugins_dir()` 的临时官方包，
   先新增失败的 `/v1/responses` 路由用例。
2. 新增 `send_responses`/`post_responses` 测试 helper，使用 `127.0.0.1:0`、临时数据
   目录和假上游，不碰真实 `8787`。
3. `server.rs` 注册 `POST /v1/responses`，鉴权失败返回 Responses 形状 401。
4. `gateway.rs` 继续在 normalize 前调用 `match_inbound`，把
   `openai-responses` 的 `agent_tool` 记为 `codex`；凭证头仍只进入
   `HeaderDigest::redacting`。
5. 复用现有 render context 的 `protocol`、`stream_id`、`response_id`、`model`；
   不改 WIT 契约。
6. 增加非流式文本、流式文本、工具闭环、401、协议不匹配、上游错误、
   流中错误和断开清理测试。
7. 重跑 Anthropic Messages 与 Chat Completions 已有用例，证明无回归。

### 验证与提交

```bash
cargo test -p token-station-cli --test proxy
cargo test -p token-station-plugin-runtime
cargo test -p token-station-conformance
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

```text
feat: 接入 OpenAI Responses 网关入口
```

## Task 5：CI 和官方插件开发链

### 文件

- 修改 `.github/workflows/ci.yml`。
- 根据实际发现结果最小修改 `apps/cli/src/plugins.rs` 的测试；不增加协议特判。

### 交付物

- 将 `plugins/official/agent-openai-responses` 加入 CI 的独立 WASM cache/workspace 列表。
- 确认开发态 plugin build/test/install/list 能发现新包。
- 如现有 registry 完全按 manifest 自动发现，不修改生产逻辑，只保留测试证据。
- 不把新插件加入 builtin release 包，不改发布脚本或信任链。

### 验证与提交

```bash
cargo test -p token-station-cli --test plugin_devchain --test plugin_install
cargo test -p token-station-conformance --test official_plugins
cargo test -p token-station-plugin-runtime --test official_plugins
./target/release/token-station-cli plugin build plugins/official/agent-openai-responses
./target/release/token-station-cli plugin test plugins/official/agent-openai-responses
```

```text
ci: 纳入 Responses 官方适配器
```

## Task 6：隔离配置与三份接入指南

### 文件

- 新增 `apps/cli/codex-deepseek-config.json`（不含凭证）。
- 新增 `docs/guides/Codex-接入指南.md`。
- 新增 `docs/guides/OpenCode-接入指南.md`。
- 新增 `docs/guides/OpenClaw-接入指南.md`。

### 配置原则

- Codex 用临时 `CODEX_HOME` 中的 `config.toml`，自定义 provider 设置
  `base_url` 为分配到的隔离实例 `/v1` 地址、`wire_api="responses"` 和临时 virtual key
  环境变量名。
- OpenCode 用 `OPENCODE_CONFIG` 或 `OPENCODE_CONFIG_CONTENT`，provider 选
  `@ai-sdk/openai-compatible`，Base URL 指向隔离实例。
- OpenClaw 同时设置临时 `OPENCLAW_CONFIG_PATH` 和 `OPENCLAW_STATE_DIR`，
  `models.providers.<id>.api="openai-completions"`。
- OpenCode 当前本机未安装。执行时在 `/tmp` 下建立独立 npm prefix，安装并锁定
  当日验收版本；不做全局安装。
- 三份指南都写明协议边界、启动、普通对话、流式、工具闭环、错误测试和
  恢复/清理方法。

### 验证与提交

```bash
./target/release/token-station-cli --config apps/cli/codex-deepseek-config.json upstream list
./target/release/token-station-cli --config apps/cli/codex-deepseek-config.json rule list
rg -n "CODEX_HOME|OPENCODE_CONFIG|OPENCLAW_CONFIG_PATH|OPENCLAW_STATE_DIR" \
  docs/guides/*-接入指南.md
git diff --check
```

```text
docs: 新增 M4 多 Agent 接入配方
```

## Task 7：假上游全链路 E2E

### 交付物

在 `apps/cli/tests/proxy.rs` 的现有真实 WASM + router + provider 假上游链路上覆盖：

1. Responses 普通文本。
2. 拆成多个 TCP/SSE 分片的文本增量。
3. function call arguments 分片、function output 回传和第二轮最终文本。
4. 本地 401、上游 401/429/5xx、流中错误。
5. 两个 Responses 流交错时的状态隔离。
6. metrics 中 protocol、requested/routed model、stream、status、usage 与 attempts 正确。
7. requests.log 不含 prompt、response、API key 或 virtual key。

此阶段只使用 `127.0.0.1:0` 和临时目录，不启动固定端口服务。

### 验证与提交

```bash
cargo test -p token-station-cli --test proxy -- --nocapture
```

```text
test: 覆盖 Responses 全链路场景
```

## Task 8：真实 Codex E2E

### 执行前门禁

- 用户当前模型厂商测试仍在运行时，使用新端口与新数据目录。
- 只检查上游 key 是否可解析，不回显值。
- 临时 `CODEX_HOME` 中不复制用户原有 auth/config/history。

### 验收项

1. 记录 `codex --version`。
2. 用 `codex exec` 发出可验证的普通流式回答。
3. 在专用临时工作目录中让 Codex 调用一次本地工具，验证 function call 和
   function output 回传后的最终回答。
4. 使用错误本地 virtual key 验证 Responses 401，不进入 router/upstream。
5. 使用受控的错误上游凭证或假上游验证上游错误。
6. 核对 metrics 和脱敏日志。

### 结论口径

reasoning summary、computer call 或 hosted tool 没有验证时明确写为未支持/未验证，
不从文本 + function tool 通过推导出“Responses 完全兼容”。

```text
test: 验证 Codex Responses 真实链路
```

## Task 9：OpenCode 和 OpenClaw 真实 E2E

两个 Agent 分别启动独立 `agent-openai` 实例；不复用用户当前 `8787` 进程。

每个 Agent 执行同一验收矩阵：

1. 记录实际可执行文件版本。
2. 普通流式文本与固定可验证标记。
3. 一次真实本地工具调用、结果回传和最终回答。
4. 错误本地 virtual key 的 401。
5. 受控上游错误，不触发无限重试。
6. metrics 和日志审计。
7. 还原临时环境并删除仅属于本轮的临时 Agent 配置/运行目录。

任一 Agent 只完成对话、未完成工具闭环或错误链时，结论是“部分通过”，
不记为 M4 该 Agent 验收完成。

```text
test: 验证 Chat Completions 多 Agent 真实链路
```

## Task 10：M4 验收报告

### 文件

- 新增 `docs/verification/2026-07-16-M4-多-Agent-入站验收.md`。

### 报告结构

- 验收对象与实际版本。
- `agent-openai-responses` 五族 conformance 结果。
- 宿主/工作区回归结果。
- 假上游 E2E 结果。
- Codex、OpenCode、OpenClaw 逐项 E2E 证据。
- Responses / IR 能力边界和未验证项。
- 凭证、prompt、运行数据和红线审计。
- 分开标记“本地实现/验证完成”、“PR-ready”、“负责人批准/合并”。

报告不写 push/合并指引，不将本地验证结论写成已合并或已发布。

```text
docs: 记录 M4 多 Agent 入站验收
```

## Task 11：最终自审与红线门禁

从干净 shell 重跑：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
cargo build --release -p token-station-cli
cargo fmt --manifest-path plugins/official/agent-openai-responses/Cargo.toml -- --check
cargo clippy --manifest-path plugins/official/agent-openai-responses/Cargo.toml \
  --target wasm32-wasip2 -- -D warnings
./target/release/token-station-cli plugin build plugins/official/agent-openai-responses
./target/release/token-station-cli plugin test plugins/official/agent-openai-responses
./target/release/token-station-cli plugin build plugins/official/agent-openai
./target/release/token-station-cli plugin test plugins/official/agent-openai
./target/release/token-station-cli plugin build plugins/official/agent-anthropic
./target/release/token-station-cli plugin test plugins/official/agent-anthropic
```

红线与泄密审计：

```bash
git diff origin/main...HEAD -- crates/router-core crates/protocol crates/release \
  plugins/official/provider-openai-compatible apps/cli/build.rs scripts/build-release.sh \
  .github/workflows/release.yml
git diff --check origin/main...HEAD
git status --short
git log --oneline origin/main..HEAD
```

再单独检查：

- Git 变更中无 API key、virtual key、未脱敏 prompt/response、运行日志、metrics DB、
  `adapter.wasm` 或 `target/` 构建物。
- 无 DeepSeek/OpenAI/OpenCode/OpenClaw 厂商或 Agent 专属路由分支；Agent 名只用于
  manifest 能力声明、入站识别、配置与文档。
- 现有 `8787` 进程、用户全局配置和模型厂商测试目录未变。
- 三个 Agent 的五项 E2E 证据齐全；任一缺项则 M4 不标为全部完成。

完成自审后只报告真实状态。是否 push、更新现有 PR 或发起新 PR，必须再获得
用户明确授权。

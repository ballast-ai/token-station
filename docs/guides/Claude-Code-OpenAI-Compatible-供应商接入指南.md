# Claude Code 接入 OpenAI-compatible 模型供应商

本指南说明如何让 Claude Code 通过 token-station 使用任意满足 OpenAI Chat
Completions 契约的模型供应商。Qwen、Kimi、MiniMax、GLM 只是首轮通用性测试样本，
不是产品白名单，也不需要四个厂商专属 provider 插件。

```text
Claude Code
  -> Anthropic Messages
  -> agent-anthropic
  -> Canonical IR -> router
  -> provider-openai-compatible
  -> configured upstream / model
```

## 1. 判断供应商能否直接配置接入

供应商至少应满足：

- 接受 `POST <base_url>/chat/completions`；
- 使用 Bearer API Key；
- 请求和响应兼容 OpenAI Chat Completions；
- 流式响应使用 SSE `data:` 帧和 `[DONE]`；
- 工具调用使用 `tools` / `tool_calls` / `role=tool`；
- 错误能通过 HTTP 状态码及可选 `error.message` 分类。

满足这些条件时，只增加 upstream 配置、模型能力和凭证，不修改 Rust。不同请求、
鉴权、流式或工具协议才需要独立 `provider-*` 插件；Canonical IR 无法表达的关键语义
必须先提交变更请求，不能直接修改 `crates/protocol`。

## 2. 首轮测试配置

| 测试样本 | 配置 | 当前拟测模型 |
|---|---|---|
| Qwen | [`claude-code-qwen-config.json`](../../apps/cli/claude-code-qwen-config.json) | `qwen3.7-max` |
| Kimi | [`claude-code-kimi-config.json`](../../apps/cli/claude-code-kimi-config.json) | `kimi-k2.7-code` |
| MiniMax | [`claude-code-minimax-config.json`](../../apps/cli/claude-code-minimax-config.json) | `MiniMax-M3` |
| GLM | [`claude-code-glm-config.json`](../../apps/cli/claude-code-glm-config.json) | `glm-5.2` |

四份配置都只有一个 upstream 和一个 default pool，`rules` / `hint_routes` 为空。
因此它们验证的是供应商协议兼容性，不是多模型路由策略。

模型别名可能随厂商更新。执行真实验收前应再次核对官方模型目录，并在验收记录中
保存测试日期、请求模型和上游实际返回模型。

## 3. 构建并安装通用插件

在仓库根目录执行：

```bash
cargo build --release -p token-station-cli

./target/release/token-station-cli plugin build \
  plugins/official/agent-anthropic
./target/release/token-station-cli plugin build \
  plugins/official/provider-openai-compatible

./target/release/token-station-cli \
  --config apps/cli/claude-code-qwen-config.json \
  plugin install plugins/official/agent-anthropic
./target/release/token-station-cli \
  --config apps/cli/claude-code-qwen-config.json \
  plugin install plugins/official/provider-openai-compatible
```

四份配置共用 `token-station-e2e/plugins/`，插件只需安装一次。如果开发目录已安装旧包，
应先使用 `plugin remove` 删除并重新安装，不要手工覆盖 WASM 或验收回执。

## 4. 安全写入 API Key

不要把 API Key 发到聊天，也不要写进 JSON、命令行参数或 shell 脚本。以下 zsh 函数
会隐藏终端输入，并通过标准输入把 Key 写入 macOS 钥匙串：

```zsh
store_provider_key() {
  local upstream="$1"
  local label="$2"
  local key

  read -s "key?$label API Key: "
  print
  print -r -- "$key" | \
    ./target/release/token-station-cli key set "$upstream" provider_api_key
  unset key
}

store_provider_key qwen Qwen
store_provider_key kimi Kimi
store_provider_key minimax MiniMax
store_provider_key glm GLM
unset -f store_provider_key
```

CLI 只输出已存储的 upstream/slot，不回显 Key。四份配置使用不同 upstream 名，虽然
slot 都叫 `provider_api_key`，钥匙串条目仍按 `upstream/slot` 隔离。

## 5. 配置检查与真实探活

先检查配置和路由：

```bash
for vendor in qwen kimi minimax glm; do
  config="apps/cli/claude-code-${vendor}-config.json"
  ./target/release/token-station-cli --config "$config" upstream list
  ./target/release/token-station-cli --config "$config" rule list
done
```

再逐家发送一个真实的最小 completion：

```bash
./target/release/token-station-cli \
  --config apps/cli/claude-code-qwen-config.json \
  upstream test qwen --model qwen3.7-max

./target/release/token-station-cli \
  --config apps/cli/claude-code-kimi-config.json \
  upstream test kimi --model kimi-k2.7-code

./target/release/token-station-cli \
  --config apps/cli/claude-code-minimax-config.json \
  upstream test minimax --model MiniMax-M3

./target/release/token-station-cli \
  --config apps/cli/claude-code-glm-config.json \
  upstream test glm --model glm-5.2
```

`upstream test` 会调用真实 API，可能产生少量费用。标准按量 Key 与 Coding Plan、
Token Plan 等套餐 Key 可能使用不同端点；401/404 时先核对 Key 类型和地域，不要直接
判断 provider 不兼容。

## 6. 启动单供应商代理

四家必须逐个测试，避免端口和 metrics 串线。例如启动 Qwen：

```bash
./target/release/token-station-cli \
  --config apps/cli/claude-code-qwen-config.json serve
```

其他供应商只替换配置名。每份配置使用独立数据目录：

```text
token-station-e2e/qwen/data
token-station-e2e/kimi/data
token-station-e2e/minimax/data
token-station-e2e/glm/data
```

首次启动会生成本地 virtual key。它只用于 Claude Code 到本机代理的鉴权，不是上游
API Key，文件权限应为 `0600`。

## 7. 启动 Claude Code

以 Qwen 为例，在另一个终端读取该配置的本地 virtual key：

```bash
export TS_VIRTUAL_KEY="$(tr -d '\r\n' < token-station-e2e/qwen/data/virtual-key)"
```

仅为本次 Claude Code 进程注入配置：

```bash
ANTHROPIC_BASE_URL='http://127.0.0.1:8787' \
ANTHROPIC_AUTH_TOKEN="$TS_VIRTUAL_KEY" \
ANTHROPIC_MODEL='claude-3-5-haiku-20241022' \
MAX_THINKING_TOKENS=0 \
CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING=1 \
CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1 \
CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
claude --model claude-3-5-haiku-20241022 \
  --safe-mode \
  --setting-sources project
```

`claude-3-5-haiku-20241022` 只是 Claude Code 侧的协议兼容标识，真实供应商和模型由
当前单供应商配置决定。不要把上游 API Key 传给 Claude Code。

`--setting-sources project` 用于隔离用户级 Claude settings 中可能覆盖当前 shell 的
网关环境变量；`--safe-mode` 用于首次连通测试时排除个人插件、hooks 和 MCP 干扰。

## 8. 每家验收场景

每个测试供应商至少完成：

1. 普通流式对话，得到固定可验证文本；
2. 要求 Claude Code 使用 `Read` 读取仓库内固定小文件；
3. 工具结果第二轮回传后得到正确最终答案；
4. 错误本地 virtual key 返回 Anthropic 401，且请求不进入上游；
5. metrics 中 upstream/model、stream、tool_count 和状态与当前配置一致；
6. 日志不含 prompt、response、上游 Key 或本地 virtual key。

厂商专有 `reasoning_content`、`reasoning_details`、`<think>` 等字段当前不属于已承诺的
无损 IR 语义。如果它们是工具闭环的必要条件，应将该供应商标记为未兼容并提出变更
请求，不能静默丢失后宣称完整支持。

## 9. 测试完成后删除 Key

```bash
./target/release/token-station-cli key remove qwen provider_api_key
./target/release/token-station-cli key remove kimi provider_api_key
./target/release/token-station-cli key remove minimax provider_api_key
./target/release/token-station-cli key remove glm provider_api_key
```

删除后再次执行 `upstream test` 应得到钥匙串缺失错误且不发出上游请求。不要为了验证
删除结果而重新把 Key 写入环境变量或临时文件。

## 10. 新供应商接入规则

后续供应商满足 §1 时，复制任意一份配置并替换：

- upstream 名；
- Base URL；
- Key 来源；
- 模型 ID 和能力。

验证配置、probe、Claude Code 对话和工具闭环通过后即可接入，不需要修改 Rust。
只有真实证据证明协议不兼容时，才决定增强通用 provider 或新增独立 provider 插件。

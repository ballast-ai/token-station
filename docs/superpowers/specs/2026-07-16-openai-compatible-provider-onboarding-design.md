# OpenAI-compatible 模型供应商通用接入设计

## 1. 目标

建立一条不依赖具体厂商名称的模型供应商接入路径：只要供应商满足本设计定义的
OpenAI-compatible 出站契约，运营者即可通过配置新增 upstream、凭证来源和模型能力，
复用现有 `provider-openai-compatible`，无需修改 Rust 业务代码。

Qwen、Kimi、MiniMax、GLM 只是首轮通用性测试矩阵，用来覆盖不同厂商实现差异，
证明接入机制不是 DeepSeek 或四家厂商的专属实现。它们不是产品白名单、硬编码枚举，
也不分别对应新的 provider adapter。

本设计建立在已完成的 Claude Code Anthropic 入站链路之上：

```text
Claude Code
  -> agent-anthropic
  -> Canonical IR
  -> 现有 router
  -> provider-openai-compatible
  -> 配置指定的 upstream / model
```

## 2. 核心结论

新增供应商是否需要改代码，只由其出站协议方言决定：

| 供应商类型 | 接入方式 | 是否修改 Rust |
|---|---|---:|
| 满足本设计契约的 OpenAI-compatible 供应商 | 新增配置并完成 E2E | 否 |
| OpenAI-compatible 但存在可通用处理的小差异 | 先用 fixture 证明，再增强通用 provider adapter | 可能 |
| 使用不同请求、鉴权、流式或工具协议 | 新增独立 `provider-<dialect>` 插件 | 是，仅限插件边界 |
| 需要 Canonical IR 无法表达的关键语义 | 先提交 IR 变更请求并等待评审 | 未批准前不得修改 IR |

任何一种接入都不应在 `agent-anthropic`、Gateway 或 router 中增加厂商名分支。

## 3. OpenAI-compatible 接入契约

供应商同时满足以下条件时，可按配置直接接入：

1. 接受 `POST <base_url>/chat/completions`。
2. 使用 `Authorization: Bearer <API key>`。
3. 请求主体兼容 OpenAI Chat Completions 的 `model`、`messages`、`stream`、
   `temperature`、`top_p`、`max_tokens`、`stop` 和 `tools`。
4. 非流式响应使用 `choices[*].message.content` / `tool_calls`、
   `finish_reason` 和标准 usage 结构。
5. 流式响应使用 SSE `data: <JSON>` 帧，以 `[DONE]` 结束；文本位于
   `choices[*].delta.content`，工具增量位于 `delta.tool_calls`。
6. HTTP 错误可按状态码和可选的 `error.message` 映射到 token-station 的闭合错误码。
7. 厂商专有字段即使存在，也不能是完成普通对话、流式或工具闭环所必需且当前
   provider adapter 会丢失的关键语义。

“厂商声称兼容”只能作为候选条件，不能替代真实 E2E。最终以请求和响应证据判断。

## 4. 配置设计

### 4.1 每个供应商一个 upstream

每个测试或生产供应商使用独立 upstream 条目，隔离 Base URL、凭证和模型目录：

```jsonc
"upstreams": {
  "vendor_name": {
    "provider": "openai-compatible",
    "base_url": "https://vendor.example/v1",
    "auth": {
      "slot": "provider_api_key",
      "env": "VENDOR_API_KEY"
    },
    "models": [
      {
        "model": "vendor-model-id",
        "tool": true,
        "json_schema": true,
        "context_window": 200000,
        "supported_parameters": [
          "max_tokens",
          "stop",
          "temperature",
          "top_p"
        ]
      }
    ]
  }
}
```

模型能力必须按厂商官方文档和实测结果保守声明。未声明的 `tool`、`vision`、
`json_schema` 等能力视为不支持，不能为了让路由命中而乐观填写。

### 4.2 每份验收配置只启用一个供应商

首轮 E2E 使用四份单供应商配置，每份配置保持：

- `plugins.agent = "agent-anthropic"`；
- `plugins.providers.openai-compatible = "provider-openai-compatible"`；
- 一个被测 upstream；
- 一个只包含被测模型的 default pool；
- `rules = []`；
- `hint_routes = []`。

这样可以证明请求确实到达被测供应商，并将供应商兼容性问题与多模型路由问题隔离。
这不修改路由逻辑，也不把四家供应商写入产品级路由配置。

### 4.3 后续供应商接入

后续新增满足 §3 的供应商时，复制通用 upstream 模板并填写 Base URL、凭证来源、
模型目录和能力即可。无需增加厂商枚举、Rust match 分支或新的 agent adapter。

provider adapter 处于无网络 WASM 沙箱中，不能主动查询远端模型目录；同时路由需要
可信的工具、视觉、上下文窗口等能力声明。因此本设计不做运行时自动发现，模型目录
由运营者显式配置并通过 `upstream test` / E2E 验证。

## 5. 首轮通用测试矩阵

以下内容是 2026-07-16 的测试输入，不是长期产品契约。测试执行前再次核对官方模型
目录，并使用当时最新稳定模型别名；验收记录同时保存日期、请求模型和上游实际返回
模型，避免别名后续漂移导致证据失真。

| 测试样本 | 中国区标准 API Base URL | 当前拟测模型 | 目的 |
|---|---|---|---|
| Qwen | `https://dashscope.aliyuncs.com/compatible-mode/v1` | `qwen3.7-max` | 地域化端点、thinking 与标准工具调用 |
| Kimi | `https://api.moonshot.cn/v1` | `kimi-k2.7-code` | Coding 模型、SSE、工具调用与 reasoning 扩展 |
| MiniMax | `https://api.minimaxi.com/v1` | `MiniMax-M3` | `<think>` / interleaved thinking 与工具历史连续性 |
| GLM | `https://open.bigmodel.cn/api/paas/v4` | `glm-5.2` | 默认 thinking、标准与流式工具调用差异 |

首轮假设使用标准按量 API Key。Coding Plan、Token Plan 或其他套餐专用 Key 可能使用
独立端点，不纳入本轮通用 API 验收；若实际凭证属于套餐 Key，先更换对应官方端点，
不能把端点不匹配误判为 provider 兼容失败。

官方资料：

- Qwen：<https://help.aliyun.com/zh/model-studio/qwen-api-reference/>
- Kimi：<https://platform.kimi.com/docs/api/chat>
- MiniMax：<https://platform.minimaxi.com/docs/api-reference/text-openai-api>
- GLM：<https://docs.bigmodel.cn/cn/guide/develop/openai/introduction>

## 6. 数据流与厂商中立边界

```text
Anthropic Messages
  -> agent-anthropic.normalize_inbound
  -> ChatRequest
  -> router.route（现有实现）
  -> provider-openai-compatible.build_http_request
  -> host 校验目标端点并注入该 upstream 的凭证
  -> vendor /v1/chat/completions
  -> provider-openai-compatible.parse_response / parse_stream_chunk
  -> ChatResponse / StreamEvent
  -> agent-anthropic.render_response / render_stream_event
  -> Anthropic JSON / SSE
```

不变量：

- 入站协议由 agent adapter 决定，模型供应商由 provider/upstream 配置决定。
- provider adapter 不持有明文 Key，只声明 `provider_api_key` 槽位。
- Gateway 只编排通用组件，不识别 Qwen、Kimi、MiniMax、GLM 等名称。
- router 只接收 Canonical IR、hint、候选模型能力和健康状态，不处理厂商方言。
- 四个测试样本不能产生四套复制的 provider 代码。

## 7. Thinking 与专有字段边界

四家最新模型均可能返回 `reasoning_content`、`reasoning_details`、`<think>` 内容或要求
额外的 thinking/tool-stream 参数。当前 Canonical IR 不能完整表达所有厂商的思考块，
现有通用 provider 也只保证标准 OpenAI 文本、工具调用、usage 和 SSE 字段。

本轮“完整 Claude Code E2E”表示以下功能链通过：

- 普通回答；
- 流式文本；
- 模型发起工具调用；
- Claude Code 执行工具并回传结果；
- 模型根据工具结果给出最终回答；
- 错误以 Anthropic 形状返回。

它不表示厂商专有思考内容已被无损保留。如果某模型必须回传专有思考字段才能完成
工具闭环，则该供应商不能按配置直接接入：记录真实失败证据，提交 IR 变更请求，
在评审前不修改 `crates/protocol`，也不通过拼接文本伪造语义兼容。

## 8. 错误与凭证处理

每个供应商使用独立环境变量或系统钥匙串项。配置、fixture、文档、日志和 Git 历史
均不得出现真实 API Key。

| 场景 | 预期行为 |
|---|---|
| 本地 virtual key 缺失或错误 | 返回 Anthropic 401，不进入 Gateway 路由或上游 |
| 上游 API Key 错误 | provider 映射为 auth 错误，再渲染为 Anthropic 错误 |
| 请求参数不被供应商接受 | 返回 invalid request，不静默删除关键参数后重试 |
| 上游限流 | 保留 429 与可用的 retry-after |
| 上游 5xx / 超时 | 映射为 upstream unavailable / timeout |
| 流中断 | 结束当前响应并记录不含内容的错误与路由元数据 |

为了避免 Claude Code 对 401 的自动重试干扰验收，上游错误可优先通过单次直接 HTTP
请求或受控 mock 验证；真实 Claude Code 只验证对最终错误形状的兼容性。

## 9. 测试与验收

### 9.1 静态与配置门

每份配置必须通过：

- 配置解析；
- `plugin list` / provider 方言解析；
- `upstream list`；
- `rule list`；
- `upstream test <name> --model <model>`。

### 9.2 每个测试供应商的真实 E2E

1. **普通流式对话**：Claude Code 得到可验证的最终文本。
2. **工具闭环**：要求 Claude Code 使用 `Read` 读取仓库内固定小文件；metrics 必须出现
   至少一轮 `tool_count > 0`，第二轮携带 `tool_result` 并得到正确答案。
3. **错误回传**：验证本地鉴权失败和上游鉴权失败不会串到其他 upstream。
4. **路由证据**：metrics / requests log 中的 upstream 和 model 必须与被测配置一致。
5. **安全审计**：日志不含 prompt、response、上游 API Key 或本地 virtual key。

四家全部通过才能证明本轮通用测试矩阵通过；任一家失败必须标记为“未兼容”或
“有条件兼容”，不得用其他三家的成功替代。

### 9.3 通用 provider 回归

即使预期只增加配置，也必须重跑：

- `provider-openai-compatible` conformance；
- `agent-anthropic` conformance；
- Gateway 相关测试；
- workspace format / clippy / test / release build。

若真实 E2E 暴露通用 adapter 缺陷，代码修复必须先新增可复现 fixture，并证明改动是
厂商中立的 OpenAI 方言兼容增强，且不破坏现有 DeepSeek 验收。

## 10. 交付物

本轮预期交付：

- 四份不含凭证的单供应商 E2E 配置；
- 一份 OpenAI-compatible 供应商通用接入指南；
- 一份按供应商分项记录的真实 E2E 验收文档；
- 如发现通用兼容缺陷，附最小 fixture、修复和回归证据；
- 如发现 IR 缺口，附独立变更请求，不混入未经批准的 IR 修改。

## 11. 变更边界

预期修改：

- `apps/cli/` 下的供应商测试配置；
- `docs/guides/` 通用接入指南；
- `docs/verification/` E2E 证据；
- 只有出现已证明的通用兼容缺陷时，才修改
  `plugins/official/provider-openai-compatible/**` 及其 fixtures。

明确不修改：

- `crates/router-core`；
- `crates/protocol`；
- `agent-anthropic` 的厂商无关边界；
- Gateway 中的路由语义；
- `crates/release` 信任链；
- 现有 DeepSeek 配置和验收记录；
- 任何用户全局 Claude Code 配置。

## 12. 完成定义

只有以下条件同时成立时，本轮才能称为“OpenAI-compatible 供应商通用接入已验证”：

- 四个代表性供应商均通过普通对话、流式、工具闭环和错误回传；
- 每份 E2E 均有实际 upstream/model 证据；
- 没有新增厂商名业务分支或四套重复 provider 插件；
- 没有修改 router 或未经批准修改 IR；
- 真实凭证未进入仓库、日志或验收文档；
- 文档明确四家只是测试样本，未来满足契约的供应商通过配置接入；
- 所有自动化回归通过；
- 改动仅本地提交，未 push，最终仍通过 Pull Request 合入。

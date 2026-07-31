# CC deep-research release 修复:原生 Anthropic 透传 + 健康鲁棒性 + tool_choice

- 日期：2026-07-31
- 触发：Claude Code `/deep-research` 经 token-station → DeepSeek 的端到端实测（`2026-07-31-CC-deep-Seek-实测.md`）。基础对话 / 单工具 / 多轮工具全通；deep-research 的 web search 步骤断，且高并发下单上游被健康摘除级联出 43 次 503。
- 状态：全部实现并测试通过（workspace 545 / official_plugins wasm / cli proxy 集成 / vitest 189）。本地 develop。

## 根因（实测 + 源码坐实）

1. **web search 断**：CC 的 WebSearch 是两级——主模型看到普通 function，执行时 CC harness 发**二级 `/v1/messages`** 带 Anthropic **服务端工具** `web_search_20250305` + `tool_choice:{type:tool}`。token-station 把一切碾进 Canonical IR 渲染到 DeepSeek 的 **OpenAI /chat/completions**，`validate_tool_choice` 只放 `auto` → 400，`attempts=0` 根本没到 DeepSeek。
   - ★对照 cc-switch：它对 Claude Code 默认 `apiFormat=anthropic` = **纯透传**，把原始 body 原样发到 DeepSeek 的 **Anthropic 兼容端点** `/anthropic`，web_search + tool_choice 分毫不动，**搜索由 DeepSeek 端点自己跑**（cc-switch 不自建搜索，全仓 grep 零）。差别就是 **passthrough vs translate**。
2. **43 次 503 级联**：post-200 流截断（`FailedAfterPartial` / `TransportTruncated`）被 `settle()` 当作与"上游宕机"同级的健康失败计入摘除；默认 `eject_after=3` → 3 次瞬时截断摘掉**唯一** DeepSeek 候选 30s，pool 空 → 后续全 503。
3. **tool_choice 报错措辞错**：`"cannot be preserved by Canonical IR"` 是假的——`ToolChoice::Other(Value)` 能装，是自订门禁。

## 已修

### A. 原生 Anthropic 透传（web_search 真能用；mirror cc-switch，更严）
- **配置**：`UpstreamConfig` 加 `api_dialect: ApiDialect`（默认 `translated`；`anthropic-native` 开透传）。纯 `apps/cli`，**不动 WASM ABI / protocol crate**。
- **gateway**：`chat_inner` 顶部、`normalize_request` 之前分叉——入站 `anthropic-messages` 且最小请求路由到 `anthropic-native` 上游时走 `passthrough_upstream`：原始 body 只改 `model` → 白名单 header（content-type/accept/anthropic-version/anthropic-beta）→ `HttpRequestDescriptor` 打 `base_url.resolve(Messages)` → **复用现有 `authorize()`→`send()`→`reject_redirect`**。流式用 `relay_raw_sse` 逐帧原样回传，非流式/上游错误 `BeginJson` 原样透传。**完全跳过 Canonical IR**，故 web_search + tool_choice:{type:tool} + server-tool 历史 + thinking 全部原样活。
- **比 cc-switch 更严**：egress 只从 base_url 推导（永不用客户端 Host）+ authorize 门禁；客户端 authorization/x-api-key 被 `SafeHeaders` 硬拒不可能转发上游、上游 key 由 host 在 `resolve_auth` 注入；仍记录无内容 RequestRecord + passthrough ConversionRecord + quota 头摘取；不注入 cc-switch 的 `claude-code` 指纹。
- **红线说明**：透传**有意放弃** Canonical 路由/打分/quota（换取原生能力存活），但保留 egress/key 隔离/记录，且不给任何 plugin 新权限（纯 host 侧）。这是设计文档 §3 停留项"原生透传通道"的落地，仅对 operator 显式标 `anthropic-native` 的单上游生效。
- **⚠️ 配置坑**：DeepSeek 端点是 `/anthropic/v1/messages`,`resolve()` 不自动插 `/v1`,故 `base_url` 须配 `https://api.deepseek.com/anthropic/v1`。
- **auth**：MVP 用 `Auth::bearer`（DeepSeek `/anthropic` 用 Bearer，与现有 provider 一致）；native `api.anthropic.com` 的 x-api-key 是后续 polish。
- **测试**：`anthropic_native_passthrough_forwards_verbatim_and_injects_the_upstream_key`（端到端：web_search+tool_choice 原样到上游、model 重映射、上游 key 注入、客户端 token 不外泄、响应原样回传）+ `api_dialect` config 往返单测。

### B. 健康摘除鲁棒性（单上游 deep-research 不再级联）
- **`settle()` 拆 arm**（`gateway.rs`）：`FailedBeforeOutput`（没出过字节 = 真上游不适）仍计入摘除；**`FailedAfterPartial`（post-200 截断，瞬时并发丢流）只记 502 不计摘除**。3 次瞬时截断不再摘掉唯一候选。
- **单候选失败保护**（`router-core/route.rs` `usable_targets` 加 `last_resort` 参数）：最终全池选择时,rotation 全空 → 把被摘候选当**最后手段**探测(而非硬 503),成功即清摘除;**`local_only` 的本地子集调用传 `false`,拒绝/cloud-fallback 语义原样保留**;多候选池行为不变。
- **测试**：`an_all_ejected_pool_degrades_to_last_resort_and_an_incapable_one_hard_fails`；`local_only` 两测原样通过（证明语义保留）。

### C. tool_choice 措辞 + translate 路径
- `validate_tool_choice`：接受 `auto|any|tool`，措辞改"not supported by the configured chat provider"（不再甩锅 IR）。
- 映射：`any`→`Required`（无损），`tool`→`Auto`（诚实降级——egress 原样序列化,chat wire 不认 Anthropic `{type:tool}` 形状,且 forced 的常是 chat 跑不了的 server 工具）。透传路径则原样保留 `{type:tool}`。
- **测试**：`anthropic_tool_choice_is_translated_not_refused`（wasm）+ 更新 `anthropic_forced_tool_choice_is_translated_and_reaches_the_upstream`（集成）。

## 未做（清楚界定）
- passthrough 的 x-api-key auth toggle（native Anthropic）、多上游 failover、DeepSeek 专属 body 修正（thinking 历史回填 d1 / effort-strip d2）、流式 usage 嗅探——都是 polish，MVP 不阻断。
- per-provider 并发默认（admission.rs 16→8）：NICE-TO-HAVE，(A)/(B) 已解级联，未改。
- CC print-mode 后台任务 600s 终止的验收终态：CC 侧行为,接入指南标注即可。

## 验证
- `cargo test --workspace`：**545 passed / 0 failed**。
- `cargo test -p token-station-cli`（含 proxy 集成 60）：全绿。
- official_plugins 真实 wasm：全绿。
- vitest：**189 passed**。

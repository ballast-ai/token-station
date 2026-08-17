# `token-station-server` 宿主接入（host-adoption）验证方案

> 前置文档：`2026-08-17-token-station-south-minimal-provider-call-implementation.md`（south 纵切实施记录）、`token-station-south/docs/design/2026-08-16-minimal-provider-call.md`（英文设计基线）、企业仓 `docs/供应商适配层拆crate-方案线稿.md`（31 号计划，G0/G1 已完结）、企业仓 `docs/product-review/32-非文本计费迁移C4执行计划.md`（sealed settlement 不变式）。
>
> 状态：P1–P4 已在企业仓 `feat/south-host-adoption` 分支实施完成（commit `d3ab9136`，2026-08-17），D1/D2/D3 按本方案推荐口径执行。实施中的两处偏差：(1) P4 的"直接切换"落地为**路由资格二分**——非纯 Bearer provider 与任何资格判定失败回落 legacy 路径（本方案 §7 的 scope 排除在同一端点上共存所致，不是运维开关）；(2) ~~south 依赖暂为 path 指向本地 worktree~~——P0 已全部完成（south PR #1 合入 main、CI 全绿、tag `v0.0.1` 发布），企业仓依赖已切 git+tag（commit `d54a774c`）。P5 剩余：wiring review（人工，硬门禁）→ compatibility.json 翻转 → D2 豁免/立项/实施记录文档 → 合入 `dev`。注意：south 仓为 ballast-ai 下私有仓（Cargo.toml repository 字段仍误写 GlimpseEngine，待修），企业仓远端 CI 构建需先配 deploy key/token。
>
> 调研基线：south `feature/minimal-provider-call`（`fd8df13`）；企业仓 `provider-crate-plan` 分支（`3706d162`）。两边代码都在变化，启动实施前应复核行号漂移。
>
> 更新日期：2026-08-17

---

## 1. 目标与"验证通过"的定义

south 设计文档对 host-adoption slice 的要求是三件事：**(a)** 选定一个真实的企业侧 Bearer JSON POST 调用点；**(b)** 宿主 adapter 针对 south 编译通过并接入该调用点；**(c)** 用真实 adapter 跑通公开的 `south.provider-call.v1` conformance suite（7 用例）。

两条硬性澄清（均出自 south 设计文档原文）：

1. **通过 suite ≠ verified**。conformance 的 call count / drop flag 是 adapter 自报证据，runner 无法独立插桩。必须有一次人工 wiring review，确认 `ProviderCallEvidenceV1::new` 的四个入参真实接在 adapter 的 resolver/transport 边界和 drop guard 上，而不是硬编码期望值。
2. 只有 (a)(b)(c) + wiring review 全部完成，才允许把 south 仓 `compatibility.json` 的 `hosts.token-station-server` 从 `not_verified` 翻转。

本方案的最终验收物：企业仓一条真实业务路径经 south `execute_provider_call_v1` 发出请求并全量测试通过；south 仓 conformance suite 以企业 adapter 为 executor 7/7 通过；wiring review 记录在案；`compatibility.json` 状态翻转的 PR 合入。

---

## 2. 接入点裁决：`/v1/embeddings` OpenAI-compat 分支

**选定**：`gateway/src/modules/inference/handler/embeddings.rs` 的 OpenAI-compat 路径（约 56-311 行），把其中 builder 构造 + 发送段（约 201-236 行）替换为 `ProviderCall`。

它是全仓 51 处 `send_traced` 直发点中，唯一同时满足全部约束的真实业务调用点：

| 约束 | 满足情况 |
|---|---|
| Bearer 认证 | `apply_provider_auth` 同步注入，OpenAI/Deepinfra 等纯 Bearer 供应商直接覆盖 |
| JSON POST | 固定 `POST {base}/v1/embeddings`，`content-type: application/json` |
| 非流式、buffered | 请求 `Vec<u8>` 全量、响应 `read_json_body_capped` 有界读取 |
| 无重试耦合 | 已是 single-shot sealed durable send（32 号 C4.4 改造产物），不需要拆重试逻辑 |
| 波及面可控 | Gemini embeddings 走独立函数 `proxy_gemini_embeddings`，不受影响 |

**候选否决清单**（调研结论留档，避免复议）：

| 候选 | 否决原因 |
|---|---|
| `center_pull::fetch_center_manifest` | 教科书级纯 Bearer + 有界 body，但是 GET；`ProviderCall` v1 只有 POST |
| `health_probe::probe_one` | 零资金风险，但 GET 无 body |
| ElevenLabs TTS | `xi-api-key` 非 Bearer，响应为音频二进制 |
| 非流式 chat completions | 与流式共路径，耦合 routing/failover/breaker/结算，31 号计划明确的资金正确性边界，不做第一刀 |
| images/video/music 媒体面 | multipart、submit+poll 轮询、SSE 计费，各违反至少一条约束 |

**必须保住的不变式**（32 号计划 + 源码棘轮测试）：`admit_text_dispatch` / `begin_dispatch` / `.finalize` / `delivery_unknown` 四类调用点的数量与顺序不变——`embeddings.rs` 的棘轮测试用 `include_str!` 数字符串出现次数，`ProviderCall` 只替换中间的发送段。`begin_dispatch()` 之后的一切失败路径必须继续走 `delivery_unknown`。

---

## 3. 前置决策（P1 启动前拍板）

### D1 — reqwest 依赖门禁在宿主侧的口径

south 仓的 `check-boundaries.sh` 要求解析图中 reqwest **恰好一个包、恰好 `0.12.28`、feature 集严格等于 7 项**。这个门禁在企业仓 workspace **必然失败**：企业仓 `Cargo.lock` 有三个 reqwest 版本（0.11.27 传递依赖、0.12.28 主栈、0.13.4 OTLP exporter），且主栈开了 `json`/`multipart` feature。

**建议**：south 的严格门禁保持只约束 south 自己的 workspace；为宿主定义一套等价但现实的门禁并写进本方案 P1 验收——(1) `south-transport-reqwest` 与宿主主栈解析到**同一个** `reqwest 0.12.28` 节点；(2) 统一后 feature 含 `rustls-tls`+`stream` 且**不含** `default-tls`/`native-tls`/`cookies`/`system-proxy`（防 OpenSSL 进树、防隐式代理回归）；(3) 0.11/0.13 两个既有旁支不新增消费方。此口径需要 south 侧认可并记入 south 文档（避免"宿主违反了 south 边界"的误读）。

### D2 — `send_traced` 收编口径的显式豁免

企业仓 E4.3b 拍板要求 47 处直发点全部收编进 `send_traced`（OTel span + traceparent 注入）。切到 `ProviderCall` 后，embeddings 这一处不再经过 `send_traced`。

**建议**：不要求 south 提供注入钩子。`traceparent` 不在 south 的 20 项保留 header 黑名单内，宿主把它作为普通 header 放进 `JsonPostRequestV1` 的 `SafeHeaders`；upstream span 与 RED metrics 由宿主在 `execute_provider_call_v1` 调用外层自行包裹。在企业仓文档中为该调用点记录一条 E4.3b 豁免（理由：传输所有权移交 south，观测语义等价保留）。

### D3 — 接受 south transport 自建 client（放弃连接池共享）

网关靠单一共享 `reqwest::Client` 做连接池复用；south 的 `ReqwestTransportV1` 自建加固 client（禁代理、禁重定向、禁压缩、禁 Cookie），且设计上不允许 reqwest 类型跨越其公开边界——要求 south 开"注入宿主 client"的口子会破坏它的核心边界设计。

**建议**：接受第二个 client。embeddings 流量低，连接池损失可忽略；`ReqwestTransportV1` 在进程内作为单例常驻（构造一次，随 `AppState` 存活），自身仍有连接复用。两点行为差异必须写进等价性测试与实施记录：(1) 共享 client 未禁代理（跟随 `HTTP_PROXY` 环境变量），south transport 强制 direct——部署环境若依赖出口代理，此调用点行为改变；(2) south 对 3xx 一律 `REDIRECT_DENIED`，共享 client 走 reqwest 默认重定向策略——API 上游实际不重定向，语义收紧可接受。

---

## 4. 架构映射

### 4.1 概念对应表

| 网关概念 | south 类型 | 映射说明 |
|---|---|---|
| provider base URL（`build_upstream_url` 的 base 部分） | `ProviderEndpointV1::parse` | 启动期/配置变更期解析并缓存；解析失败的 provider 不进入 embeddings 可用集 |
| 固定路径 `"/v1/embeddings"` | `RelativePathV1::parse("v1/embeddings")` | 注意 south 相对路径**不带前导 `/`**；P2 需验证 `build_upstream_url` 对 scope 内供应商的输出能分解为 endpoint + relative path，不能分解的供应商排除出本纵切 scope |
| provider 名 | `CredentialSlotV1` | slot 取 provider name 小写规范化（规则：1-64 ASCII、首字符 a-z、其余 a-z0-9._-）；规范化后仍不合法者启动期报配置错误 |
| endpoint + slot | `ProviderBindingV1::new` | 与 endpoint 一起缓存，作为"一个 endpoint 只授权一个凭证槽"的信任锚点 |
| `credential_store.snapshot` + `weighted_choices` 选出的 `RuntimeCredential.api_key` | `CredentialResolver` 实现 | 见 §4.2 |
| 请求体 `Vec<u8>` | `JsonBodyV1::parse` | 需先 `String::from_utf8`；south 会校验"恰好一个完整 JSON 值"且 ≤ 32 MiB——比现状更严，畸形 JSON 从"透传上游拒绝"变为宿主侧 `INVALID_JSON_BODY`，等价性测试要覆盖此差异 |
| `content-type` + `traceparent` | `SafeHeaders::try_from_iter` | 均不在保留黑名单；重复 header 会报错而非覆盖 |
| `.timeout(upstream_non_streaming_timeout)` | `deadline = Instant::now() + total` | 绝对 deadline 由宿主换算 |
| （现状无请求级取消） | `CancellationToken` | 生产路径 v1 传不触发的 token；conformance 的取消用例由 testkit fixture 驱动同一条 adapter 代码路径 |
| 共享 client 的 connect 10s / read 超时 | `ReqwestTransportConfigV1::try_new(total, connect, read)` | total = `upstream_non_streaming_timeout`；connect = 10s；read = 与现共享 client 同口径 |

### 4.2 `CredentialResolver` 实现形态

网关凭证在内存中同步可得（`CredentialStore` 读锁 clone），凭证选择（加权）发生在调用之前。因此 resolver 是一个薄包装：handler 选好 `RuntimeCredential` 后，构造一个持有该凭证 `api_key` 的一次性 resolver，`resolve` 返回 `SecretValue::new(api_key)`。south 版本一不限制 secret 大小，宿主按设计文档要求自行 bound（建议 ≤ 8 KiB，超限映射为解析失败）。宿主具体失败原因留在宿主日志，对 south 一律 `CredentialResolutionErrorV1`。

### 4.3 结果与错误映射（资金安全的核心）

| south 返回 | 网关处理 | 结算口径 |
|---|---|---|
| `Ok(resp)`，`status` 2xx | 走现有成功路径（body 已是有界 `&str`，替代 `read_json_body_capped`） | `finalize` 正常结算 |
| `Ok(resp)`，`status` ≥ 400 | south 把 4xx/5xx 视为**成功的 transport outcome**，body 保留——正好对应现状"上游明确拒绝"分支 | `delivery_unknown` + `UpstreamError`，口径不变 |
| `Ok(resp)`，`status` 3xx | 不会出现（south 一律 `REDIRECT_DENIED`） | — |
| `Err(Preparation(_))`（含 `CREDENTIAL_RESOLUTION_FAILED`/`DEADLINE_EXCEEDED`/`CANCELLED`） | 发送前失败（含 URL/slot 校验失败） | **注意**：若失败发生在 `begin_dispatch` 之后，仍必须走 `delivery_unknown`；建议把全部 south 调用放在 `begin_dispatch` 之后统一映射，不做"发送前失败可豁免"的精细化 |
| `Err(Transport(_))`（9 变体） | transport 不确定 | `delivery_unknown` |

响应 body 上界：south 固定 32 MiB；网关 `response_body_budget` 若更小，宿主在拿到 buffered body 后补一次预算检查。语义差异（south 先 buffer 到 32 MiB 再由宿主裁决 vs 现状边读边裁）记入实施记录。

---

## 5. 分阶段执行与验收标准

### P0 — 前置（south 仓侧）

1. push `feature/minimal-provider-call` + 主仓 `main` 领先提交，开 PR，远端 CI 全绿，合入 `main`，打 tag（如 `v0.0.1`）。
   - 验收：south 仓 `main` 上 tag 可被 git 依赖引用；CI 记录在案。
2. §3 三项决策拍板，结论写回本文档；D1 的宿主侧门禁口径同步记入 south 文档。
   - 验收：本文档 §3 各项标注"已拍板 + 结论"。

### P1 — 依赖引入（企业仓）

1. `[workspace.dependencies]` 增加 `south-contracts`/`south-core`/`south-transport-reqwest`（生产）与 `south-testkit`/`south-provider-conformance`（dev），开发期 `path` 或 `[patch]` 指向本地，落地前切 git + tag（沿用 token-station 三 crate 的既有接法与"依赖只许闭源 → 开源单向"红线）。
2. 验收（全部可脚本化，建议直接落成 CI 检查）：
   - `cargo build` 全 workspace 通过；
   - `cargo tree -i reqwest@0.12.28` 显示 south transport 与 gateway 主栈共享同一节点；
   - `cargo tree` 全图不含 `openssl-sys`/`native-tls`；统一后的 reqwest 0.12.28 feature 集不含 `cookies`/`system-proxy`/`default-tls`；
   - `cargo deny check` 通过（多版本告警不新增）。

### P2 — adapter 模块（企业仓）

1. 新增 `gateway/src/modules/inference/engine/south_adapter.rs`（或独立小模块）：endpoint/binding 缓存构造、一次性 `CredentialResolver`、`ReqwestTransportV1` 单例、请求组装（path/headers/body）、§4.3 错误映射。**接口只收值类型**（`&str`/`String`/`Vec<u8>`），不引用 `InferenceRuntime`/`ProviderConfig`/`RuntimeCredential`/`AppError`（31 号计划红线的镜像约束——虽然方向相反，口径一致：south 不认识宿主类型，adapter 模块把宿主类型拆成值再进 south）。
2. 验收：
   - adapter 单测覆盖：endpoint 解析失败的 provider 被排除、slot 规范化、`Vec<u8>` → `JsonBodyV1` 的畸形 JSON 分类、§4.3 映射表逐行（含 ≥400 保 body、9 个 transport 变体归并 `delivery_unknown`）；
   - `build_upstream_url` 分解验证：对 scope 内每个纯 Bearer OpenAI-compat 供应商断言 endpoint+relative path 重组结果与原 URL 逐字节相等。

### P3 — conformance suite（企业仓跑 south 的 7 用例）

1. 实现 `AssembledProviderCallExecutorV1`：内部走**真实 adapter 代码路径**（P2 的组装 + `execute_provider_call_v1`），在自己的 resolver/transport 包装上埋真实计数器与 `Drop` guard，经 `ProviderCallEvidenceV1::new` 报证据。
2. 按 south 参考模板运行：`current_thread` + `start_paused` 虚拟时钟、`tokio::join!` 结构化并发 deadline driver、外层 `tokio::time::timeout` watchdog、不 spawn。
3. **登记陷阱**：企业仓 `gateway` 是 `autotests = false`，新测试必须挂进某个 `group_*.rs` 或在 `gateway/Cargo.toml` 显式登记 `[[test]]`，否则永不运行且不报错。
4. 验收：suite 7/7 通过（`passed_case_ids().len() == 7`）；测试已登记并在 CI 实际执行（以 CI 日志出现该 target 为准）；证据埋点代码位置记入实施记录供 wiring review。

### P4 — 调用点切换（embeddings）

1. 替换 `embeddings.rs` 发送段为 adapter 调用；直接切换、不加运行时开关（回退手段 = git revert；该路径 single-shot、低流量，开关只会增加长期复杂度）。
2. wiremock 等价性测试（沿用 `retry/tests_b.rs` 的两个 `post_json_attempt` 单测形态改造）：成功 2xx、上游 4xx（body 保留进 `UpstreamError`）、慢滴超时（total deadline 生效 → `delivery_unknown`）、超限响应体、畸形请求 JSON（新语义：宿主侧拒绝）。
3. 验收：
   - 上述等价性测试全绿并完成 `[[test]]` 登记；
   - `embeddings.rs` 源码棘轮测试不红（四类结算调用点计数不变）；
   - 现有 `group_billing` 等相关测试组全绿；
   - proptest 计费守恒不变量测试全绿。

### P5 — 收口与状态翻转

1. wiring review：reviewer 对照 P3 的证据埋点，确认四项证据接在真实边界；结论记录在案。
2. south 仓 PR：`compatibility.json` 的 `hosts.token-station-server` → 已验证状态；附企业仓 commit/CI 引用。
3. 企业仓文档：D2 豁免记录 + 在 `docs/product-review/` 立项编号收口（south 是第一个带 I/O 的外部 crate，先例必须显式立项，不塞进 `gateway-provider-protocol` 的零 I/O ALLOW-list）。
4. 主仓（token-station）：更新中文实施记录，链接本方案。
5. 验收：三个仓的 PR 均合入；`compatibility.json` 状态与事实一致。

---

## 6. 风险清单

| # | 风险 | 缓解 |
|---|---|---|
| 1 | feature unification 拖 OpenSSL 进树，破坏 rustls-only 与 musl 静态链接 | P1 的 `cargo tree` 门禁固化进 CI |
| 2 | 结算不变式被破坏（`begin_dispatch` 后失败未走 `delivery_unknown`） | §4.3 统一映射 + P4 棘轮/守恒测试 |
| 3 | 代理行为静默改变（共享 client 跟随环境变量，south 强制 direct） | D3 已显式裁决；等价性测试记录差异；部署文档标注 |
| 4 | conformance 通过但证据是假接线 | P5 wiring review 为硬门禁，非走过场 |
| 5 | `[[test]]` 漏登记导致测试"永不运行且不报错" | P3/P4 验收以 CI 日志实际出现 target 为准 |
| 6 | south tag 前置阻塞（当前 feature 分支仍未 push） | P0 第一项就是它；企业侧开发期可先用 path/`[patch]` 并行推进 P2/P3 |
| 7 | 两仓行号/现状漂移（本方案基于 2026-08-17 快照） | 实施启动时复核 §2/§4 引用的位置 |

---

## 7. 明确不做（本纵切 scope 外）

- 非 embeddings 的任何调用点切换（chat、媒体、探活、center_pull）。
- Gemini embeddings 分支（`proxy_gemini_embeddings`）。
- streaming/SSE、multipart、GET、重试/failover、`ProviderTask`、WIT/Wasmtime runtime。
- 非纯 Bearer 认证供应商（`x-api-key` 族、自签 JWT、SigV4、OAuth 刷新族）。
- south 侧任何新功能（如 GET 支持、client 注入钩子）——若实施中发现确需 south 改动，停下来回到设计层面，不在 adapter 里绕。

---

## 8. 与既有文档的关系

- 本方案是 `2026-08-16-token-station-south-acceptance-checklist.md` §6（企业网关改造）的第一个落地切片，但接入点选择（embeddings）比该清单 §6.3 设想的 `post_json_attempt` 改造更收窄——调研确认 `post_json_with_retry*` 系列是被棘轮测试封印的将死 API，且 `post_json_attempt` 返回裸 `Response`（可流式），与 `ProviderCall` 的 buffered 契约不兼容，不作为替换目标。
- 企业仓 31 号计划（provider-crate G0/G1）与本方案正交：`gateway-provider-protocol` 拿走协议翻译（零 I/O），south 拿走 HTTP 传输（有 I/O），互不冲突；但 31 号的依赖纪律先例要求本方案在企业仓单独立项（P5.3）。
- 32 号计划（C4 sealed settlement）定义了本方案必须保住的资金不变式，P4 实施前必读。

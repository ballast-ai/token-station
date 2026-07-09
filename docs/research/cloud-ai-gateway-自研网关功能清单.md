# cloud_ai_gateway 功能清单（自研 LLM 路由网关分析）

> 仓库：https://github.com/second-state/cloud_ai_gateway
> 分析日期：2026-07-08
> 技术栈：Rust（axum + tokio + rusqlite），单静态二进制（内建 TLS、SVG→PNG 渲染、嵌入式字体），SQLite 存储
> 代码规模：主服务 117 个 Rust 文件（src/ 含 routes、proxy/translate、middleware、db、i18n 等子模块），外加独立子项目 profile-analyzer 与 WASM 插件体系

## 定位一句话

把多家上游 AI 提供商统一封装成 OpenAI / Anthropic / Gemini 兼容 API 的多协议路由网关，自带用户账户、鉴权、计费/配额、故障转移、WASM 可编程路由插件、请求日志与一套五语言 Web 控制台。

---

## 一、核心路由能力

### 1.1 上游 provider 支持（约 35 种 provider_type，`src/config.rs`）

按 wire 协议分四类：

| Wire 类型 | Provider |
|---|---|
| OpenAI 兼容 | OpenAI、GLM（Z.AI）、GLM Coding Plan（订阅）、Kimi Code、阿里云百炼/DashScope、BytePlus、DeepInfra、GitHub Copilot、Azure Foundry、Azure OpenAI、AWS Bedrock |
| Anthropic wire | Anthropic、Claude Code（CLI OAuth 订阅后端）、AWS Bedrock Claude、AWS Kiro |
| Responses wire | OpenAI Codex、AWS Bedrock OpenAI |
| 媒体专用（Other） | ElevenLabs（TTS）、Kling（视频）、MiniMax（图/视频/音乐）、Azure Speech、Ideogram（图）、Reve（图）、GMI Media、Stability（Stable Image）、Gemini native |

**订阅型后端接入是一个特色能力**：Claude Code / Codex / GitHub Copilot / Kiro / GLM Coding Plan 这类"订阅套餐"可作为上游凭证使用，带 OAuth token 自动刷新（`src/proxy/token_refresh.rs`）、Copilot 短期 token 铸造、AWS SigV4 签名 + eventstream 解析（`src/proxy/aws_sigv4.rs`、`aws_eventstream.rs`）。

### 1.2 模型路由与映射（`src/proxy/routing.rs`）

两层解析：

1. **routing_groups + routing_channels**（管理员配置的显式路由组）：请求 `model` 字段匹配逻辑名后，按组策略从候选 channel 中选一个上游模型，其余组成故障转移链
2. **回退到 legacy model_aliases**：别名重写 + 查表，无 channel 归因

**五种选路策略**：`single`、`weighted_random`（加权随机）、`round_robin`（原子计数器取模）、`lowest_latency`（按健康探测 p50 延迟）、`fallback`（带 `fallback_cooldown_secs` 熔断冷却）。

- **RPM 限流**：channel 级 `max_rpm`，内存 `RpmTracker` 判断，达上限跳到下一优先级
- 模型名替换：channel `target_name` → `ModelConfig.upstream_model` 写回请求体

### 1.3 Fallback / 重试（`src/proxy/retry.rs` + `dispatch.rs`）

- 可重试状态码：5xx、401、408、429；其他 4xx（400/403/404/422）不重试
- **凭证轮换**：每请求构建有序 AttemptPlan（legacy = 全部启用凭证按权重；routed = 主 channel + failover 链），循环到首个 2xx 或耗尽 `max_retries`
- **跨 provider dispatch**：沿 `[primary, ...failover]` 链派发，凭证按 provider 的 `credential_strategy` 从凭证池（`provider_pool.rs`）挑选
- **后台健康探测**（`health_probe.rs`）：按凭证写 `channel_health`（p50 延迟、错误率），供 lowest_latency 策略与 admin UI 使用

---

## 二、API 兼容层（三协议进，任意协议出）

### 2.1 端点清单（`src/routes/mod.rs`）

**通用（OpenAI 风格）端点**：

- 聊天：`POST /v1/chat/completions`、`POST /v1/responses`
- Anthropic：`POST /v1/messages`
- 模型列表：`GET /v1/models`
- 音频：`/v1/audio/transcriptions`、`/v1/audio/translations`、`/v1/audio/speech`
- 嵌入：`/v1/embeddings`
- 图像：`/v1/images/generations`、`/v1/images/edits`（azure/bailian/gemini/gmi/ideogram/minimax/stability 多后端）
- 视频：`/v1/video/generations`（网关内部轮询异步任务，一次调用进出）
- 音乐：`/v1/music/generations`、`/v1/minimax/music/generate`

**Provider 原生直通端点**：

- Gemini native：`/v1beta/models/{model_action}`（generateContent / streamGenerateContent）、operations 轮询（Veo 长任务）、Interactions API
- Kling 任务生命周期：text2video / image2video / omni-video / motion-control + task_id 轮询
- Reve 原生：`/v1/image/create|edit|remix`
- 百炼原生：videos/images generate + tasks 轮询
- ElevenLabs 原生 TTS：`/v1/text-to-speech/{voice_id}`

### 2.2 格式翻译（`src/proxy/translate/`）

入口支持 OpenAI / Anthropic / Gemini 三种格式，与上游 wire 不匹配时自动双向翻译（请求体 + 响应体 + 流式 SSE 逐帧改写）：

- OpenAI chat ↔ Anthropic messages
- OpenAI chat ↔ OpenAI Responses（Codex 及 `supports_responses=true` 上游）
- OpenAI responses ↔ Anthropic messages
- OpenAI chat ↔ Gemini generateContent
- OpenAI chat ↔ AWS Bedrock Converse
- OpenAI embeddings ↔ Gemini embeddings
- Kiro 专用翻译

流式驱动 `src/proxy/streaming.rs`（3364 行，SSE 逐帧改写与用量帧提取）。

**实测兼容主流 coding agent CLI 直连**（`tests/test_cli_agents.sh`）：Claude Code（Anthropic wire）、Codex（`wire_api="responses"`）、pi，可将 base_url 指向网关直接使用。

---

## 三、用户 / 鉴权 / 计费体系

### 3.1 鉴权（`src/middleware/auth.rs`）

- **网关 API key**：`Bearer gw_...`，SHA256 哈希存储查 `api_keys` 表；校验 key active、用户 activated、余额/信用池 > 0
- **用户会话**：`gw_session` cookie（magic-link 登录）
- **管理员会话**：`gw_admin_session` cookie

### 3.2 计费模式（Cargo feature 编译期切换）

| 模式 | 机制 |
|---|---|
| `billing-balance` | 每用户余额（微美元），Stripe Checkout 充值 + webhook，首充匹配奖金 |
| `billing-quota` | 无金钱，配额窗口限流（`limit_amount` / `window_seconds`），座位上限 `max_seats` |
| `billing-quota-credits` | 配额之上叠加全网系统信用池，admin 用 Stripe 充值 |

- **每用户 markup 加价倍率**（`users.markup`），计费时乘算
- **每用户模型白名单**（`user_allowed_models`）
- 配额执行（`src/quota.rs`）：窗口内已花费超限返回 429

### 3.3 Token 计量（`src/proxy/token_counter.rs`，4000+ 行）

- 分类用量：input / output / cached / cache_creation / cache_read 分桶，各有独立单价（缓存命中按上游折扣价计）
- 多模态计量：视频按秒、TTS 按字符、图片按张、Reve 按信用点；支持 price_tiers 阶梯定价
- **对账测试体系**（`tests/test_billing.sh`）：xAI 内联 cost ticks 透传比对、Kimi 账户余额快照 diff、OpenAI/Anthropic org 级 Admin API 日聚合交叉验证

---

## 四、WASM 插件中间件（WasmEdge 运行时，feature `wasm`）

可编程的请求改写 / 路由决策层，请求进上游前执行：

- **两级执行链**（`src/wasm_chain.rs`）：管理员全局插件 → API key 绑定插件；任一级返回新 model 则重新解析 provider
- **ABI**（`src/wasm_runner.rs`）：模块导出 `allocate(len)->ptr` 与 `run(body_ptr,body_len,model_ptr,model_len)->i64`（高 32 位指针/低 32 位长度），输出 JSON `{"model":"...","body":"<base64>"}`；上传时用 mock 请求校验
- **gateway_sdk**：`handler!` 宏封装 C ABI，插件作者只写 `fn(GatewayRequest) -> GatewayResponse`；沙箱无网络/DB 宿主接口，只能读请求、改路由/请求体
- 管理 UI 支持 `[[wasm_presets]]` 预设下拉与文件上传

**示例插件能力谱系**（`wasm_modules/`）：

1. `echo_wasm` — 透传骨架
2. `echo_redirect` — 无条件模型重定向（证明插件可覆盖客户端目标模型）
3. `complexity_router` — **基于内容的动态路由**：10 个加权维度打复杂度分（token 数 0.10、代码存在 0.15、推理标记 0.18、技术术语 0.15、简单指示减分 0.12、多步模式 0.08、问题数 0.05、数学逻辑 0.08、创意写作 0.06、system prompt 复杂度 0.03），按阈值路由到 4 档模型（SIMPLE/MEDIUM/COMPLEX/REASONING）；≥2 个推理标记强制 REASONING；JSON 解析失败回退原样透传

---

## 五、可观测性

- **三级请求日志**（`[logging] level`，`src/request_logging.rs`）：`none` / `usage`（默认，请求体 + 分桶用量）/ `full`（再加完整上游响应，流式含 SSE 原文不截断）；按 `request_logs/{user_id}/{api_key_id}/requests.jsonl` 落盘
- **DB 用量统计**（`usage_records`）：始终记录 token、成本、上游状态码、耗时、路由 channel 归因
- **管理控制台**（`src/routes/admin.rs`，3774 行）：overview、日志、用户/路由/provider/模型/信用管理、CSV 导出、图表

---

## 六、用户画像体系（两套）

### 6.1 内建 Builder Profile（`src/profile.rs`，确定性）

完全从 `usage_records` 聚合派生，零模型调用：主原型（Explorer/PowerBuilder/Architect/Generator…）、次要 Trait（NightOwl/BigSpender/Marathoner…）、6 维雷达评分、匿名 peer 百分位（≥5 人才显示）。配套 **OG 分享卡片**（`src/share_card.rs`）：1200×630 PNG，纯 Rust SVG→PNG（resvg/tiny-skia + 嵌入字体），数据最小化无 PII。

### 6.2 profile-analyzer（独立子项目，LLM 驱动深度画像）

离线管理员工具/cron 任务，只读网关 DB + JSONL 日志，产出自包含 HTML 报告 + 每用户知识库 Markdown。六阶段流水线：

1. **Redact**：纯正则 PII 脱敏（邮箱/API key/长 hex/信用卡/IP/电话 → 类型化占位符），先于一切模型调用，刻意高召回
2. **Embed**：批量走网关 `/v1/embeddings`，失败批二分收缩重试
3. **Cluster**：确定性 k-means（无 RNG、固定迭代，可复现），k≈√(n/2)，每簇取 medoids 用便宜模型生成短标签
4. **Extract**：逐交互结构化抽取（intent 11 类 / domain / 语言 / 实体 / 技能与挫败信号），full 与 medoids 采样两种模式
5. **Sessionize + Temporal**：30 分钟间隔切会话，窗口三等分比较意图漂移
6. **Synthesize + Verify**：强模型只看聚合信号（绝不看原始 prompt）产出带 confidence+evidence 的 Claim；每条 claim 走 **LLM judge ⊕ 词法重叠双重验证，词法可否决 judge**（杀幻觉）；定量数据由服务端盖章不可篡改

工程亮点：watermark 增量续跑、断点恢复、四阶段独立模型配置（extract/embed/synth/verify）、推理模型 temperature 400 自适应回退、全程 dogfood 网关自己的 API（专用 key 计费归因，且排除自身避免递归分析）。

---

## 七、产品化外围

- **Web 控制台**（服务端渲染，`src/templates/`）：用户 dashboard（keys/usage/playground/models/docs/billing/profile）+ 管理端全套页面
- **五语言 i18n**（`src/i18n/`）：en / zh-CN / zh-TW / ja / ko，locale 优先级 cookie > Accept-Language > en，CI 有翻译完整性测试
- **营销/静态页**（`assets/`）：intro / enterprise / router / terms / privacy 各语言版
- Resend 邮件（magic link 注册/登录，`[registration]` 控制开关）、Stripe 支付、Docker Compose 部署、TLS 内建、musl 静态编译发布

---

## 八、数据模型（migrations 0001–0057，17 张核心表）

| 表 | 用途 |
|---|---|
| `users` | 账户：email、激活态、余额、总花费、magic/session token、markup、归档 |
| `api_keys` | 网关 key：hash、prefix、active、绑定 wasm_file、软删除 |
| `usage_records` | 每请求用量：模型、endpoint、token、成本、上游状态、耗时、channel 归因 |
| `balance_transactions` | 余额流水（充值/扣费）、Stripe session、收据 |
| `user_quotas` / `user_allowed_models` | 配额窗口 / 模型白名单 |
| `provider_credentials` | 上游凭证池：api_key/oauth/aws_sigv4、权重、启用态、策略、分组 |
| `routing_groups` / `routing_channels` | 路由组（策略/failover/熔断）/ 候选上游（权重/RPM/优先级） |
| `providers` / `models` | 目录 + 完整定价（config 为权威源，启动时 `config_seed.rs` 同步进 DB） |
| `channel_health` | 每凭证健康：p50 延迟、错误率、最近错误 |
| `system_credit_transactions` | 系统信用池流水（quota-credits 模式） |
| `model_aliases` / `gateway_state` | legacy 别名 / 通用 KV 运行态 |
| `user_profiles` / `profile_runs` | LLM 画像结果与运行审计（0057 已 drop 重建，analyzer 现走文件 sidecar） |

注意：`sql/schema.sql` 仅是 0001–0010 快照，真实 schema 以 `db/migrations.rs` 内嵌迁移为准。

---

## 九、已知文档滞后（代码为准）

1. `wasm_modules/complexity_router/README.md` 的阈值（0.15/0.35/0.60）与模型名与实际代码（0.05/0.15/0.35 + gpt-5.4 系）不符
2. `wasm_modules/echo_wasm/README.md` 的 ABI 仍写着已废弃的 `provider` 参数（当前 SDK 只有 `model` 单参数）

---

## 十、对 token-station 的借鉴点（速记）

- **订阅套餐当上游凭证**（Claude Code/Codex/Copilot/GLM Coding Plan + OAuth 刷新）与 token-station 的核心业务模式直接对应
- 路由组五策略 + channel 级 RPM + 熔断冷却 + 健康探测，是"智能路由能力"文档可直接参照的成熟实现
- 三协议入口互译（OpenAI/Anthropic/Gemini）保证 coding agent CLI 零改造接入
- WASM 沙箱插件提供了"用户可编程路由"这条差异化路径（complexity_router 即按请求复杂度自动省钱）
- 编译期三种计费形态切换（余额/配额/信用池）对应不同商业模式的定价划分
- 分桶 token 计量（含 cache 折扣价）+ 上游对账测试，是计费可信度的工程底线

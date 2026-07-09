# 调研详细报告 · 层 A：计费 / 多租户网关

> 本文是 [开源token路由系统全景借鉴.md](../../../token-station/docs/research/开源token路由系统全景借鉴.md) 第一节的原始调研资料，保留全部事实细节与来源链接。调研日期 2026-07-05，所有 GitHub 数据经 API 实测。
> 覆盖：LiteLLM · Bifrost · Helicone ai-gateway · one-api/new-api 家族 · TensorZero · Portkey · Higress/Kong/Envoy AI Gateway。

---

## 一、BerriAI/litellm

**元数据**
- GitHub: https://github.com/BerriAI/litellm
- Stars: 52,618 | 语言: Python | 最近 push: 2026-07-05（当天，高度活跃，weekly stable release，v1.7x 系列）
- 许可证: 混合 —— 主体 MIT + `enterprise/` 目录商业许可（GitHub 显示 NOASSERTION）。LICENSE 原文规定 `enterprise/` 目录内容按 `enterprise/LICENSE.md`（BerriAI Enterprise License，需有效订阅才能生产使用），其余为 MIT。来源: https://github.com/BerriAI/litellm/blob/main/LICENSE

**Virtual Keys**（https://docs.litellm.ai/docs/proxy/virtual_keys）
- `POST /key/generate` 发放 `sk-...` 虚拟 key，需 Postgres（`DATABASE_URL`）+ master key
- key 级参数：`models`（模型白名单）、`max_budget`、`budget_duration`（如 `30d`，周期性重置）、`duration`（key 过期）、`tpm_limit`/`rpm_limit`、`max_parallel_requests`、`model_max_budget`（per-key-per-model 预算）、`model_rpm/tpm_limit`（per-key-per-model 限速）、`metadata`、tags、guardrails
- 支持 block/unblock；key rotation（`/key/{key}/regenerate`）标注为 Enterprise

**Budget / Rate limit 层级**（https://docs.litellm.ai/docs/proxy/users）
- 七个层级均可设 `max_budget` + `budget_duration` + tpm/rpm：proxy 全局、per-team、per-user（internal user，名下多 key 合计）、per-key、per-end-user（customer，请求体 `user` 字段识别终端用户，`/customer/new` 设预算）、per-model、per-team-member
- `budget_duration` 支持 `30s/30m/30h/30d`，到期自动重置。超限返回 `ExceededBudget`/`ExceededTokenBudget` 错误；支持 soft_budget（只告警不阻断）。per-customer 限速为 Enterprise。

**Team / Organization**（https://docs.litellm.ai/docs/proxy/self_serve）
- 层级：Organization → Team → User → Key；`/team/new` 可设 team 模型白名单、预算、限速、成员角色（admin/user），team admin 可自助管理
- Organizations 为 Enterprise 功能；角色体系含 proxy_admin / org_admin / internal_user 等

**路由策略**（https://docs.litellm.ai/docs/routing）
- `simple-shuffle`（默认，支持 deployment 级 `weight` 或 `rpm` 加权随机）
- `least-busy`（in-flight 最少）
- `usage-based-routing-v2`（TPM/RPM 用量最低者，推荐配 Redis，尊重 deployment 限额）
- `latency-based-routing`（最近延迟最低，可配 ttl、buffer）
- `cost-based-routing`（按价格表选成本最低 deployment）
- 另有 tag-based routing（Enterprise）与 provider budget routing（按 provider 周期预算路由）
- 多实例路由状态共享依赖 Redis（`router_settings`）

**Fallback / 容灾**（https://docs.litellm.ai/docs/proxy/reliability）
- `fallbacks`（跨 model group 兜底，请求级可覆盖）、`context_window_fallbacks`（超长专用）、`content_policy_fallbacks`（内容审查专用）
- 重试：`num_retries`、`retry_policy`（按错误类型细分次数）；冷却：`allowed_fails` + `cooldown_time`（失败超阈值的 deployment 暂时移出路由池）；`enable_pre_call_checks` 路由前过滤上下文不够的 deployment

**成本追踪**
- 每请求写 `LiteLLM_SpendLogs`（Postgres），`/spend/logs`、`/global/spend/report` 按 team/customer 聚合；成本基于仓库根目录 `model_prices_and_context_window.json` 价格表，自托管模型可自定义 `input/output_cost_per_token`（https://docs.litellm.ai/docs/proxy/custom_pricing）
- Prometheus `/metrics` 为 Enterprise；Langfuse/OTel/Datadog 回调免费

**部署依赖与性能**
- Postgres 必须（只要用 key/预算/spend）；多实例限速一致性需 Redis
- 官方 benchmark：单实例 4 vCPU 约 475 RPS、中位数 overhead ~40ms（https://docs.litellm.ai/docs/benchmarks）
- 已知痛点：Python 高并发 overhead、spend logs 表膨胀与 DB 写入瓶颈（官方 prod 文档建议 `proxy_batch_write_at` 批量落库、必要时 `disable_spend_logs`）、历史内存泄漏 issue（#5745 等）

**最值得借鉴**
1. **七层预算模型 + `budget_duration` 自动重置**：天然映射「订阅（周期重置额度）+ PAYG（不重置余额）」混合计费；per-end-user（customer）维度对应平台模式「客户的客户」。
2. **三类语义化 fallback 分离**：普通失败 / 上下文超长 / 内容审查各配兜底链 + 按错误类型细分 retry_policy——溢出决策应区分「容量型失败→溢出」与「能力型失败→换大窗口模型」。

---

## 二、maximhq/bifrost

**元数据**
- GitHub: https://github.com/maximhq/bifrost
- Stars: 6,263 | 语言: Go | 最近 push: 2026-07-05（当天，高度活跃）| 许可证: Apache-2.0
- 背景：由 Maxim AI（LLM 观测/评测平台商）主导，网关是获客入口；文档 https://docs.getbifrost.ai

**性能声称**
- "Adds only 11µs latency at 5,000 RPS"（README，t3.xlarge 自测中位数 overhead ~11-15µs）
- 对比 LiteLLM 营销数据："9.5x faster, ~54x lower P99, 68% less memory"（https://www.getmaxim.ai/blog/bifrost-vs-litellm-benchmark/，t3.medium 500 RPS；厂商自测、litellm 侧未调优，需打折看待）；benchmark 脚本公开于 https://github.com/maximhq/bifrost-benchmarking
- 架构因素：纯 Go、热路径无外部依赖、sync.Pool 对象复用；SQLite/Postgres 只做配置与日志异步落盘，不在请求关键路径

**Adaptive Load Balancing**（https://docs.getbifrost.ai/features/adaptive-load-balancing）
- 对同一模型的多 provider/多 key 维护实时健康分（错误率、延迟、429/5xx 限流信号加权），自动把流量迁离退化节点、恢复后自动回流；静态 weight 与动态调整叠加；与 fallback 链协同（先同模型多 key 均衡，全退化再走 fallback 模型链）

**Governance（开源，未锁企业墙）**（https://docs.getbifrost.ai/features/governance）
- Virtual Keys 绑定 provider/model 白名单；层级 **Customer → Team → VK** 三级，预算和限速可挂任意一级、逐级继承收紧
- 美元预算 + reset 周期（daily/weekly/monthly），超限 402/429 拒绝；内置价格表实时算成本（含 cache/batch 定价）；per-VK token/h、request/h 限速；内置用量统计 + Web UI

**容灾 / 集群**
- 请求级/配置级 fallback 链（model+provider 序列），per-provider `max_retries` + backoff
- **Cluster 模式：多实例间用 gossip 协议同步限速计数/健康分/预算消耗，不依赖中心 Redis**——但属企业版能力；开源多副本则各自计数 + DB 汇总

**部署**
- 单二进制/单容器，默认 SQLite 零依赖启动，可选 Postgres；内置 Next.js 控制台；插件体系（Go PreHook/PostHook：logging、Prometheus telemetry、governance、semantic_cache、guardrails、MCP gateway）
- 开源/付费边界：核心网关、路由、adaptive LB、governance、语义缓存、UI 全开源；企业版为 cluster mode、SSO/RBAC、审计、多集群治理

**最值得借鉴**
1. **健康分驱动的 adaptive LB**：连续健康分 + 自动回流，优于 one-api 式「失败 N 次禁用」二值开关，贴合「IDC 优先、渐进溢出」。
2. **计费不在请求关键路径**：内存额度判定扣减 + 异步批量落盘——验证「Redis 预扣 + CKafka 异步落账」路线，勿退回同步记账。

---

## 三、Helicone/ai-gateway

**元数据**
- GitHub: https://github.com/Helicone/ai-gateway
- Stars: 607 | 语言: Rust | 许可证: GPL-3.0 | 最近 push: **2025-11-21，已停滞约 7.5 个月**

**现状：实质已被放弃**
- README 已引导用户改用 Helicone 的云端 AI Gateway（托管、按请求计费）；官方文档 gateway 章节（https://docs.helicone.ai/gateway/overview）现描述的是 cloud 产品，self-host Rust gateway 文档移入 legacy；issue 区维护者表示自托管 Rust 网关不再是重点
- Helicone 主产品 https://github.com/Helicone/helicone（观测平台）仍活跃，商业重心在观测 + 托管网关
- **结论：不适合 2026 年选型**，仅设计参考

**曾提供的能力**
- 负载均衡：latency-based（**P2C + PeakEWMA**）、weighted；健康检查驱动的 provider 摘除
- 限流：**GCRA 算法**，per-API-key，可选 Redis 分布式
- 缓存：响应缓存（内存/Redis）
- **无内建计费**——成本/观测委托给 Helicone 云平台上报；网关本体只做路由/LB/限流/缓存
- 部署：单二进制、无强制外部依赖
- GPL-3.0 影响：自托管对外提供 SaaS 不触发源码公开义务（无 AGPL 网络条款）；私有化交付修改版二进制才需开源修改。实际风险在项目已死而非许可证

**最值得借鉴**
1. **P2C + PeakEWMA 负载均衡**：随机取二、选峰值指数加权延迟低者——O(1) 且天然惩罚慢节点，能应对推理延迟抖动。
2. **反面教训**：计费完全外包给云平台 → 自托管版价值单薄 → 被弃。计费/配额必须是自托管网关的一等公民能力。

---

## 四、one-api / new-api 家族

### 4.1 项目总览（GitHub API 实测，2026-07-05）

| 项目 | Stars | Forks | 许可证 | 最后 push | 状态 |
|---|---|---|---|---|---|
| songquanpeng/one-api | 35,499 | 6,714 | MIT | 2026-01-09 | 未归档，但已近 6 个月无 push，open issues 1,020 |
| QuantumNous/new-api | 41,110 | 9,501 | AGPL-3.0 | 2026-07-04（活跃） | 创建于 2023-11-10 |
| Veloera/Veloera | 1,620 | 150 | GPL-3.0 | 2026-02-11 | README 自述「不再被主动维护」 |
| yym68686/uni-api | 1,234 | 153 | Apache-2.0 | 2026-07-02（活跃） | Python |
| deanxv/done-hub | 786 | 141 | Apache-2.0 | 2026-07-05（当天有 push，活跃） | one-hub 二开 |
| MartialBE/one-hub | 2,860 | 494 | Apache-2.0 | 2026-02-19 | one-api 二开（repo 名 one-api） |
| VoAPI/VoAPI | 1,062 | 125 | Other/NOASSERTION | 2026-01-27 | 闭源二进制分发 |

### 4.2 one-api 核心机制（README 原文确认，https://github.com/songquanpeng/one-api）

- **定位**：「LLM API 管理 & 分发系统…统一 API 适配，可用于 key 管理与二次分发。单可执行文件，提供 Docker 镜像」。「通过标准的 OpenAI API 格式访问所有的大模型」。
- **渠道**：支持 OpenAI/Azure/Claude(含 AWS)/Gemini/文心/讯飞/智谱/通义/混元/Moonshot/DeepSeek/Ollama/Groq/Cohere/Cloudflare/DeepL/xAI 等数十种渠道类型；多渠道负载均衡；支持模型映射（重定向请求模型）；用户可在令牌后缀渠道 ID 指定渠道。
- **分组与倍率**：「支持用户分组以及渠道分组，支持为不同分组设置不同的倍率」。
- **计费公式**：「额度 = 分组倍率 × 模型倍率 ×（提示 token 数 + 补全 token 数 × 补全倍率）」；补全倍率跟随 OpenAI 官方定价（GPT-3.5 为 1.33，GPT-4 为 2）。支持以美元显示额度。
- **令牌**：「设置令牌的过期时间、额度、允许的 IP 范围以及允许的模型访问」；兑换码「支持批量生成和导出」，用于充值。
- **失败处理**：「失败自动重试」；环境变量 `CHANNEL_TEST_FREQUENCY`（定期检查渠道）、`CHANNEL_UPDATE_FREQUENCY`（定期更新渠道余额）、`ENABLE_METRIC`（根据请求成功率禁用渠道，默认关）、`METRIC_SUCCESS_RATE_THRESHOLD`（默认 0.8）、`METRIC_QUEUE_SIZE`（默认 10）、`POLLING_INTERVAL`。
- **多机部署**：统一 `SESSION_SECRET`、MySQL（非 SQLite）、从节点 `NODE_TYPE=slave`、可选 Redis。
- **许可证**：MIT，但要求页脚保留署名与项目链接。
- **维护状态**：未归档、无停止维护声明，但最后 push 2026-01-09，积压 issue 1,020——事实上低维护。

### 4.3 new-api 核心机制与增量（https://github.com/QuantumNous/new-api）

**额度单位与计费**
- 额度换算：**1 美元 = 500,000 quota**。按量计费：`配额消耗 = (输入token + 输出token × 补全倍率) × 模型倍率 × 分组倍率`。
- **按次计费**：`配额消耗 = 模型固定价格 × 分组倍率 × 500,000`（模型固定单价覆盖按量计费）。
- **缓存计费**：「按次、按量或缓存命中成本核算」，覆盖 OpenAI/Azure/DeepSeek/Claude/Qwen；Prompt Cache Ratio（缓存倍率）取值 0–1（如 0.5 = 缓存命中按 50% 计费），系统级或渠道级配置。
- **音频模型**另有音频倍率/音频补全倍率。默认倍率示例：gpt-4o 模型倍率 1.25、补全倍率 4；o1 模型倍率 7.5。
- FAQ：令牌额度与账户额度是两回事，「令牌额度仅用于设置最大使用上限」。

**渠道管理**
- 「渠道加权随机 + 失败自动重试（failover）」；重试次数在运营设置配置。
- 优先级/权重机制：同分组先走**优先级最高**渠道；同优先级内按**权重**加权随机；失败重试后才降级到低优先级。渠道状态三种：已启用 / 手动暂停 / 自动禁用。来源：https://linux.do/t/topic/200280 、https://github.com/QuantumNous/new-api/issues/3499
- 渠道级参数、令牌分组、令牌级模型限制、**用户级模型限速**。

**多租户/运营**
- 钱包：充值、兑换码、**AFF 邀请返利、余额划转**（https://doc.newapi.pro/en/guide/console/wallet/）
- **在线充值**：易支付（EPay）+ Stripe。登录：Discord、LinuxDO、Telegram、OIDC。可视化看板、多语言 UI。

**协议与模型面**
- OpenAI ⇄ Claude Messages 互转、OpenAI → Gemini 原生格式；Realtime API、Responses 格式；Reasoning Effort 配置（o3-mini/gpt-5/Claude thinking/Gemini thinking）。
- 扩展渠道：**Midjourney-Proxy、Suno API、Rerank（Cohere/Jina）**。
- 「与原版 One API 数据库完全兼容」。

**许可证变更时间线**（https://github.com/QuantumNous/new-api/commits/main/LICENSE）
- 2023-12 至 2024-05：多次脱离 one-api MIT 原文；**2025-07-20** 从 Apache 2.0 切换到现行许可；2026-01-26 再次更新。
- 当前 GitHub API 识别为 **AGPL-3.0** + 署名附加条款。**商用影响**：修改版以 SaaS 形式部署必须开源完整源码；不能接受 AGPL 可联系 support@quantumnous.com 购买商业授权。

### 4.4 后继 forks 差异定位

- **Veloera**（GPL-3.0，1.6k）：new-api 二开、数据库兼容；单渠道多 Key 随机、礼品码、**空回复不计费**、日志完整 token 统计。**已弃维护**（2026-02）。
- **uni-api**（Apache-2.0，1.2k，Python）：面向个人，**纯 YAML 配置默认无数据库**；负载均衡四种（渠道加权 / Vertex 多区域 / 顺序轮询 / 多 Key 轮询）；**失败自动重试 + 渠道冷却**；key 可设模型通配符权限、credits 上限。无充值/注册等商业化体系。
- **done-hub**（Apache-2.0，786，活跃）：one-hub（MartialBE 二开，2.9k star 已放缓）的活跃续作；主打**新型客户端反代（Claude Code / Gemini CLI / Codex / Antigravity）**、跨协议原生路由、动态 BaseURL 模板、邀请返利、多实例部署；数据库与原版兼容。
- **VoAPI**（1.1k，**闭源**二进制，禁商用）：高颜值 UI、JS 规则引擎、5 级用户体系、渠道熔断自动恢复、实时 RPM/TPM 监控；部署重（MySQL+Redis）。
- 生态周边：Calcium-Ion/new-api-horizon（高性能特性版）；all-api-hub、metapi 等聚合工具印证该家族的中转站市场格局。

### 4.5 关键结论

1. new-api 已在 star（41.1k vs 35.5k）和活跃度上全面超越 one-api；one-api 事实停滞。
2. 计费体系同一血统：分组倍率 × 模型倍率 ×（输入 + 输出×补全倍率），500,000 quota = $1；new-api 叠加按次计费、缓存倍率、音频倍率。
3. 许可证分叉点 2025-07-20（Apache 2.0 → AGPL-3.0）：自托管商用不受影响，修改后对外 SaaS 必须开源，闭源商用需购买授权。
4. fork 定位互补：Veloera（弃维护）、uni-api（个人向无 DB）、done-hub（CLI 客户端反代 + 返利运营）、VoAPI（闭源禁商用）。

---

## 五、TensorZero — 已停运（wind-down，非收购）

**归档事实**
- 仓库 `archived: true`，归档 2026-06-12；最后 push 2026-06-11；11,686 stars；Apache-2.0；393 open issues 无人处理。来源: https://github.com/tensorzero/tensorzero
- **停运原因**：CEO Gabriel Bianconi 在 HN 确认主动关停——「开源公司必须两次找到 PMF——一次 OSS 项目、一次商业产品」；$7.3M 种子轮（2025-08，FirstMark）只花了不到一半，**剩余资金退还投资人**，无负债无收购方。来源: https://news.ycombinator.com/item?id=48518120
- 官网声明："TensorZero remains available on GitHub but is no longer maintained"。Apache-2.0 可 fork，但无 provider 更新、无安全补丁。
- 背景：2026-01 ClickHouse 收购其最直接竞品 Langfuse，LLM 可观测赛道被数据基础设施厂商吃掉。来源: https://byteiota.com/tensorzero-shuts-down-what-oss-llmops-cant-survive/

**曾有的能力（归档前 README）**
- Rust 网关（<1ms p99）、ClickHouse 观测、adaptive A/B（variants）、routing/fallbacks/retries/LB、缓存 + dynamic in-context learning + best-of-N、评测（heuristics/LLM judge）
- **无多租户计费/virtual keys/配额**——自托管单租户 LLMOps 栈定位

**选型结论**：不可作为新项目选型；其「网关 + 实验闭环 + 观测一体」的架构设计仍可读。

---

## 六、Portkey-AI/gateway — 已被 Palo Alto Networks 收购

**收购事实**
- Palo Alto Networks **2026-04-30 宣布收购、2026-05-29 交割**，金额未披露，并入 Prisma AIRS 平台。来源: https://www.paloaltonetworks.com/company/press/2026/palo-alto-networks-completes-acquisition-of-portkey-to-secure-ai-agents
- 活跃度：最后 push 2026-05-25（交割前 4 天）；2026-06-25 至 07-03 新开 20 个 issue/PR 中 19 个零回复（含一个 `/v1/proxy/*` SSRF 安全披露 #1718 无人响应）。仓库未归档：12,311 stars，MIT，212 open issues。

**开源 vs 托管边界（2026-03 起大幅移动）**
- **Gateway 2.0（2026-03-24）把原企业版核心开源**："fully open source, no license keys"：Model Catalog（250+ 模型含定价）、**Usage Policies（请求/token/成本级限额，入口强制执行）**、Circuit Breakers（按 P99 延迟或错误率熔断 + 探测恢复）、MCP Registry + OAuth 2.1、实时成本/延迟/用量指标、semantic caching 与 budget controls。来源: https://portkey.ai/blog/gateway-2-0/
- 注意：main 分支 README 标注 Gateway 2.0 为 **Pre-Release**；main 仍是 v1 形态。
- 仍闭源/付费：On-Prem Enterprise 的 gRPC、SSO、SCIM、KMS、RBAC、审计、多 workspace、合规包；托管平台日志存储 + dashboard。

**Config-based routing（开源，v1 即有）**
- JSON config，`strategy.mode`：`single` / `loadbalance` / `fallback` / `conditional`；`targets` 数组**递归嵌套**（loadbalance 套 fallback）
- `weight`、`override_params`、`default_params`；`retry`: attempts（≤5，指数退避）+ `on_status_codes` + `use_retry_after_headers`；`request_timeout`
- Canary：用 loadbalance 权重实现（95/5 分流 + override_params 换模型）
- **Conditional routing**：条件来源 `metadata.*` / 请求参数 / `url.pathname`；操作符 `$eq $ne $in $nin $regex $gt $gte $lt $lte`，逻辑 `$and $or` 可嵌套。来源: https://portkey.ai/docs/product/ai-gateway/conditional-routing

**Guardrails 与 Cache**
- 40-50+ 预置 guardrails（含 CrowdStrike AIDR 插件）
- Simple cache（精确匹配）全计划可用；Semantic cache 自托管开源版可用——需自配 embedding provider + 向量库；TTL 60s–90d，支持 force refresh 与 namespace

**部署形态**
- TypeScript，~122KB，宣称 <1ms overhead；`npx @portkey-ai/gateway`、Docker、**Cloudflare Workers（edge）**、K8s 等；v1 核心无状态、无数据库依赖

**选型结论**：功能上 OSS（尤其 2.0 的 usage policies/熔断）有存活基础，但收购后 5 周无 commit、安全披露无人回应、2.0 停在 pre-release——开源版在 Palo Alto 体系下的维护承诺无官方声明，重大不确定。

---

## 七、基础设施层 AI 网关：Higress / Kong / Envoy

### 7.1 Higress（higress-group/higress）— 重点

**AI 网关能力**
- **ai-proxy 插件**：OpenAI API 契约统一代理，30+ providers（openai/azure/qwen/deepseek/claude/gemini/vertex/bedrock/ollama/groq/openrouter/zhipuai/hunyuan/spark…）；OpenAI 与 Claude 协议自动检测、双向转换（v2.1.6 起同时暴露 `/v1/chat/completions` 和 `/v1/messages`）。来源: https://higress.cn/docs/latest/plugins/ai/api-provider/ai-proxy/
- **ai-token-ratelimit**：基于 Redis 的全局 token 限流。限流 key 支持 URL 参数、请求头、客户端 IP、**consumer 名**、cookie，精确/正则/通配 per-key 模式；窗口 `token_per_second/minute/hour/day`；超限默认 429（可配）；计量依赖 ai-statistics。来源: https://higress.cn/en/docs/latest/plugins/ai/api-consumer/ai-token-ratelimit/
- **ai-quota**：配额存 Redis（前缀 `chat_quota:`），按 consumer 记账，与 key-auth/jwt-auth 联动。**自带管理 API**：`/quota/refresh`（充值/重置）、`/quota`（查询）、`/quota/delta`（增减，支持负数），`admin_consumer` 鉴权。**无自动周期重置**——需外部定时调 refresh。来源: https://higress.cn/en/docs/latest/plugins/ai/api-consumer/ai-quota/
- **ai-statistics**：token 用量统计，是 quota 与 ratelimit 的计量基础。
- **ai-cache**：LLM 结果缓存，流式/非流式；精确缓存 Redis KV，语义缓存对接向量库（DashVector 等）。
- **ai-security-guard**：对接**阿里云内容安全付费服务**。
- **模型灰度 / fallback**：AI 路由层能力——按比例模型灰度（90% openai / 10% deepseek）、5xx 自动切备用模型；ai-proxy modelMapping 支持模型名映射。来源: https://higress.ai/blog/

**多租户/consumer 体系**
- key-auth consumer + ai-quota = per-consumer token 总量配额；+ ai-token-ratelimit（`limit_by_consumer`）= per-consumer 速率限制；两者叠加：配额管总量、限流管速率。

**定位与部署**
- 2.x 定位 "AI Native API Gateway"：100+ 模型、API Key 池轮转、语义缓存、护栏、模型 LB 与 fallback、HTTP-to-MCP；自带控制台（:8001）。
- 内核 Envoy + Istio（阿里生产验证）；**standalone all-in-one Docker 单容器**（免 K8s）或 K8s Helm。

### 7.2 Kong AI Gateway（简要）
- Kong 3.6 起开源版含 6 个 AI 插件：ai-proxy、ai-request/response-transformer、ai-prompt-guard/template/decorator。
- **ai-proxy-advanced（多目标 LB、语义路由）和 ai-rate-limiting-advanced（token 级限流）均为企业版**——开源版只能按请求数限流。来源: https://developer.konghq.com/ai-gateway/
- 结论：开源 Kong 无 token 级限流/配额，与按 token 计费需求不匹配。

### 7.3 envoyproxy/ai-gateway（简要）
- 构建在 Envoy Gateway + K8s Gateway API 上（AIGatewayRoute + InferencePool CRD），两层网关模式，**强绑定 K8s**。
- 能力：token 感知限流、token Quota Policy、Provider Fallback、Upstream Auth；providers 覆盖主流。
- 成熟度：**v1.0.0 于 2026-06-23 发布**，刚到 1.0；无控制台，纯 CRD/YAML。来源: https://github.com/envoyproxy/ai-gateway/releases

### 7.4 三者对比结论
- 只有 Higress 同时满足：开源免费 token 限流 + 配额管理（带充值 API）+ 免 K8s standalone 部署 + 控制台。Kong token 限流付费；Envoy AI Gateway 功能对口但必须上 K8s 且刚 1.0。
- Higress 两个注意点：ai-quota 无自动周期重置；ai-security-guard 依赖阿里云付费服务。

---

## 八、本层横向对比

| 维度 | LiteLLM | Bifrost | Helicone GW | new-api | Portkey | Higress |
|---|---|---|---|---|---|---|
| 状态 | 高度活跃 | 高度活跃 | 弃维护 | 高度活跃 | 收购后不明 | 高度活跃 |
| 计费深度 | 7 层预算+周期重置+spend logs | 3 层美元预算+重置周期 | 无 | 倍率体系+充值/返利 | Usage Policies（2.0） | 插件级配额+限流 |
| 路由策略 | 5 种+加权 | 健康分 adaptive | P2C+PeakEWMA | 优先级+权重随机 | 4 模式递归嵌套 | 灰度+fallback |
| 容灾 | 3 类 fallback+retry_policy+冷却 | fallback 链+摘除回流 | 健康摘除 | 失败重试+自动禁用 | 熔断器（2.0） | 5xx 切换 |
| 多租户 | Org→Team→User→Key→Customer | Customer→Team→VK | 无 | 用户/分组/令牌 | workspace（企业） | consumer 体系 |
| 部署依赖 | Postgres 必须+Redis | 单二进制 SQLite | 单二进制 | MySQL/SQLite | 无状态 edge 可跑 | Docker 单容器/K8s |
| 许可证 | MIT+企业目录 | Apache-2.0 | GPL-3.0 | **AGPL-3.0** | MIT | Apache-2.0 |

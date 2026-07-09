# 闭源商业 Token 服务功能面调研：OpenRouter / Portkey / Helicone 等能借鉴什么

> 场景：token-station 定位「统一 token API 服务 + 智能路由器」，此前 research 深挖的是**开源自托管网关**（LiteLLM / new-api / Bifrost / vLLM semantic-router）。本文补齐另一侧——**闭源商业服务**的用户可见功能面，作为「完整 token API 服务面」功能想象力的来源。
> 核心结论：**闭源商业服务把「路由」做成了产品的一等能力（OpenRouter 的 provider routing 有 12+ 可配置维度、Presets 把整套路由策略服务端集中管理），并普遍把「治理/访问控制/支出控制」和「内容 Guardrails」分成两套东西。OpenRouter 的 BYOK 分成模型（5% 抽成、可配 always-use / fallback 三态）、Presets 配置外置、Workspaces 多租户，是 token-station 三种服务模式最直接的产品参照。Portkey 补齐 Gateway 之外的 observability / prompt studio / guardrails 三件套形态。**
>
> 调研日期 2026-07-05。所有功能均来自各产品官方 docs / pricing 页实测。配套：[开源token路由系统全景借鉴.md](./开源token路由系统全景借鉴.md)（开源侧）、[../features/token-station功能需求清单.md](../features/token-station功能需求清单.md)（本文的落点）。

---

## 零、闭源 vs 开源两侧的功能重心差异

| 维度 | 开源自托管（LiteLLM/new-api/Bifrost） | 闭源商业（OpenRouter/Portkey/Helicone） |
|------|-----------------------------------|--------------------------------------|
| 计费重心 | **运营侧**：倍率、充值、兑换码、渠道分组（new-api 把它做成一门转售生意） | **消费侧**：credits 充值、BYOK 抽成、workspace 预算、发票 |
| 路由重心 | 负载均衡 + fallback + 健康分（供给侧为主） | **provider routing 精细化**（12+ 维度）+ Auto Router（质量侧）+ Presets 配置外置 |
| 治理形态 | virtual key + 七层预算（LiteLLM） | 治理/访问控制与内容 guardrail **分离成两套**（OpenRouter 尤其明显） |
| 可观测 | 日志 + 用量统计 + 仪表盘 | observability 做成**独立产品线**（Helicone / Portkey 各自的 traces/analytics） |
| 交付 | 单二进制自部署、DB 依赖 | SaaS + Enterprise（SSO / ZDR / SLA / 私有部署） |

**对 token-station 的意义**：开源侧告诉我们「一个网关产品的功能骨架」，闭源侧告诉我们「路由这件事能做到多精细、以及如何把路由/治理/观测包装成可售卖的产品能力」。

---

## 一、OpenRouter：路由作为一等产品能力的标杆

OpenRouter 是「统一 API 聚合 + 智能路由」形态与 token-station 定位最接近的商业产品，逐块拆解。

### 1.1 Provider Routing —— 12+ 维度的供给侧精细路由

同一个模型可能有多家 provider 提供，OpenRouter 让用户对「这次请求派给哪个 provider」有极细的控制（[provider-selection 文档](https://openrouter.ai/docs/guides/routing/provider-selection)）：

| 配置项 | 作用 |
|--------|------|
| `order` | provider 偏好排序，按列表顺序逐个尝试；**设了就关闭负载均衡** |
| `allow_fallbacks`（默认 true） | 主 provider 失败时是否允许回退到备用 provider；设 false 则只用指定的 |
| `only` | provider 白名单，只用列出的 |
| `ignore` | provider 黑名单，排除列出的 |
| `require_parameters`（默认 false） | 只路由到**支持本请求全部参数**的 provider（如 JSON 格式、response schema） |
| `sort` | 按 `price` / `throughput` / `latency` 排序，覆盖默认负载均衡；也可用模型后缀 `:nitro`(吞吐) / `:floor`(价格) |
| `preferred_min_throughput` / `preferred_max_latency` | 用**百分位阈值**（p50/p75/p90/p99）**降权**（而非硬屏蔽）表现差的端点 |
| `data_collection`（deny/allow） | 排除会**非瞬态存储用户数据**（用于训练）的 provider |
| `zdr`（true） | 只路由到 **Zero Data Retention** 端点 |
| `enforce_distillable_text` | 只路由到「作者允许文本蒸馏用于训练」的模型 |
| `max_price` | 屏蔽超过指定 per-token 单价的 provider，如 `{"prompt":1,"completion":2}` |
| `quantizations` | 按量化精度过滤开源模型（int4/int8/fp8/fp16/bf16/fp32…） |

**默认策略**：无自定义时，对稳定 provider 按「**价格平方倒数**」加权，并规避近期宕机的端点——兼顾成本与可靠性。

> **对 token-station 的直接价值**：这套 schema 就是「供给侧路由」（[OmniRoute 分析](./omniroute-开源系统借鉴分析.md) 讲的那一层）的成熟商业范本。尤其 `data_collection`/`zdr`/`quantizations` 三项——**路由决策直接消费隐私与精度约束**，正好对接 token-station「精度真实性标注」和三种模式的隐私梯度。`quantizations` 过滤更是自营 GLM-5.2「锁 fp8」溢出语义的通用化表达。

### 1.2 Auto Router —— 质量侧路由（Not Diamond 驱动）

`openrouter/auto` 端点按 query 内容自动选模型，底层是 Not Diamond（闭源）。这属于 token-station 四层路由的第 ③ 档（学习型），已在 [智能路由能力.md](../features/glm-5.2-智能路由能力.md) 覆盖，此处不展开。**闭源不可借鉴其实现，只印证「Auto Router 是可售卖的产品形态」。**

### 1.3 Presets —— 路由策略的服务端集中管理

[Presets](https://openrouter.ai/docs/guides/features/presets) 把「模型选择 + 系统 prompt + 生成参数 + provider 路由偏好」打包成一个命名配置，客户端用 `@preset/email-copywriter` 一个别名引用整套策略：

- 在仪表盘创建/改 preset，**客户端代码不动**即可切模型、改 prompt、调路由偏好——「应用代码与 LLM 配置解耦」；
- provider routing 偏好被封装在 preset 内部。

> **对 token-station 的直接价值**：这是「用户自定义路由规则」（四层路由第 ① 层）**最好的产品化交付形态**——规则不是散在客户端 header 里，而是服务端集中管理的命名配置。同时天然是套餐差异化载体（不同套餐对同一 preset 别名解析出不同档位）。与 plano 的「模型语义别名 fast/smart/cheap」是同一思路的两种粒度。

### 1.4 BYOK —— 自带 provider key 的分成模型

OpenRouter 的 [BYOK](https://openrouter.ai/docs/use-cases/byok) 是 token-station「个人/企业模式（BYOK 直连）」的最直接商业参照：

- **抽成**：用自带 key 收「相当于该模型在 OpenRouter 正常价 **5%**」的服务费，从 credits 扣；**每月前 100 万 BYOK 请求免抽成**；
- **三态 fallback 控制**（关键设计）：
  - 默认：优先用你的 key，遇限流/失败**回退到平台共享 credits**；
  - `Always use this key`：只用你的 key，耗尽就报错，**保证所有请求走你的账户**（不泄漏到平台额度）；
  - `Use this key as a fallback`：优先用平台 credits，失败再用你的 key；
- 支持 **60+ 推理 provider**；BYOK 端点在有 key 时自动优先。

> **对 token-station 的直接价值**：`Always use this key` 这个开关正是 token-station 个人 BYOK 模式「内容不经平台」承诺（L2 隐私）的产品旋钮——**用户可显式关闭平台 fallback，把数据流锁死在自己账户内**。抽成模型（对 BYOK 收小额服务费而非赚差价）也是 BYOK 模式的可行商业化路径参照。

### 1.5 Workspaces & 组织管理 —— 多租户

[Workspaces](https://openrouter.ai/docs/guides/features/workspaces) 把项目/团队/agent 隔离成独立环境（[组织管理](https://openrouter.ai/docs/guides/administration/organization-management)）：

- 每个 workspace 装自己的 API keys、guardrails、BYOK provider keys、routing policies、presets、plugins、observability 集成；
- **成员与角色**：org admin 跨所有 workspace 有管理权，成员可属于多个 workspace、在所属 workspace 里自建 key；
- **API key provisioning**：org admin 可建「workspace 拥有」而非「个人拥有」的 system key；management key 在账户级、经 management API 跨 workspace 操作；
- **支出控制**：每 workspace 可设 daily/weekly/monthly/lifetime 支出上限（Enterprise），每 workspace 最多 4 个预算（每种周期一个）；
- **组织级**：共享 credit 池集中计费、RBAC（Admin/Member）、组织级 activity 追踪。

> **对 token-station 的直接价值**：workspace = token-station「小 Team / 企业模式」的租户单元；「每 workspace 独立装 routing policies + presets + BYOK key」正是三种模式「不共用同一套路由器」约束的商业实现——**路由配置绑定在租户边界内**。

### 1.6 Guardrails —— 治理 = 支出与访问控制（非内容审查）

OpenRouter 特意把 [Guardrails](https://openrouter.ai/docs/guides/features/guardrails/overview) 定义为「**Organization Spending and Access Controls**」——即模型/provider 访问白黑名单、支出限制、数据策略（zdr/data_collection）强制、prompt 注入检测，按 key/workspace 强制。

> **对 token-station 的启示**：**「治理 guardrail」（谁能用什么、花多少、数据能不能留）和「内容 guardrail」（PII/审查/越狱）是两件事**，OpenRouter 把前者叫 Guardrails。token-station 的功能清单要把这两类分开建模，别混在一个「安全」大筐里。

### 1.7 其他产品化细节

- **成本透明**：activity 页 + 逐请求成本；`:nitro`/`:floor` 让用户显式选「贵但快 / 慢但便宜」；
- **Prompt Caching**：按模型能力自动启用，Anthropic 的 tool call 也支持缓存（passthrough 上游缓存能力，不自建）；
- **模型库**：400+ 模型统一目录，带各 provider 的价格/上下文/吞吐对比。

---

## 二、Portkey：Gateway + Observability + Prompt + Guardrails 四件套

Portkey 代表「AI Gateway 做成企业中台」的形态，其功能面按四个产品线组织，对 token-station 的价值在于**功能域的划分方式**。

### 2.1 AI Gateway（[文档](https://portkey.ai/docs/product/ai-gateway)）

Universal API、Fallbacks、Load Balancing（多 key 分摊限流）、**Conditional Routing**（按自定义条件路由到不同 target）、Automatic Retries、Request Timeout、**Circuit Breaker**、**Canary Testing**（生产环境灰度测新模型）、Budget & Rate Limits（按成本/token、按 时/日/分 限流）、Multimodality、MCP Support、Custom Hosts（路由到私有/本地模型）、gRPC(Beta)。

> **可借鉴**：`Conditional Routing`（条件表达式路由）+ `Canary Testing`（新模型灰度）是 token-station 路由层值得抄的两个能力——前者是「用户自定义规则」的条件引擎，后者是「上新模型时先给 5% 流量验证再全量」的安全上线机制。

### 2.2 Observability（[文档](https://portkey.ai/docs/product/observability)）

Logs（全多模态请求/响应）、Tracing（全生命周期）、Analytics（21+ 指标）、Filters、**Custom Metadata**（自定义标签分组排障）、Feedback（反馈值+权重闭环）、Budget Limits。

> **可借鉴**：`Custom Metadata`（用户给请求打自定义标签，日志按标签聚合）+ `Feedback`（用户对某次响应打反馈分）——后者正是 token-station 数据飞轮「用元数据/反馈信号训路由分类器」的采集入口。

### 2.3 Prompt Engineering Studio（[文档](https://portkey.ai/docs/product/prompt-engineering-studio)）

Prompt Templates（变量动态提示）、版本管理（每次改生成新版本、可回退）、部署（Completions/Render 端点）、Playground（跨模型测试的 prompt IDE）、Variables、Prompt Partials（可复用片段）、A/B 对比（1600+ 模型/prompt 并排对比）。

> **对 token-station 的取舍**：prompt 管理属于「LLMOps 平台」范畴，**不是路由器核心**，列为可选/差异化，不进 MVP。

### 2.4 Guardrails（[文档](https://portkey.ai/docs/product/guardrails)）—— 内容侧

Input/Output Guardrails（发给 LLM 前 / 输出后）、PII 检测与脱敏（20+ 确定性 guardrail）、内容审查、越狱/prompt 注入检测、Regex 匹配、JSON Schema 校验、第三方集成（Aporia/SydeLabs/Pillar Security）、**Guardrail Actions**（Deny 拦截 / Async 异步记录 / Sequential 顺序 / Fallback 切模型 / Retry / Feedback）。

> **对 token-station 的取舍**：内容 guardrail 与 vLLM semantic-router 内置的 PII/越狱检测重叠（见 [全景借鉴](./开源token路由系统全景借鉴.md) §2.2）——**优先复用开源方案**，Portkey 印证「Guardrail Actions 里 `Fallback 切模型` 应与路由层打通」（审查不过 → 走内容策略 fallback 链，和 LiteLLM 的 `content_policy_fallbacks` 一致）。

---

## 三、Helicone 及其他：可观测性作为独立入口

- **Helicone**：以 observability 为主（一行代理接入、日志、追踪、缓存、评估、用户级用量），代表「先用免费可观测性获客、再向网关/计费延伸」的路径。其**缓存和 user-level 用量分析**可借鉴，但 token-station 不把 observability 做成独立产品线，只做「路由透明 + 用量账单」够用的部分。
- **Vercel AI Gateway / Cloudflare AI Gateway**：都主打「一个端点接多模型 + 缓存 + 可观测 + 限流」，Cloudflare 版免费缓存/日志、Vercel 版绑其 AI SDK。形态与 token-station 重叠但深度不足，**只作「轻量网关也能成立」的市场佐证**，不逐项借鉴。

---

## 四、汇总：闭源侧借鉴落点表

| 借鉴点 | 来源 | 落入 token-station 的功能域 |
|--------|------|--------------------------|
| provider routing 12 维 schema（order/only/ignore/sort/max_price/require_parameters） | OpenRouter | 供给侧路由配置 |
| `data_collection`/`zdr`/`quantizations` 路由约束 | OpenRouter | 供给侧路由 + 隐私/精度标注 |
| Presets：路由策略服务端集中管理、别名引用 | OpenRouter | 用户自定义规则（第 ① 层）的交付形态 |
| BYOK `Always use this key` 三态开关 + 5% 抽成 | OpenRouter | 个人/企业 BYOK 模式的隐私旋钮 + 商业化 |
| Workspaces：租户隔离，每租户独立装 routing/preset/BYOK | OpenRouter | 小 Team/企业模式多租户 |
| 治理 guardrail ≠ 内容 guardrail（分两套） | OpenRouter | 功能域划分原则 |
| Conditional Routing（条件表达式） | Portkey | 用户自定义规则条件引擎 |
| Canary Testing（新模型灰度上线） | Portkey | 路由/模型运营 |
| Custom Metadata + Feedback 采集 | Portkey | 可观测性 + 数据飞轮采集入口 |
| Guardrail Action `Fallback 切模型` 与路由打通 | Portkey | 内容策略 fallback 链 |

## 五、明确不借鉴 / 不进 MVP 清单

| 项 | 原因 |
|----|------|
| Auto Router / Conditional Routing 的**闭源实现** | 均闭源，只借形态；实现走开源（vLLM SR / 规则引擎） |
| Portkey Prompt Studio 全套 | LLMOps 范畴，非路由器核心，可选/差异化 |
| Helicone 式独立 observability 产品线 | token-station 只做「路由透明 + 账单用量」，不做通用 LLM 可观测平台 |
| 各家的内容 guardrail 自研 | 与 vLLM SR 内置能力重叠，优先复用开源 |

---

## 参考

- [OpenRouter Provider Routing](https://openrouter.ai/docs/guides/routing/provider-selection) · [Presets](https://openrouter.ai/docs/guides/features/presets) · [BYOK](https://openrouter.ai/docs/use-cases/byok) · [Workspaces](https://openrouter.ai/docs/guides/features/workspaces) · [组织管理](https://openrouter.ai/docs/guides/administration/organization-management) · [Guardrails（支出/访问控制）](https://openrouter.ai/docs/guides/features/guardrails/overview) · [Enterprise Quickstart](https://openrouter.ai/docs/enterprise-quickstart) · [模型路由机制博客](https://openrouter.ai/blog/insights/model-routing/)
- [Portkey AI Gateway](https://portkey.ai/docs/product/ai-gateway) · [Observability](https://portkey.ai/docs/product/observability) · [Prompt Engineering Studio](https://portkey.ai/docs/product/prompt-engineering-studio) · [Guardrails](https://portkey.ai/docs/product/guardrails)
- Helicone · Vercel AI Gateway · Cloudflare AI Gateway（形态佐证，未逐项引用）

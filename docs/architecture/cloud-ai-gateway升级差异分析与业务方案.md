# cloud_ai_gateway → token-station 差异分析与升级业务方案

> 场景：以现有自研网关 [cloud_ai_gateway](https://github.com/second-state/cloud_ai_gateway)（能力盘点见 [研究文档](../research/cloud-ai-gateway-自研网关功能清单.md)）为基座，对照 token-station 已收口的需求（[功能需求清单](../features/token-station功能需求清单.md)、[业务模式与定价划分](../features/业务模式与定价划分.md)、[智能路由能力](../features/智能路由能力.md)、[路由透明与用户信任](../features/路由透明与用户信任.md)、[个人模式本地客户端需求](../features/个人模式本地客户端需求.md)/[概要设计](./个人模式本地客户端概要设计.md)），回答两个问题：**差在哪、怎么升**。
>
> 整理日期 2026-07-08。

---

## 零、一句话结论

cloud_ai_gateway 已经把 token-station **M1 门面层做完、M2 供给侧做了一半、M3 计费做了六成**——协议互译、35 种上游、五策略路由、fallback/熔断/健康探测、分桶计量计费都是现成的，起点远高于从零开始。但 token-station 的**灵魂能力恰好都缺**：质量侧智能路由（四层框架）约缺 90%，路由透明三件套完全没有，BYOK 不存在，多租户治理只有单管理员密码。更关键的是有**六个结构性差距**不是加功能能补的，涉及配置权下放、计费机制运行时化、多形态交付等架构级改造。

**升级策略**：网关升级为 token-station 的**服务端内核**（同时承担交付物②私有部署网关与③平台 SaaS，同一代码库、license 开关分档），路由引擎抽成独立 crate 供交付物①本地客户端复用。不重写，分四期改造。

---

## 一、能力对照总表（按功能需求清单十一个域）

图例：✅ 已有可直接用 · ◐ 部分有需改造 · ❌ 缺失需新建。「等级」列沿用需求清单的 🔴🟡🟢。

### §一 统一 API 与协议兼容层 —— 完成度 ~90%，M1 基本白拿

| 需求点 | 等级 | 现状 | 说明 |
|---|---|---|---|
| OpenAI 兼容端点（含流式） | 🔴 | ✅ | `/v1/chat/completions`、`/v1/responses` |
| Anthropic Messages 端点（接 Claude Code） | 🔴 | ✅ | `/v1/messages`，且实测过 Claude Code/Codex/pi CLI 直连 |
| Gemini 原生端点 | 🟡 | ✅ | `/v1beta/models/*` generateContent + operations |
| 跨格式互译 OpenAI⇄Claude⇄Gemini | 🟡 | ✅ | translate 层齐全，含 SSE 逐帧改写、Bedrock Converse |
| Embeddings / 多模态端点 | 🟢 | ✅ | images/audio/video/music 全有——超出需求 |
| `/v1/models` 模型目录 | 🔴 | ✅ | |
| 模型名后缀控推理强度（`-thinking`/`-high`） | 🟡 | ❌ | 需在模型解析层加后缀语法 |
| SDK + OpenAI SDK 直接指向 | 🔴 | ◐ | base_url 指向即用已验证；缺打包好的 Python/JS SDK 与接入文档产品化 |
| MCP / 工具调用 passthrough | 🟢 | ◐ | tool 调用随协议透传已有，MCP 未专门处理 |

### §二 模型/供应商聚合 + BYOK —— 聚合完成度 ~85%，**BYOK 为 0**

| 需求点 | 等级 | 现状 | 说明 |
|---|---|---|---|
| 多供应商渠道接入 | 🔴 | ✅ | 35 种 provider_type，含订阅型上游（超出需求，见 §三资产盘点） |
| 模型统一目录（价格/上下文对比） | 🔴 | ◐ | models 表有完整定价，缺面向用户的选型对比页 |
| 模型语义别名（fast/smart/cheap） | 🟡 | ◐ | model_aliases 是管理员全局别名，缺「按套餐/租户解析档位」 |
| **BYOK 自带 key 直连** | 🔴 | ❌ | **结构性缺失**：provider_credentials 是平台全局凭证池，无用户归属维度 |
| BYOK 三态 fallback 开关 | 🟡 | ❌ | 依赖 BYOK 先落地 |
| 渠道分组/优先级/权重/标签 | 🔴 | ✅ | routing_channels + 凭证分组 |
| 渠道测活/自动禁用/自动恢复 | 🔴 | ✅ | health_probe 循环 + fallback 冷却 |

### §三 智能路由（质量侧）—— **完成度 ~10%，最大差距**

| 需求点 | 等级 | 现状 | 说明 |
|---|---|---|---|
| ① 用户自定义声明式规则 | 🔴 | ❌ | 唯一近似物是 API key 绑定 WASM（要求用户写 Rust 编译，不是产品）|
| 规则服务端集中管理（Preset） | 🟡 | ❌ | |
| ② 启发式复杂度打分 | 🔴 | ◐ | complexity_router WASM 已实现 10 维打分算法，但是插件 demo 不是内建层 |
| ③ 学习型分类器路由 | 🟡 | ❌ | |
| ④ 级联兜底 | 🟢 | ❌ | |
| Auto Router 一键端点 | 🟡 | ❌ | |
| 单阈值成本旋钮（套餐参数） | 🟡 | ❌ | |
| 客户端 hint header 消费 | 🔴* | ❌ | *四层框架第 2 层，本地客户端概要设计已依赖它 |

> 现网关的路由回答的是「**这个模型由谁来服务**」（供给侧），token-station 要补的是「**这个请求该用哪个模型**」（质量侧）。前者是管理员视角的运维配置，后者是用户视角的产品能力——这不是同一层东西，不能混用现有 routing_groups 硬扛。

### §四 供给侧路由 / 容灾 —— 完成度 ~60%

| 需求点 | 等级 | 现状 | 说明 |
|---|---|---|---|
| 负载均衡（权重/多 key 分摊） | 🔴 | ✅ | 五策略 + 凭证池 credential_strategy |
| 渠道健康分 + 熔断冷却 | 🔴 | ✅ | fallback_cooldown_secs + channel_health |
| 重试（按状态码 + 凭证轮换） | 🔴 | ◐ | 有，但只按状态码分可否重试，无 backoff 细分策略 |
| provider 偏好排序（order/only/ignore，用户请求级） | 🔴 | ❌ | 现路由完全管理员配置，用户无请求级控制 |
| 按 price/throughput/latency 排序 | 🟡 | ◐ | lowest_latency 是管理员配的组策略，非用户可选 sort |
| `require_parameters` 能力过滤 | 🟡 | ❌ | |
| `max_price`/`quantizations`/`zdr` 路由约束 | 🟡 | ❌ | |
| 三层正交容灾（熔断/冷却/模型锁定） | 🔴 | ◐ | 有熔断冷却，无「模型锁定」粒度分离 |
| 语义化 fallback 分离（容量型/能力型/内容策略型） | 🔴 | ❌ | 现在失败一律走同一条 failover 链 |
| 配额感知路由（fill-first） | 🟡 | ❌ | |
| Canary 灰度 | 🟢 | ❌ | |

### §五 Key / 多租户 / 治理 —— **完成度 ~25%，第二大差距**

| 需求点 | 等级 | 现状 | 说明 |
|---|---|---|---|
| Virtual Key 发放/撤销 | 🔴 | ◐ | 有发放/停用/软删 + 模型白名单；缺过期、轮换、IP 白名单、绑定 preset |
| 组织→团队→用户→key 层级 | 🔴 | ❌ | **结构性缺失**：users 是平的，无 org/team/workspace |
| 每租户独立 routing/preset/BYOK | 🟡 | ❌ | 依赖多租户先落地 |
| RBAC | 🔴 | ❌ | 只有一个 admin 密码 + 普通用户两态 |
| 团队额度公平切分 | 🟡 | ❌ | |
| 治理 guardrail（白名单/支出上限/数据策略） | 🔴 | ◐ | 模型白名单 ✅、配额上限 ✅；数据策略强制 ❌ |
| SSO/OIDC/SAML | 🟢 | ❌ | 现在是 magic-link 单体系 |

### §六 计费 / 额度 / 充值 —— 完成度 ~60%，内核强、产品面缺

| 需求点 | 等级 | 现状 | 说明 |
|---|---|---|---|
| 自有 credits/钱包 | 🔴 | ✅ | balance 微美元账本 + Stripe + 首充奖金 |
| 逐请求成本 + **反事实价格回显**（X-Cost 等响应头） | 🔴 | ◐ | 成本已逐请求落库；**响应头回显与反事实价完全没有** |
| 七层预算 + budget_duration 周期重置 | 🔴 | ◐ | 只有用户级配额窗口，无 key/team/org 层级预算 |
| 倍率/按量/按次/缓存命中计费 | 🔴 | ✅ | markup + 分桶计量（含 cache 折扣价）+ 多模态计量，超出需求 |
| 充值/订阅/兑换码/发票/易支付 | 🔴 | ◐ | Stripe 充值 ✅；订阅、兑换码、发票、易支付 ❌ |
| soft_budget 告警 | 🔴 | ❌ | 现在只有硬拒绝 |
| 定价表外置 | 🔴 | ✅ | config 权威、启动同步进 DB |
| 成本旋钮做成套餐参数 | 🟡 | ❌ | |

> **机制冲突**：现网关三种计费模式是 **Cargo feature 编译期切换**（三个不同二进制），而 token-station 的 Team/Enterprise 拍板是「**同一镜像、license 开关控差异**」。编译期开关必须改成运行时开关，否则交付物②③ × 档位的 SKU 矩阵会爆炸。

### §七–§十一 速览

| 域 | 完成度 | 关键缺口 |
|---|---|---|
| §七 内容 Guardrails | ❌ 0% | 🟡 级，按需求「优先复用开源（vLLM SR）」，后置 |
| §八 可观测/路由透明 | ◐ 40% | 用量统计/日志/看板 ✅；**路由透明三件套全缺**（决策原因、反事实价、覆写）；实际模型+精度写响应头 ❌；Feedback 采集 ❌ |
| §九 缓存/限流 | ◐ 40% | 缓存计费透传 ✅、channel 级 RPM ✅；**按 key/租户的 RPM/TPM 准入限流 ❌**（现在只有花费配额） |
| §十 开发者体验 | ◐ 60% | Playground/文档/五语言 ✅；本地客户端 ❌（新产品线）；Webhook/状态页 ❌ |
| §十一 企业/部署 | ◐ 45% | 单二进制/Docker/TLS ✅（= Team 档形态基本现成）；**Enterprise 档全缺**：K8s/Postgres/Redis、多副本状态共享（现 RPM/轮询计数在单机内存）、license 体系、air-gap 激活、SSO、审计日志 |

---

## 二、六个结构性差距（加功能补不了的）

1. **路由配置权在管理员手里，token-station 要下放给用户/租户**。现网关的 routing_groups/model_aliases/WASM 全局链都是运营者视角；token-station 的第①层规则、Preset、provider 偏好、成本旋钮全是**用户自助配置**。这是数据模型（配置要挂租户/key 维度）+ 权限模型 + 产品面三层的改造，是升级工程的主轴。

2. **凭证模型没有「用户拥有」维度（BYOK）**。provider_credentials 是平台全局池。BYOK 需要：凭证挂 user/tenant、路由时按「BYOK 三态」参与派单、计费区分「平台代采购」与「BYOK 只收路由服务费」两条账路（对应功能需求清单 §6.1 的 A/B 结算）。

3. **计费/档位是编译期 feature，需求要运行时 license/套餐开关**。含 Team/Enterprise license 签发与校验（air-gap 用 Ed25519 签名 license 文件，可复用现有安装包签名的思路）。

4. **单机内存态挡住 Enterprise 多副本**。RPM tracker、round-robin 计数、熔断冷却状态都在进程内存；SQLite 单写。Team 档（单机 SQLite）现状即可交付，Enterprise 档需要状态外置（Redis）+ Postgres 支持。好消息：这条可以只做 Enterprise 档，不动 Team 默认形态。

5. **请求链路没有「决策记录」结构**。usage_records 记了 routing_channel_id（结果），没记「为什么」（命中哪条规则/哪层路由/候选集/反事实价）。路由透明三件套要求 decision trace 贯穿 routing→dispatch→落库→响应头→账单页，这是一条纵切的新数据通路，越早埋越便宜——**建议升级第一刀就切这里**。

6. **一套代码要裂变成三个交付物**。现网关 ≈ 交付物②③的混合体；交付物①本地客户端（127.0.0.1 单进程、CLI 唯一管理面、本地路由引擎）按概要设计是独立二进制。升级方案：**把质量侧路由引擎（规则/启发式/分类器推理）抽成无 IO 依赖的 Rust crate**，服务端与本地客户端共用——概要设计 §七「客户端 Go 或 Rust」由此拍向 Rust，省一套路由内核的双语言维护。

---

## 三、资产盘点：网关超出需求的部分怎么处置

| 现有资产 | 处置建议 |
|---|---|
| **订阅型上游**（Claude Code/Codex/Copilot/Kiro/GLM Coding Plan 当凭证 + OAuth 刷新） | **保留，升为差异化卖点**。这本质是「BYOK 的订阅变体」（自带订阅套餐），OpenRouter 没有这个能力面；与个人客户端的 BYOK 人群天然契合。建议纳入 BYOK 改造统一设计「自带 key / 自带订阅」两种用户凭证 |
| 多模态端点（图/音/视频/音乐，含异步任务轮询） | 保留。需求清单标 🟢「扩品类」，已经做完就是白赚的品类差异化 |
| WASM 插件机制 | **降级为 Enterprise 扩展点，不作为用户规则的交付形态**。第①层规则必须是声明式配置（可审计、可版本化、Preset 化）；WASM 保留给「声明式表达不了」的企业自定义逻辑。complexity_router 的 10 维打分算法**平移进内核**作为第②层内建实现 |
| profile-analyzer + Builder Profile + 分享卡片 | **默认下线，慎重处置**。对用户内容做画像与 token-station「路由器自证清白」的信任叙事直接冲突（智能路由能力 §二的飞轮三条出路都不含内容画像）。可保留为私有部署运营方自用工具（数据在客户边界内），平台 SaaS 形态下不启用 |
| 请求日志 full 级（存完整请求/响应内容） | 交付物②无碍（客户边界内）；**交付物③需按数据策略治理**（ZDR/留存承诺是 §四路由约束的消费项），默认档位收紧为 usage |
| 五语言 i18n、营销页、magic-link 账号体系 | 保留直用 |

---

## 四、升级业务方案

### 4.1 产品形态映射

```
                    ┌── 交付物① 本地客户端（新二进制，C1–C3）
                    │      复用: 路由引擎 crate + 协议互译 crate + 定价表格式
cloud_ai_gateway ───┤
  （升级为服务端内核）  └── 交付物②③ 同一服务端（同一镜像）
                           ② 私有部署: Team 档(≈现形态) / Enterprise 档(license 解锁)
                           ③ 平台 SaaS: 平台自营部署 + credits/订阅计费
```

- 交付物②③共用代码库，差异全部走**运行时 license/套餐开关**（替换 Cargo feature）；
- 平台侧新增模块（客户端下载分发、设备码授权、云同步接收、远程配置服务、用量报表）按概要设计 §五挂进服务端；
- glm5.2-platform 按拍板口径始终只是一个普通上游渠道，不做任何特殊对接。

### 4.2 分期路线（对齐需求清单 M1–M4）

> 2026-07-08 拍板：升级工程按 [GlimpseEngine/rust-coding-standards](https://github.com/GlimpseEngine/rust-coding-standards) 执行。对现有代码的 15 项规范符合性审计见 [开发规范符合性审计](./cloud-ai-gateway开发规范符合性审计.md)——**新增 P0 加固期**，且 P1–P4 各期并入审计文档 §三的规范改造搭车项（P1 搭 request_id/错误码/tracing 重写，P2 搭 Service 层雏形/退避 jitter/熔断状态机，P3 搭 rusqlite→SQLx 迁移/Redis 外置/巨型文件拆分，P4 搭错误目录完整化）。

**P0 — 规范加固期（先于一切功能，约 2–3 周）**

七项高危修复，详见审计文档 §二：SSE 断连计费丢失（流式结算移入 drop guard）、认证安全五连（Argon2id/session 哈希/cookie flags/CSRF/constant-time）、CI 自动门禁（PR 触发 + deny/audit）、优雅停机 + livez/readyz、秘密 secrecy 包裹、错误体停止回显内部错误（引入 thiserror）、计费 overflow-checks + proptest 守恒测试。其中 SSE 丢账与计费守恒直接关系 token-station 的**计费可信度叙事**——路由透明产品自己的流式计费先丢账，是产品级矛盾。

**P1 — 决策链路 + BYOK + 接入产品化（网关改造的第一刀）**

| 事项 | 对应差距 |
|---|---|
| **决策记录（decision trace）纵切**：请求链路记录「候选集→逐层决策→最终模型/provider/凭证→原因码」，落 usage_records 扩展 + 写响应头（`X-Routed-Model`/`X-Routed-Reason`/`X-Cost`） | 结构性差距 5，路由透明的地基 |
| BYOK：凭证挂用户/租户 + 三态 fallback + 两条结算账路（代采购 / BYOK 服务费） | 结构性差距 2，🔴 |
| 模型目录选型页、SDK 打包、模型名后缀语法 | §一/§二 零散 🔴🟡 |
| 路由引擎 crate 骨架（无 IO 依赖）+ 本地文件配置源，先覆盖规则/启发式最小子集 | 为本地客户端 C1 解依赖；P2 再接服务端 DB 配置源 |

**P2 — 质量侧路由 + 路由透明（对应 M2，价值密度最高的一期）**

| 事项 | 说明 |
|---|---|
| 第①层声明式规则引擎（match 维度：token 数/代码块/tool/JSON schema/会话轮数/header/时段/预算余量），P2 先挂 user/key，P3 升级为 org/team/key | 智能路由能力 §一①，命中即终止；避免 P2 依赖 P3 多租户 |
| 第②层内建启发式打分：平移 complexity_router 算法进 P1 crate，P2 接服务端 DB 配置源，阈值与目标池可配 | 现成算法资产 |
| hint header 消费（Agent 步骤类型） | 第 2 层，客户端依赖它 |
| 供给侧补齐：provider 偏好（order/only/ignore）、sort、max_price 等请求级约束、语义化 fallback 分离（容量/能力/内容策略三条链） | §四 🔴 项 |
| 路由透明三件套成品化：账单页「本月路由为你省了 $X」（反事实价用定价表算）、决策原因可查、①层规则覆写闭环 | 续费关键展示位 |
| Auto Router 端点（`auto` 逻辑模型 = ①+② 的默认组合策略） | OpenRouter 形态 |

**P3 — 多租户治理 + 计费产品化 + license 分档（对应 M3，让产品可独立销售）**

| 事项 | 说明 |
|---|---|
| org→team→user→key 层级 + workspace + RBAC（替换单 admin 密码）；迁移 P2 的 user/key 规则归属为 org/team/key | 结构性差距 1 的权限半边 |
| 计费运行时化：balance/quota/credits 三模式 + 套餐/订阅 → 运行时配置；license 签发/校验（air-gap 离线激活） | 结构性差距 3 |
| 预算层级化（org/team/key + budget_duration 周期重置）+ soft 告警 | 抄 LiteLLM |
| 订阅、兑换码、发票、易支付；按 key/租户 RPM/TPM 准入限流 | 运营侧补全 |
| Enterprise 部署档：Postgres/Redis 状态外置、多副本、SSO/OIDC、管理操作审计日志 | 结构性差距 4；只做 Enterprise 档，Team 保持单机 SQLite |
| 客户端 C2 配套：设备码授权、云同步接收、用量报表、最小远程配置 profile 服务、客户端下载分发与签名流水线 | 远程配置先做非机密 profile 的版本化/回滚/导出，不要求 `@preset` 别名产品化 |

**P4 — 学习型路由 + Preset + 数据飞轮（对应 M4，护城河）**

| 事项 | 说明 |
|---|---|
| Preset 服务端集中管理产品化（`@preset/xxx`，版本化/回滚/导出）——构建在 P3 最小远程配置 profile 服务之上 | ①层规则的最终交付形态，不阻塞 C2 远程配置 |
| 第③层学习型路由：先 LLM-as-Router 冷启动（④档顶替），攒元数据后训分类器；蒸馏本地小模型（ONNX）给客户端 C3 | 智能路由能力 §三路线 |
| Feedback 采集 + 元数据飞轮（条款先留口子：只用元数据/opt-in 换折扣） | 智能路由能力 §二尖锐点 2 |
| 成本旋钮套餐参数化、级联兜底（仅非流式/批处理） | 🟡🟢 收尾 |

**并行轨 — 本地客户端 C1–C3**（依概要设计既有里程碑）：P1 期完成路由引擎 crate 骨架并启动 C1；C1 同时验离线可用与网络边界。C2 依赖的平台侧模块（设备码授权/云同步/报表/最小远程配置 profile）排进 P3，并验云同步字段白名单。C3 的小分类器依赖 P4 蒸馏产出，闭源信任在 C3 收口为本地审计输出。

### 4.3 各期出口标准（可验证）

- **P1**：任一请求的响应头能看到实际模型+原因码；一个用户能用自己的 OpenAI key 走 BYOK 完成请求且账单只收路由费
- **P2**：用户在仪表盘写一条「带 tool 的请求给 claude」规则即刻生效；账单页出现「本月节省 $X」；Claude Code 挂 hint header 后路由按步骤类型分流
- **P3**：同一镜像用两个 license 分别起出 Team/Enterprise 形态；一个 org 建两个 team 各自预算独立扣减
- **P4**：`@preset/code-review` 别名跨客户端生效并可回滚版本；auto 端点的路由决策由分类器给出且特征可查

---

## 五、风险与已决口径

| # | 风险/决策点 | 口径 |
|---|---|---|
| 1 | 决策链路晚埋 → P2 之后每个路由功能都要回头补 trace | P1 第一刀先切决策记录（§4.2） |
| 2 | 多租户 schema 改造动 users/api_keys/usage_records 核心表 | 迁移体系（0057+）已成熟，但要求 P3 前冻结相关表的其他改动窗口 |
| 3 | 内容画像资产与信任叙事冲突 | profile-analyzer 平台形态默认下线（§三），避免「路由器偷看内容」的舆情面 |
| 4 | WASM 与声明式规则双轨并存造成产品面混乱 | 拍板：规则=产品面（全档位），WASM=Enterprise 扩展点，文档不并列宣传 |
| 5 | 编译期 feature 迁运行时的过渡期 | P3 完成前保留 feature 构建线不删，双轨到 license 体系验收通过 |
| 6 | 本地客户端语言选型 | 建议 Rust（复用路由引擎/互译/定价 crate），推翻概要设计 §七「Go 或 Rust」的开放项——若拍板 Go 则路由内核需双实现，成本理由需重估 |

---

## 六、与既有文档的同步

| 文档 | 同步内容 | 状态 |
|---|---|---|
| [token-station 功能需求清单](../features/token-station功能需求清单.md) §十二 | M1–M4 各阶段可标注「网关已有/需改造/需新建」基线 | 待回写 |
| [个人模式本地客户端概要设计](./个人模式本地客户端概要设计.md) §七 | 客户端语言选型建议收敛为 Rust（复用路由 crate），待拍板 | 待决 |
| [cloud-ai-gateway 功能清单](../research/cloud-ai-gateway-自研网关功能清单.md) | 本文的能力现状依据 | 已引用 |
| [开发规范符合性审计](./cloud-ai-gateway开发规范符合性审计.md) | P0 加固期与 P1–P4 规范搭车项的依据；两个待拍板问题（profile-analyzer 定位、spawn_blocking 范围） | 已并入 §4.2（2026-07-08） |

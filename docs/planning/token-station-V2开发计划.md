# token-station V2 完整开发计划

> 场景：以 V1（现有 [cloud_ai_gateway](https://github.com/second-state/cloud_ai_gateway)，能力基线见 [功能清单](../research/cloud-ai-gateway-自研网关功能清单.md)）为基座，开发 V2（token-station 全量需求，见 [功能需求清单](../features/token-station功能需求清单.md)），全程按 [rust-coding-standards](https://github.com/GlimpseEngine/rust-coding-standards) 执行重构（符合性差距见 [规范审计](../architecture/cloud-ai-gateway开发规范符合性审计.md)）。差异分析与升级策略的依据见 [升级差异分析与业务方案](../architecture/cloud-ai-gateway升级差异分析与业务方案.md)。
>
> 整理日期 2026-07-08。工作量为粗估（人周），供排期与人力决策，进入各期前需按详细设计复核。

---

## 一、术语与总体策略

| 术语 | 含义 |
|------|------|
| **V1** | 现有 cloud_ai_gateway：M1 门面基本完整、供给侧路由 ~60%、计费内核 ~60%，质量侧路由/透明/BYOK/多租户缺失 |
| **V2** | token-station 需求全量：三交付物（①本地客户端 / ②私有部署网关 / ③平台 SaaS）、四层质量侧路由、路由透明三件套、自有计费、多租户治理 |
| **重构** | 按 rust-coding-standards 15 项规范对 V1 代码的改造：7 项高危加固 + 结构性债务（Service 层、SQLx、巨型文件、测试体系） |

**三条编排原则**：

1. **重构不整体立项、不停机重写**。只有 P0 是纯重构期（高危加固，独立可交付）；其余重构项全部「搭车」在 V2 功能期里——功能触到哪个模块，就顺势把该模块重构到规范（避免同一文件开两次膛）。
2. **V1 主干渐进演进（trunk-based）**。每期结束 V1 都处于可发布状态，V2 功能用配置/feature 开关渐进放量，不搞 big-bang 切换；users/api_keys/usage_records 等核心数据经 migration 平滑升级，不迁库重导。
3. **一个内核、三个交付物**。服务端（交付物②③同一代码库，license 分档）走 P0–P4 主线；本地客户端（交付物①）走 C1–C3 支线，复用服务端抽出的路由引擎 crate；两线在 P2（共建路由 crate）与 P3（账号/授权/云同步配套）有强同步点。

---

## 二、工作分解总览

```
服务端主线   P0 重构加固 → P1 决策链路+BYOK → P2 质量侧路由 → P3 多租户+计费+Enterprise → P4 学习型路由+Preset
                              │                  │                    │                        │
V2 里程碑                    M1 补齐            M2                   M3                       M4
                                                 │                    │                        │
客户端支线                              C1 最小可用(共建路由crate)  C2 账号与云端观测          C3 完整形态
```

任务标签：〔V2〕新功能 · 〔重构〕规范对齐 · 〔配套〕客户端的服务端配套。

---

## 三、阶段详表

### P0 — 重构加固期（纯重构，先于一切功能）

**目标**：清除全部高危缺陷，建立工程门禁。V1 在此期结束时是一个「安全、有 CI 门禁、计费不丢账」的基线。

| # | 任务 | 标签 | 粗估 |
|---|------|------|------|
| 1 | SSE 断连计费丢失修复：流式结算移入 spawned task / drop guard，断连时已产出 token 入账 | 〔重构〕 | 1 人周 |
| 2 | 认证安全五连：admin 密码 Argon2id + spawn_blocking、session token 哈希存储、cookie `Secure`/`__Host-`、CSRF 中间件、`subtle` constant-time 比较 | 〔重构〕 | 1 人周 |
| 3 | CI 门禁：PR/push 自动触发，fmt → clippy(-D warnings) → test → cargo deny → cargo audit；补 rustfmt.toml / clippy 配置 / deny.toml / rust-toolchain.toml；edition 2021→2024 迁移 | 〔重构〕 | 1 人周 |
| 4 | 优雅停机：tokio::signal + CancellationToken 根 + with_graceful_shutdown，后台 loop 接线；补 /livez /readyz | 〔重构〕 | 0.5 人周 |
| 5 | 秘密治理：secrecy::Secret 包裹全部凭证字段；config 补 deny_unknown_fields | 〔重构〕 | 0.5 人周 |
| 6 | 错误体收口：引入 thiserror + `#[from]`（消灭 638 处手工 map_err）；Database/Internal 变体停止向客户端回显内部错误 | 〔重构〕 | 1 人周 |
| 7 | 计费防线：release overflow-checks；db/billing.rs 补 proptest 守恒不变量测试 | 〔重构〕 | 0.5 人周 |

**出口标准**：CI 在 PR 上全绿门禁生效；模拟客户端中途断开流式请求，用量完整入账；安全项复测通过（cookie flags / CSRF / 哈希存储可验证）。
**小计：≈ 5.5 人周**

### P1 — 决策链路 + BYOK + 接入产品化

**目标**：埋下路由透明的地基（决策记录纵切），补齐 BYOK 这一 🔴 级结构缺失，先抽出本地客户端 C1 可复用的路由引擎 crate 骨架，并建立 Agent 接入 / Model Provider 双端 adapter 插件地基。

| # | 任务 | 标签 | 粗估 |
|---|------|------|------|
| 1 | 决策记录纵切：请求链路记录候选集→逐层决策→最终模型/provider/凭证→原因码；usage_records 扩展；响应头 `X-Routed-Model` / `X-Routed-Reason` / `X-Cost` | 〔V2〕 | 2 人周 |
| 2 | 错误响应搭车改造：对象内补稳定 `code` + `request_id`/`trace_id`（保留 OpenAI 兼容外形），与 #1 同一刀 | 〔重构〕 | 0.5 人周 |
| 3 | 可观测底座：重写 init_tracing（JSON/pretty 按 config、OTel、guard）；关键路径补 `#[instrument]` | 〔重构〕 | 1 人周 |
| 4 | BYOK：provider_credentials 增加用户/租户归属维度；三态 fallback（always-use / fallback / 默认回退）；两条结算账路（平台代采购 / BYOK 只收路由服务费）；订阅型上游纳入统一「自带 key / 自带订阅」模型 | 〔V2〕 | 3.5 人周 |
| 5 | 模型目录选型页（价格/上下文/吞吐对比）+ SDK 打包（Python/JS）+ 模型名后缀语法（`-thinking`/`-high`） | 〔V2〕 | 1.5 人周 |
| 6 | 路由引擎 crate 骨架（无 IO 依赖）+ 本地文件配置源；先覆盖规则/启发式最小子集，供 C1 复用 | 〔V2+客户端地基〕 | 2 人周 |
| 7 | 双端 adapter 插件地基：Canonical IR、AgentHint、`agent-adapter-v1` / `provider-adapter-v1` WASM Component ABI、manifest schema、双端 conformance runner、OpenAI-compatible agent/provider 官方样板插件 | 〔V2+生态地基〕 | 4 人周 |

**出口标准**：任一请求响应头可见实际模型 + 原因码 + request_id；用户可用自己的 OpenAI key 走 BYOK 完成请求且账单只收路由费；日志为结构化 JSON 且含 trace 关联；社区 CLI 可本地加载通过 conformance 的 OpenAI-compatible agent/provider 两个 WASM adapter。
**小计：≈ 14.5 人周**

### P2 — 质量侧路由 + 路由透明（V2 M2，价值密度最高）

**目标**：交付四层路由框架的第 ①② 层与路由透明三件套——V2 的产品灵魂。P1 已抽出路由引擎 crate 骨架；本期重点是接入服务端 DB 配置源、补齐服务端候选集/决策记录集成与用户可见产品面。

| # | 任务 | 标签 | 粗估 |
|---|------|------|------|
| 1 | 第①层声明式规则引擎服务端集成：match 维度 = token 数/代码块/tool 定义/JSON schema 要求/会话轮数/header/时段/预算余量；命中即终止；P2 先挂 user/key，P3 升级为 org/team/key | 〔V2〕 | 3 人周 |
| 2 | 第②层内建启发式：complexity_router 的 10 维打分算法接入 P1 crate，服务端 DB 配置源提供阈值与目标池 | 〔V2〕 | 1 人周 |
| 3 | 客户端 hint header 消费（Agent 步骤类型，第 2 层） | 〔V2〕 | 0.5 人周 |
| 4 | 供给侧补齐：provider 偏好（order/only/ignore）、price/throughput/latency sort、max_price/require_parameters 等请求级约束 | 〔V2〕 | 2 人周 |
| 5 | 容灾升级：语义化 fallback 分离（容量型/能力型/内容策略型三条链）；failover 补指数退避 + jitter；cooldown 升级为 open/half-open 熔断状态机 | 〔V2+重构〕 | 2 人周 |
| 6 | 路由透明三件套产品化：账单页「本月路由为你省了 $X」（反事实价按定价表计算）、逐请求决策原因可查页、①层规则覆写闭环 | 〔V2〕 | 2 人周 |
| 7 | Auto Router 端点（`auto` 逻辑模型 = ①+② 默认组合策略） | 〔V2〕 | 1 人周 |
| 8 | Service 层雏形：routing 域先行 DDD 切片示范（handler→service→repo），随 #1–#5 对 routing.rs/dispatch.rs/retry.rs 的重写落地 | 〔重构〕 | 1 人周 |

**出口标准**：用户在仪表盘写一条「带 tool 的请求给 claude-sonnet」规则即刻生效并可在决策页看到命中记录；账单页出现节省金额；Claude Code 挂 hint header 后按步骤类型分流；routing 域代码符合分层规范。
**小计：≈ 12.5 人周**

### P3 — Team 商业闭环 + Enterprise 基座（V2 M3，最重的一期）

**目标**：先让 Team 档可独立销售，再把 Enterprise 基座补齐。P3 拆成 **P3a Team 商业闭环** 与 **P3b Enterprise 基座**：P3a 交付组织/成员/key/预算/计费/license/单机部署/C2 配套；P3b 交付 PG/Redis 多副本、OIDC、审计、K8s/Helm。**SQLx 迁移与多租户 schema 大改仍是同一手术窗口**，但 Enterprise 销售不得压在 P3a 出口。

| # | 任务 | 标签 | 粗估 |
|---|------|------|------|
| 1 | P3a 多租户：org→team→user→key 层级 + workspace 隔离 + RBAC（替换单 admin 密码）；每租户独立 routing/preset/BYOK | 〔V2〕 | 5 人周 |
| 2 | P3a rusqlite → SQLx 迁移（SQLite 后端先行，PG 后端随 P3b），与 #1 的 schema 改造同窗口执行 | 〔重构〕 | 4 人周 |
| 3 | 计费运行时化：balance/quota/credits 三模式由 Cargo feature 改为运行时配置；license 签发/校验体系（Ed25519 签名文件，支持 air-gap 离线激活）；Team/Enterprise 开关矩阵 | 〔V2〕 | 3 人周 |
| 4 | 预算层级化：org/team/key 级预算 + budget_duration 周期重置 + soft 告警；key 过期/轮换/IP 白名单 | 〔V2〕 | 2 人周 |
| 5 | 运营侧补全：订阅、兑换码、发票、易支付；按 key/租户 RPM/TPM 准入限流 | 〔V2〕 | 3 人周 |
| 6 | P3b Redis 外置（Enterprise 档）：RPM 滑动窗口（Lua 原子）、全局轮询计数、共享熔断冷却，fail-open 降级 | 〔V2+重构〕 | 1.5 人周 |
| 7 | P3b Enterprise 部署形态：Postgres 支持、多副本、SSO/OIDC、管理操作审计日志、K8s/Helm（不含 SCIM / 细粒度 RBAC / 超额审批流） | 〔V2〕 | 6 人周 |
| 8 | 巨型文件拆分（admin.rs 3774 行必拆，随多租户触及逐个执行）；集成测试 Rust 化（计费与路由核心断言迁 `tests/` + testcontainers，license 分档矩阵测试） | 〔重构〕 | 2 人周 |
| 9 | 〔配套 C2〕设备码授权（RFC 8628）、云同步接收接口、用量报表网页、最小远程配置 profile 服务（非机密配置版本化/回滚/导出）、客户端下载分发 + 签名流水线 | 〔配套〕 | 3 人周 |

**出口标准**：P3a 出口 = Team（单机 SQLite）可售：一个 org 建两个 team 各自预算独立扣减与告警；`cargo sqlx prepare` 覆盖 SQLite 查询；客户端可完成设备码绑定并在平台网页看到多设备用量。P3b 出口 = Enterprise 基座可售：同一镜像用 Enterprise license 起出 PG+Redis 多副本形态，OIDC 与管理审计日志可用。
**小计：≈ 29.5 人周**（P3a ≈ 22 / P3b ≈ 7.5）

### P4 — Preset + 学习型路由 + 数据飞轮（V2 M4，护城河）

| # | 任务 | 标签 | 粗估 |
|---|------|------|------|
| 1 | Preset 服务端集中管理产品化：在 P3 远程配置 profile 之上提供 `@preset/xxx` 命名别名（模型+参数+provider 偏好+路由策略）、版本化/回滚/导出与套餐差异化 | 〔V2〕 | 2 人周 |
| 2 | 第③层冷启动：LLM-as-Router（便宜模型判难度/领域，prompt 即策略） | 〔V2〕 | 1.5 人周 |
| 3 | Feedback 采集 + 元数据飞轮（重试/重新生成信号；条款留口子：仅元数据 / opt-in 换折扣） | 〔V2〕 | 2 人周 |
| 4 | 学习型分类器：元数据+公开偏好数据训练与离线评测；只有达到质量/成本门槛才替换 LLM 判官，否则作为元数据策略优化器保留；蒸馏本地小模型（ONNX，几十 MB）供客户端 C3 | 〔V2〕 | 4 人周 |
| 5 | 成本旋钮套餐参数化（0=全便宜…1=全SOTA）；级联兜底（仅非流式/批处理/agent 步骤） | 〔V2〕 | 2 人周 |
| 6 | 错误目录完整化：集中 catalog（code→status→message_key 单表 + 唯一性 CI）+ API 错误 Fluent 五语言本地化 | 〔重构〕 | 1.5 人周 |

**出口标准**：`@preset/code-review` 别名跨客户端生效并可回滚版本；auto 端点可由 LLM 判官或达标分类器给出决策，且「读了哪些特征」可查；ONNX 分类器在客户端本地推理延迟 <10ms，并附离线评测报告。
**小计：≈ 13 人周**

### 客户端支线 C1–C3（交付物①，依 [概要设计](../architecture/个人模式本地客户端概要设计.md) 里程碑）

| 阶段 | 内容 | 依赖 | 粗估 |
|------|------|------|------|
| **C1 最小可用** | 本地端点（127.0.0.1，OpenAI 兼容+流式）+ BYOK 直连 + 本地模型（Ollama/vLLM）+ 规则/启发式路由（复用 P1 路由 crate）+ 密钥保管（钥匙串/主密码）+ 文件日志 + 指标库（SQLite，默认开启）+ CLI 管理面 + 简统计命令 + 注册下载（零绑定可用）+ 离线可用与网络边界最小化验收 | P1 路由 crate 骨架；账号/下载分发可先用 V1 现有账号体系 | 8 人周 |
| **C2 账号与云端观测** | 设备码授权（懒触发）+ 平台账户上游 + 云同步（字段白名单硬编码）+ 定价表下发与成本估算 + 最小远程配置 profile 拉取 + 匿名版本检查/签名升级 + 同步字段白名单验收 | P3 #9 服务端配套 | 5 人周 |
| **C3 完整形态** | 本地对账命令（拉只读账单 → 与指标库 diff → 差异报告）+ 内嵌 ONNX 小分类器 + 本地审计输出（配置摘要、启用云端功能、同步字段版本、最近 N 条路由决策与外联摘要） | P4 #4 蒸馏产出；对账依赖账单只读 API | 4 人周 |

**客户端小计：≈ 17 人周**（语言拍板 Rust，见 [升级方案 §五#6](../architecture/cloud-ai-gateway升级差异分析与业务方案.md)；若拍 Go 则路由内核双实现，C1 +3 人周且长期双维护）

---

## 四、里程碑映射与关键路径

### 映射表

| 开发阶段 | V2 需求里程碑 | 客户端 | 交付物状态 |
|---------|-------------|--------|-----------|
| P0 | —（加固） | — | V1 安全基线 |
| P1 | M1 补齐 + 透明地基 + BYOK + 路由 crate 骨架 | C1 启动（复用 P1 crate） | V1 + 透明地基 + BYOK |
| P2 | M2 | C1 继续 | **可对外宣传的智能路由器**（适合 design partner / 试点，不称 Team 档可售）|
| P3a | M3 Team | C1 交付、C2 启动 | **Team 可售、SaaS 可运营、①客户端可下载** |
| P3b | M3 Enterprise | C2 启动 | **Enterprise 基座可售**（PG/Redis 多副本、OIDC、审计、K8s/Helm） |
| P4 | M4 | C2 交付、C3 启动→交付 | 完整护城河形态 |

### 关键路径与强依赖

```
P0 → P1(决策链路 → BYOK → 路由crate骨架) → P2(服务端规则引擎 → 透明三件套) → P3a(SQLx+多租户schema → Team商业闭环) → P3b(Enterprise基座) → P4(飞轮 → 分类器/评测 → 蒸馏)
                                              └→ C1(依赖P1路由crate) ──→ C2(依赖P3a#9配套) ─────────────────────────→ C3(依赖P4#4蒸馏)
```

- **P3 是关键路径瓶颈**（约 29.5 人周）：SQLx 迁移与多租户 schema 必须同窗口，期间冻结其他人对核心表的 migration；商业出口拆成 P3a Team 与 P3b Enterprise，避免把 Enterprise 销售压在最大手术窗口上；
- **P1 路由 crate 是两线汇合点**：crate 的 API 设计要同时满足服务端（DB 配置源后续接入）与客户端（本地文件配置源），设计评审必须两线共同参与；
- 决策记录（P1#1）先于一切路由功能——P2 每个路由层都要能解释自己，晚埋则全部返工。

---

## 五、工作量汇总与排期方案

| 线 | 人周 |
|----|------|
| 服务端 P0–P4 | ≈ 75（P0 5.5 / P1 14.5 / P2 12.5 / P3 29.5 / P4 13）|
| 客户端 C1–C3 | ≈ 17 |
| **合计** | **≈ 92 人周**（含 ~20% 各期内置的测试与文档，不含产品/设计/运营）|

### 排期方案（按投入人力）

| 方案 | 人力 | 节奏 | V2 全量（P4+C3 完成）|
|------|------|------|---------------------|
| **A 推荐** | 2 名服务端 Rust + 1 名客户端 Rust（C1 起加入）| P0–P1 双人串行推进；P1 crate 骨架完成后客户端并行；P3 双人全投 | **约 8–9 个月** |
| B 精简 | 2 名 Rust 全栈 | 客户端 C1 插在 P1 后串行做，C2/C3 与 P3/P4 交错 | 约 11–12 个月 |
| C 单人 | 1 名 | 不建议——P3 单人窗口过长，SQLx+schema 大改无人 review，风险不可控 | >18 个月 |

> 方案 A 的分期出口即商业节点：**P2 末（约第 4 个月）可启动智能路由 design partner / 试点销售**，不称 Team 档早鸟；P3a 末 Team 与 SaaS 可正式销售，P3b 末 Enterprise 基座可售，P4 是续费与差异化。

---

## 六、阶段验收总清单

### 6.1 通用门禁

每个阶段结束都必须满足：

| 门禁 | 验收方式 |
|------|----------|
| V1 回归不破 | 现有 API / 计费 / 路由 / 管理端关键路径回归通过；对外 OpenAI 兼容 shape 只加不改 |
| 工程质量 | fmt、clippy `-D warnings`、unit/integration tests、cargo deny、cargo audit 通过 |
| 数据迁移 | migration 可重复执行、可从上一稳定版本升级；涉及核心表时补回滚/降级说明 |
| 安全边界 | secret 不进日志；错误响应不泄露内部细节；客户端内容不上云约束未被破坏 |
| 可观测 | 新增关键路径有 trace_id/request_id，失败可定位到阶段内新增模块 |
| 文档同步 | 阶段新增能力同步到需求清单、架构设计、DB 设计和用户可见口径 |

### 6.2 阶段出口验收

| 阶段 | 必过验收 | 不通过则不得进入 |
|------|----------|------------------|
| P0 | 流式断连后 usage 完整入账；认证安全五项复测通过；CI 门禁真实拦截失败 PR；/livez /readyz 与优雅停机可演示 | P1 功能开发 |
| P1 | 任意请求可查决策记录并返回 `X-Routed-Model` / `X-Routed-Reason` / `X-Cost`；服务端 BYOK 三态和双账路跑通；`router-core` 最小 API 可被 C1 调用；OpenAI-compatible `agent-adapter` / `provider-adapter` WASM 插件通过 conformance 并可本地加载 | P2 完整路由、C1 客户端开发 |
| P2 | 用户规则即时生效并可在决策页解释；启发式分档、Agent hint、provider 偏好、请求级约束、语义 fallback 都写入决策记录；账单页能展示反事实节省金额；routing 域 handler 不再直连 DB | design partner / 试点销售 |
| C1 | IDE/Agent 指向 `127.0.0.1` 可用；BYOK / 本地模型混合路由；断网可用；key 不进日志；指标库默认开启；关闭云同步/远程配置时外联目的地可解释 | 个人版本地试用 |
| P3a | Team 单机 SQLite 可售：org/team/user/key 隔离；预算、限流、计费、license、订阅/发票/兑换码跑通；C2 所需设备码、云同步接收、下载分发和最小远程配置服务可用 | Team 正式销售、C2 启动 |
| C2 | 设备码懒触发绑定；平台账户上游可切换；云同步字段白名单抽样无 prompt/response/header secret；平台网页可看多设备用量；远程配置离线缓存可回退 | 个人版云端观测发布 |
| P3b | Enterprise license 起出 PG+Redis 多副本；OIDC、管理审计、K8s/Helm 可用；双副本下限流/熔断状态全局一致；离线/air-gap license 激活可演示 | Enterprise 基座销售 |
| P4 | `@preset` 可跨客户端生效并回滚；LLM-as-Router 或达标分类器能给出可解释决策；分类器有离线评测报告，不达标不得替换判官；错误目录唯一性 CI 通过 | V2 完整护城河发布 |
| C3 | 本地对账命令能拉只读账单并与指标库 diff；ONNX 小分类器本地推理 <10ms；本地审计输出包含配置摘要、云功能开关、同步字段版本、最近 N 条路由决策与外联摘要 | 个人版完整形态 |

---

## 七、风险登记

| # | 风险 | 缓解 |
|---|------|------|
| 1 | P3 的 SQLx + 多租户 schema 同窗口改造是全程最大手术，失败会拖垮排期 | 拆 P3a/P3b 商业出口；窗口前冻结核心表其他 migration；SQLite 后端先迁、PG 后置；每步保留 rusqlite 回退分支直到验收 |
| 2 | 路由 crate 双端 API 设计不当 → 客户端与服务端各自 fork | crate 设计评审两线共审；配置源抽象（trait）从第一天就双实现（DB / 本地文件）|
| 3 | 学习型路由（P4#4）依赖数据积累，且仅元数据训练可能不足以替代内容级判官 | P1 决策记录落库字段就按训练特征需求设计；冷启动用 LLM-as-Router 顶替；分类器必须过离线评测门槛才替换，否则只作为策略优化器 |
| 4 | V1 存量用户在渐进升级中受 schema/行为变更影响 | 每期出口跑 V1 兼容回归（现有 bash 集成测试在此期间仍是资产）；对外 API 只加不改 |
| 5 | 重构与功能同支线开发，PR 混杂难 review | 约定 PR 打 `refactor:`/`feat:` 前缀分离提交；搭车重构以「模块」为单位整体交付 |
| 6 | 人力估算偏差（本表为文档级粗估）| 每期启动前出详细设计并复核工作量，偏差 >30% 触发重排期 |

## 八、待拍板清单（开工前）

| # | 事项 | 影响 | 建议 |
|---|------|------|------|
| 1 | 客户端语言：Rust 还是 Go | C1 +3 人周与长期双维护 | **Rust**（复用路由/互译/定价 crate）|
| 2 | profile-analyzer 定位 | 平台形态是否下线、共享 DB 是否违规 | 按升级方案 §三：默认下线，留私有部署自用 |
| 3 | 排期方案 A/B/C | 总周期 8 个月 vs 12 个月 | A（3 人）|
| 4 | P2 末商业动作 | 提前回款 vs 预期错配 | 只启动智能路由 design partner / 试点销售；Team 档正式销售放到 P3a 出口 |
| 5 | spawn_blocking 范围 | P2 全量 vs 等 P3 SQLx | 等 P3 SQLx（原生 async 根治），P2 仅 billing 热路径 |

## 九、与既有文档的同步

| 文档 | 关系 | 状态 |
|------|------|------|
| [升级差异分析与业务方案](../architecture/cloud-ai-gateway升级差异分析与业务方案.md) | P0–P4 的差距依据与阶段定义来源；本文细化为可排期计划 | 一致（2026-07-08）|
| [规范审计](../architecture/cloud-ai-gateway开发规范符合性审计.md) | 重构项与搭车编排来源；其 §五两个待拍板并入本文 §八 | 一致（2026-07-08）|
| [功能需求清单 §十二](../features/token-station功能需求清单.md) | M1–M4 切分与本文 P1–P4 对齐 | 一致 |
| [个人模式本地客户端概要设计 §八](../architecture/个人模式本地客户端概要设计.md) | C1–C3 里程碑来源；已同步为「P1 公共地基后启动 C1，并与 P2 共建路由能力」 | 一致（2026-07-09）|

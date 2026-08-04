# OmniRoute 与 token-station 深度竞品拆解与查漏补缺

> 研究日期：2026-07-27
>
> token-station 基线：`test/manual-agent-regression-20260723` @ `ba813b4471d6f923427cdf4771c3dacb743714fe`；业务代码与 `develop` @ `d8b29393b30e9b8038e1495c8c863c530e17e714` 一致，分支额外包含最新人工测试记录。
>
> OmniRoute 源码基线：官方仓库 `diegosouzapw/OmniRoute` 的 `release/v3.8.49` @ `ed7db3ee5f89a144b2d931d8605534522f83de30`；最新正式版是 [`v3.8.48`](https://github.com/diegosouzapw/OmniRoute/releases/tag/v3.8.48) @ `7ee5bbc64dbb03e967521227f2afffeb7c9dad1e`（`4f00f84...` 是 annotated tag object）。
>
> 2026-07-28 深度复核：开发源码补查至 `d6c06932ec9f27af140a43af030d2a77488e0863`，确认 19 个公开策略、内部 `quota-share`、Combo 与 Connection 两级选择的分类结论仍成立。下文保留初始取证提交的 permalink，避免漂移。
>
> 行动边界：本轮只调研、审计、验证和写报告；未改业务代码，未安装或登录真实上游账号，未提交或推送 Git。
> 证据口径：`已验证` = 本轮实际运行；`源码存在` = 固定提交的实现可直接证明；`文档宣称` = README/发布说明的产品口径；`外部信号` = GitHub issue/release，不等于本轮独立复现。

---

## 0. 执行摘要

### 0.1 先说最简单的：token-station 到底应该学什么

一句话：**不学 OmniRoute 怎样聚合大量免费账号或网页权益，只学怎样安全地管理同一合规供应商下、由组织授权的多条 Credential Connection。**

先明确证据边界：OmniRoute 整体源码可以证明多 Connection/账号配置、OAuth/Web session、额度读取、429/限流冷却和连接切换机制；不能证明账号通过日抛号、假信用卡或其他滥用方式获得，也不能把这些用户行为归因给项目。OpenCode 这条具体路径更窄：它证明的是本地 slot、可选 proxy、429 cooldown 与换槽，不是多个真实账号。

TS 真正应该学的只有四件事。

#### 1. 同一供应商下的多个合规 API Key

```text
Provider
  ├─ Credential A：企业主 Key
  ├─ Credential B：组织授权的备用 Key
  └─ Credential C：独立项目、独立预算
```

每条 Credential 都必须来源明确、受组织授权、进入批准的 SecretSource；Desktop 默认存入 OS Keychain，并记录负责人、用途、项目和预算 Scope。

#### 2. 感知正式的套餐与额度窗口

只从 Provider 的正式 Usage API、标准 rate-limit header 或管理员配置读取：

- 剩余额度；
- RPM/TPM/RPD；
- reset 时间；
- 月度预算；
- Credential 对应的组织、项目和模型范围。

目的不是“薅更多额度”，而是避免把请求继续发给已经接近上限的连接。

#### 3. 失败后切换 Connection，而不只是切模型

```text
Provider → Credential Connection → Model
```

仅对下列可恢复错误做有界切换：

- 429；
- 明确 `quota_exhausted`；
- 临时 5xx；
- 连接失败；
- 明确可恢复的鉴权刷新错误。

401、账号禁用、合规拒绝和来源不明的错误必须隔离并告警，不能不断换 Key 掩盖问题。所有切换仍受全局 attempt/timeout budget 约束。

#### 4. 做企业级、无正文的切换审计

Receipt 只记录：

- 选择了哪个非秘密 credential ID；
- 为什么选择或切换；
- 额度证据来自哪里、何时观测；
- cooldown 到什么时候；
- 是否发生 fallback。

仍然不记录 Key、Prompt 或 Response。

TS 明确不应该学：

- OAuth/Web Cookie 抓取；
- 模拟官方客户端身份；
- 账号农场和高频轮换；
- 封禁规避；
- 网页或内部额度接口；
- 把来源不明的账户做成统一资源池。

因此最准确的借鉴结论是：

> 借鉴“同一合规供应商下的多 Credential 连接池、正式配额窗口和有界故障转移”，不借鉴账号聚合、Cookie 抓取、客户端伪装或封禁规避。

这对应 `TS-OMNI-P2-009`：是 **P2 企业韧性能力**，不是当前 P1，不是产品主线，更不是“免费模型战略”。

### 0.2 一句话结论

**OmniRoute 是“大而全的本地 AI 控制平面”，token-station 是“窄而硬的本机 Agent 可信路由层”。** OmniRoute 领先的不是单一算法，而是 Provider/协议覆盖、账户与配额状态、路由策略、故障诊断和可安装分发形成的完整闭环；token-station 真正更有价值的差异化，是默认 loopback + 鉴权、Desktop/默认快速开始把上游 Key 存进 OS 钥匙串、受测产品自有观测面没有正文栏位且 canary 未落入 Metrics/JSONL、路由可重放、首字节前才 fallback，以及无网络/文件权限的 WASM 插件边界。

“90+ free”不能理解成 OmniRoute 自己提供 90 多个免费算力池。固定提交里可数出 **126 个宽松 `hasFree` 标记、81 个 canonical 免费预算 Provider、43 个 headline pool**，三者不是同一集合；其中只有 21 个 headline pool 提供正的可量化月 token 数。fresh install 的 zero-config `auto` 则只预接 OpenCode 与 Felo 两个无 Key 后端，两者都被 OmniRoute 自己的目录标为 ToS `avoid`，Felo 还是明确的逆向网页接口。更准确的产品描述是：

> 聚合大量第三方 Provider，并宣称包含 90+ 免费或有免费额度的来源；免费资格、账号要求、额度、稳定性与条款适用性并未逐项形成同一等级的验证。

因此，正确路线不是追平“290 Provider、19 种策略、MCP/A2A/Memory/MITM”的功能数量，而是：

1. 先修复当前会阻断真实 Agent 使用的 P1 闭环；
2. 只借鉴同一合规 Provider 下的多 Credential、正式配额窗口、有界故障转移，以及路由预览、诊断和分发；
3. 保持 token-station 的隐私、安全和确定性红线；
4. 把定位明确为：**本机 Agent 最可解释、最小权限、可恢复的可信路由层**。

### 0.3 结论总表

| 维度 | OmniRoute | token-station | 判断 |
|---|---|---|---|
| 产品边界 | Provider、账号、OAuth/Web 会话、路由、压缩、媒体、MCP/A2A、Memory、远程与桌面的一体化平台 | 本机 Agent → 本地网关 → BYOK/本地模型的可信路由器 | 不是完全同类；可比较的是 Agent 网关、路由、韧性、观测、分发 |
| Provider/协议覆盖 | 文档口径 290 Provider；88 个 executor；Chat/Responses/Messages/Gemini 与多类媒体端点 | 当前仅 1 个 OpenAI-compatible Chat Completions 出站 Adapter；4 种 Agent 入站 | OmniRoute 明显领先 |
| 免费来源 | “90+”营销标签；源码有 126 个 `hasFree` 标记、81 个预算目录 ID、43 个 headline pool；zero-config 实际预接 2 个高风险无 Key 后端 | BYOK/本地模型，没有把第三方免费权益包装成自有资源池 | OmniRoute 覆盖广，但免费口径异质、漂移且不能直接视为稳定优势 |
| 路由 | 19 种公开策略；账户/配额/成本/延迟/健康/粘性等动态排序 | 规则、Hint、确定性启发式、三档池、精确模型、显式 recovery | OmniRoute 更广；token-station 更可解释、可重放 |
| 韧性 | Provider breaker、connection cooldown 默认工作；model lockout 为 opt-in；另有多账号/配额窗口 | 每 `(upstream, model)` breaker + 有界同请求 fallback | token-station 基础可靠，但缺账户/配额层 |
| 语义保真 | 大量跨协议转换与兼容兜底，覆盖广但状态空间很大 | 已建模的关键语义有拒绝矩阵，错误白名单才重试；部分未建模参数仍会被丢弃 | token-station 边界更窄，但还需补语义覆盖门禁 |
| 隐私/安全默认 | 本地自托管，但默认 `0.0.0.0`、数据面 API Key 关闭；部分响应/推理可落 SQLite | loopback + 鉴权默认；Desktop/默认快速开始用 OS 钥匙串，CLI 另支持 keyring/env/file SecretSource；正文不进 Metrics/Receipt | token-station 明显更强，不能倒退 |
| 安装/升级 | npm、Docker、Win/macOS/Linux 桌面资产、自动更新、SBOM | 当前主要从源码构建；正式升级公钥未注入 | OmniRoute 明显领先 |
| 当前稳定性 | 正式版有高可信启动回归；未发布分支有单用户路由风险信号和本周期历史门禁失败，但固定 HEAD quick gate 已绿 | 本轮自动化测试全绿，但人工测试仍有 Agent 接入阻断 | 两边短板不同：OmniRoute 是广度与发布，token-station 是接入闭环 |
| 最适合借鉴 | Onboarding、合规 Credential/正式配额状态、诊断、路由模拟、分发与 SBOM | — | 应分阶段采纳 |
| 最不应复制 | Provider/免费池数量 KPI、匿名数据面、Web-cookie/MITM/TLS stealth、账号堆叠、默认正文缓存、隐式复杂路由 | — | 明确列为架构红线 |

### 0.4 排名前八的动作

下表是建议的实施顺序，不是仅按严重度排序；同一 Phase 内会把共享 schema/测试夹具的 P2 工作合并交付。

| 顺序 | Finding ID | 动作 | 优先级 |
|---:|---|---|---|
| 1 | `TS-OMNI-P1-001` | 修复 Desktop Provider tool 能力证据不回写，打通已支持的 Agent tool 流量 | P1 |
| 2 | `TS-OMNI-P1-002` | 修复 Claude Code/OpenClaw 真实安装发现与原因级接入诊断 | P1 |
| 3 | `TS-OMNI-P2-006/007` | 修复 Add Provider 重复保存与 southbound URL 预览两个明确 UX 违约 | P2 |
| 4 | `TS-OMNI-P1-003` | 发布签名安装包、首启自检和 fail-closed 更新链 | P1 |
| 5 | `TS-OMNI-P1-004` | 修复核心配置/Agent ownership/snapshot 的备份一致性与失败回滚 | P1 |
| 6 | `TS-OMNI-P2-001` | 增加路由模拟、排除原因、配置生效域和可重放解释 | P2 |
| 7 | `TS-OMNI-P2-008/009` | 补关键原生 Provider 协议，并增加显式预算模式与合规多 Credential/正式配额状态 | P2 |
| 8 | `TS-OMNI-P2-002` | 增加稳定/成本/延迟三个版本化、可解释、可重放的运行态排序预设 | P2 |

---

## 1. 产品边界与可比性

### 1.1 OmniRoute 的真实产品边界

OmniRoute 的 README 将产品描述为安装后即可用的本地 AI 网关，宣称 290 个 Provider、90+ 免费源、19 种路由策略和多类 Agent；源码还覆盖账号 OAuth/Web 会话、Provider 连接池、配额、MCP/A2A、Memory、压缩、媒体端点、远程模式、代理与 MITM 等能力。这里的 290 是官方注册表/生成文档口径，不是 290 个同等能力、同等验证深度的聊天后端。参考：

- [README 产品口径](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/README.md#L8-L24)
- [README zero-config](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/README.md#L152-L167)
- [README 策略与韧性](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/README.md#L238-L377)
- [README 近期能力面](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/README.md#L405-L426)

它面向的主要是愿意管理多账号、多订阅、多路由策略、配额和成本的高级个人或团队操作者，而不仅是一个透明代理。

### 1.2 token-station 的真实产品边界

token-station 当前聚焦：

- 多种 Coding Agent 入站；
- 本地 loopback 网关；
- BYOK 与本地模型；
- 三档/规则/Hint/精确模型路由；
- 对已建模关键语义的拒绝矩阵；
- 无正文的 Request Receipt；
- 可恢复的 Agent 配置接管；
- 无网络/文件权限的 WASM Adapter。

参考 [中文 README](../../README.zh-CN.md) 和 [架构总览](../contributing/架构总览.md)。

### 1.3 哪些可以直接比较

| 可直接比较 | 只可作相邻参考 | 不应视为当前直接竞品范围 |
|---|---|---|
| Agent 接入、Provider 接入、协议转换、模型路由、fallback、健康、预算/成本、日志、桌面分发、安全默认 | 多账号 OAuth、配额窗口、路由策略 UI、远程管理、团队控制面 | MCP/A2A、持久 Memory、媒体生成、浏览器会话抓取、MITM/TLS stealth、VNC/容器/云 Agent |

本报告会把相邻能力放入“借鉴/延后/不复制”三类，而不会把“OmniRoute 有、token-station 没有”一律判为缺陷。

---

## 2. 方法、版本与证据等级

### 2.1 基线

| 对象 | 固定基线 | 说明 |
|---|---|---|
| token-station | `ba813b447...` | 相对 `develop` 只多最新人工测试文档；业务代码基线是 `d8b2939...` |
| OmniRoute 源码 | `ed7db3ee...` | 官方默认分支 `release/v3.8.49`，版本字段 3.8.49，但尚未正式发布 |
| OmniRoute 正式版 | `v3.8.48` / `7ee5bbc...` | 2026-07-13 发布，是普通用户可获得的稳定通道 |

### 2.2 证据等级

| 等级 | 定义 | 本报告用法 |
|---|---|---|
| A | 固定提交源码、实际测试输出、官方 release asset/CI | 可直接下事实结论 |
| B | 官方 README/文档、维护者 issue 诊断、多用户可复现 issue | 需要标注“宣称/外部信号” |
| C | 单用户 issue、搜索摘要、未复现行为 | 只能作风险线索 |
| U | 本轮无法访问/无法运行 | 明确列为未验证，不补猜测 |

### 2.3 研究动作

1. 固定双方提交、分支、版本和正式发布边界。
2. 审计 token-station Router、Gateway、协议、Provider/Agent Adapter、Desktop、插件、Metrics、备份、发布。
3. 审计 OmniRoute 策略、Auto Combo、账户/配额状态、协议执行器、数据持久化、安全默认、分发和门禁。
4. 对“90+ free”做供应链专项：全量计算 `hasFree`/canonical catalog/预算池三套集合，分类凭据来源，并抽查代表性 Provider 的当前官方额度与条款。
5. 运行 token-station Workspace、Desktop Rust、Desktop Frontend 测试。
6. 检查 OmniRoute 官方 release、GitHub issue 和当前 release branch；由于发布包约 200–350 MB、npm 解包约 739 MB，且本轮没有真实账号，未做完整安装与真实上游 E2E。
7. 分别做事实、产品、工程复核。

---

## 3. 全量能力对照

| 领域 | OmniRoute | token-station | 状态判断 | 建议 |
|---|---|---|---|---|
| 安装 | npm/Docker/Win/macOS/Linux 桌面 | 主要从源码构建 | token-station 缺口大 | 立即补 |
| 首次使用 | zero-config `auto` + onboarding + Provider test | Provider/Agent 配置能力存在，但真实安装发现有阻断 | token-station 部分/破损 | 立即补 |
| Provider 目录 | 官方口径 290；含 API Key、OAuth、Web Cookie、No-auth、媒体等多种类型 | 41 个默认直连预设 + 1 个隔离的 OpenRouter 候选；核心仅 OpenAI-compatible | 产品边界不同 | 不追数量，追可信状态 |
| Provider 出站协议 | 多种原生/翻译执行器 | 仅 OpenAI-compatible Chat Completions | 战略缺口 | P2 补关键原生协议 |
| Agent 入站 | README 宣称 33 coding agents | 7 个 registry descriptor；核心 4 种入站方言 | OmniRoute 广；token-station 深度管理更强 | 先修现有 7 个 |
| 精确模型语义 | 模型/Combo/auto 名称空间丰富，也产生心智歧义 | 明确 exact model；只在同模型跨 Provider fallback | token-station 强项 | 保留并强化预览 |
| 路由策略 | 19 公开策略 + 1 内部 quota-share | rule/hint/heuristic/default + ordered recovery | OmniRoute 广 | 只借 3 个元数据预设 |
| 动态评分 | 13 个源码因子、6 个 mode pack；README 仍写 12 | 版本化整数启发式，纯函数、可重放 | 各有优势 | 增加可解释元数据排序 |
| 账户/配额 | 多账号、quota、reset window、connection cooldown | 无账户层；预算仅观察 | 战略缺口 | P2 |
| Provider 健康 | Provider breaker + connection cooldown；model lockout opt-in/default off | 每 `(upstream, model)` breaker | 基础已具备 | 增加 rate-limit/配额状态 |
| 同请求 fallback | Combo 有全局 attempt/timeout、响应质量检查、能力筛选 | 有 attempt budget、Retry-After、错误白名单、首字节边界 | token-station 核心已可靠 | 保留严格分类 |
| Tool 能力 | 目录、能力筛选、转换链丰富 | Core 支持，但 Desktop test 证据不回写 | 已支持主链实际破损 | P1 修复 |
| Structured output | 多协议转换覆盖较广 | Provider test 会探测 JSON，但官方入站当前先拒绝 structured output，未形成端到端链路 | 未闭环 | P2 随协议/conformance 补齐 |
| 多模态 | Chat + embeddings/images/audio/video/OCR 等多端点 | text/image URL/tools；明确拒绝 audio/embeddings | 当前非主链 | P3 按需 |
| 预算 | per-request USD、API key quota、账号 quota、strict budget fallback | 显示/提醒，不影响准入与路由 | observe 是有效现状；控制能力缺失 | P2 可选模式 |
| 观测 | 内容/元数据日志、成本、配额、P95、缓存等广 | 无正文 Receipt/Attempt/Conversion、成本、JSONL | token-station 隐私更强 | 增强关联诊断 |
| 数据隐私 | 本地部署；Memory 可关闭；但 semantic cache 默认开且响应落 SQLite，reasoning replay 也落库 | 正文不进 Receipt/决策；Desktop 默认用 OS keychain，CLI 也允许 env/file SecretSource | token-station 明显更强 | 作为核心承诺 |
| 网络安全默认 | CLI `serve` 等主路径默认 `0.0.0.0`，`REQUIRE_API_KEY=false`，setup 可跳过管理密码 | 非 loopback 拒绝，鉴权默认开启 | token-station 明显更强 | 不复制 |
| 插件 | JS/worker/管理面丰富 | WASM、无网络/FS、Host 签名注入 | token-station 边界更强，UI 更弱 | 补安全插件 UI |
| 备份恢复 | DB/升级/迁移体系广，但状态复杂 | CLI 备份范围不足且 live SQLite raw copy | token-station 真实缺口 | P1 |
| 更新发布 | 多平台资产、自动更新、SBOM；近期有致命发布回归 | 更新验签 fail closed，但公钥未注入、无正式可安装闭环 | 双方各有问题 | 取其分发，保留验签 |
| 团队/远程 | Remote、Scoped tokens、管理 API、团队化能力 | 本机单用户 | 当前非目标 | 延后 |

---

## 4. OmniRoute 深拆

### 4.1 值得借鉴的产品能力

#### A. 从安装到第一次成功请求的闭环

OmniRoute 的 README 将安装、指向本地端点、调用 `auto` 压缩成三步；onboarding 源码则把欢迎、安全、Provider、连接测试、完成拆成明确步骤。这个闭环比“功能已实现但用户仍需理解配置文件与构建链”更接近产品完成度。

借鉴重点不是“匿名免费 Provider”，而是：

- 首启检查；
- 一次 Provider test；
- 一次真实但可控的请求；
- 明确显示成功/失败原因；
- 不要求用户先理解路由内部结构。

#### B. 账户与配额是路由的一等状态

OmniRoute 把 Provider、Connection/Account、Model 分成不同故障层，并把 quota、reset window、account tier、connection pool size 等输入路由。公开策略列表直接包含 `headroom`、`reset-aware`、`reset-window`、`cost-optimized` 等策略：

- [19 种公开策略源码](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/shared/constants/routingStrategies.ts#L1-L21)
- [账户 fallback 策略子集](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/shared/constants/routingStrategies.ts#L51-L61)

源码能证明的是多连接、OAuth/Web Cookie、额度读取、封禁/限流冷却和自动切换机制；它不能证明账号通过日抛号、假信用卡或其他滥用方式获得，也不能把此类用户行为归因给项目。对 token-station 有价值的边界更窄：同一合规 Provider 下、来源可证明的多个企业/项目 Key，其额度不是普通“健康/不健康”，而应成为独立 Connection 状态。

#### C. Auto Combo 的动态排序

Auto Combo 源码当前有 13 个潜在因子：quota、health、inverse cost、inverse latency、task fitness、stability、tier priority/affinity、specificity、context/cache/reset affinity、connection density；同时有 `ship-fast`、`cost-saver`、`quality-first`、`offline-friendly`、`reliability-first`、`chaos-mode` 权重包：

- [Scoring factors 与默认权重](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/services/autoCombo/scoring.ts#L11-L57)
- [候选运行态指标](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/services/autoCombo/scoring.ts#L78-L108)
- [因子计算](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/services/autoCombo/scoring.ts#L204-L259)
- [Mode packs](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/services/autoCombo/modePacks.ts#L13-L111)

值得借的是“把运行态元数据显式变成分数项”，不值得借的是一次性复制全部因子和模式。

#### D. 多层韧性与可恢复诊断

Combo 热路径可见：

- 能力过滤和任务感知重排；
- 无可执行目标时返回诊断与恢复方向；
- 全局 attempt 与 timeout；
- cooldown-aware retry；
- 对 HTTP 200 内容做合法性检查，避免把空/坏响应记成成功；
- 区分请求本身错误与可换 Provider 的错误。

主要源码在 [`open-sse/services/combo.ts`](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/services/combo.ts)。这些理念与 token-station 的错误白名单、首字节边界并不冲突，可以借到更好的诊断层。

#### E. 分发与公开 release evidence

正式版提供 Windows、macOS x64/arm64、Linux x64/arm64、AppImage、deb、源码包和 CycloneDX SBOM。即使当前存在严重回归，这仍证明其分发面远成熟于 token-station。正确借鉴是：

- 多平台矩阵；
- 资产摘要与 SBOM；
- 安装/升级/已有数据库/二次启动 E2E；
- 稳定通道与开发分支严格区分；
- 发布失败公开、可追踪。

#### F. 协议翻译注册表比 Provider 数量更值得研究

OmniRoute 的格式注册表明确覆盖 OpenAI Chat/Responses、Claude、Gemini、Codex 及多种客户端变体，bootstrap 将 request/response 双向翻译独立注册。这种“Translator Registry + 双向 contract test”比把协议判断散进 Provider executor 更值得 token-station 借鉴：

- [Translator formats](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/translator/formats.ts#L1-L12)
- [Translator bootstrap](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/translator/bootstrap.ts#L1-L27)

同时，其 OpenAPI 文档版本仍是 3.8.35，而包版本已到 3.8.49，说明端点广度也需要 spec ↔ route coverage 门禁，不能只看“端点存在”：

- [OpenAPI version](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/public/openapi.yaml#L1-L15)
- [Package version](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/package.json#L1-L4)

### 4.2 OmniRoute 的边界与风险

#### A. Provider 数量不是同一种“支持”

官方检查脚本从生成的 Provider Reference **读取** 290，并统计到 88 个 executor、19 种公开策略、21 个 OAuth Provider。需要注意：脚本注释明确说明 Provider gate 信任生成文档，并不会独立重数源码；Reference 各分类标题手工相加为 289，且其中写的 executor 文件数也与树内实际文件数不一致。290 的集合还混合 API Key、OAuth、Web Cookie、No-auth、本地、搜索、音频、媒体、上游代理、云 Agent 和系统项，不等于“290 个都经过同等深度、同等协议、同等 E2E 验证”。

同时，固定提交的 `package.json` 描述仍写“160+ providers”，README 和 AGENTS 写 290；README 写 Auto Combo 12 factors，而源码接口已有 13 个因子。这类漂移说明：

1. 大目录需要自动生成 provenance 与验证状态；
2. 数量不能作为 token-station 的核心 KPI；
3. “源码存在”不能自动升级为“正式版稳定可用”。

#### B. 路由能力强，但心智模型复杂

[`Issue #7992`](https://github.com/diegosouzapw/OmniRoute/issues/7992) 中，用户认为启用 named combo 会影响 `auto`；维护者最终确认 `model=auto` 走独立 zero-config 路由，named combo 只有请求模型名精确等于 combo 名才生效。它不是已确认代码缺陷，但高置信地证明了“启用”“auto”“combo”生效域不清晰。

对 token-station 的启示：任何策略组都必须在保存前明确展示：

- 生效对象；
- 优先级；
- 哪些 Agent/请求会变化；
- 为什么本次没有命中；
- 指定模型是否绕过智能路由。

#### C. 配额/韧性覆盖没有 headline 看起来那么完整

源码确有账户 fallback、connection cooldown、Provider breaker 和 model lockout，但边界需要保守描述：

- 显式 usage/quota fetcher 的 source-of-truth 只覆盖 37 个 ID/alias；未知 Provider 会返回 `Usage API not implemented`，远少于 290 的目录口径。
- Auto Strategy 的 quota cutoff 在“全部候选都低于阈值”时会返回 429；但只要还有可路由候选，fallback tail 会把 quota-blocked 候选重新加入，源码注释将其定义为 de-prioritize，而不是绝对 hard cutoff。
- Provider breaker、connection cooldown 默认工作；model lockout 是 opt-in，默认 `enabled=false`。Provider-wide cooldown、quota hard preflight、early/mid-stream recovery 等其他关键保护也默认关闭。
- Combo 的 target 层对 non-OK 目标通常继续下一个目标，边界比 token-station 的 request/account/model/provider 错误白名单更宽；不宜复制为默认行为。

证据：

- [Quota usage coverage](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/services/usage.ts#L81-L145)
- [Quota cutoff 与 fallback tail](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/services/combo/resolveAutoStrategy.ts#L294-L312)
- [Fallback tail 重加 quota-blocked candidates](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/services/combo/resolveAutoStrategy.ts#L420-L428)
- [Target non-OK fallback](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/services/combo.ts#L2190-L2193)
- [Resilience defaults](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/lib/resilience/settings.ts#L50-L153)
- [Model lockout default off](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/lib/resilience/modelLockoutSettings.ts#L12-L18)

对 token-station 的正确借鉴是分层状态机、账户选择、Retry-After 和统一健康面板；同时继续保持更严格的错误作用域和全局 attempt budget。

#### D. 本地自托管不等于正文不落盘

OmniRoute 有多项隐私保护：Memory 默认关闭、详细日志可配置、`no-log`/`no-cache` 机制；标准 bootstrap 通常会生成 storage key，并对 API key/token 等 credential 字段做 AES-256-GCM 加密。但这不是 SQLCipher/整库加密，其数据边界与 token-station 不同：

- Semantic cache 默认启用；显式 `temperature: 0` 的响应会进入内存和 SQLite，默认 TTL 30 分钟：
  - [缓存设计与 SQLite 持久化](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/lib/semanticCache.ts#L1-L10)
  - [响应写入 SQLite](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/lib/semanticCache.ts#L225-L253)
  - [缓存默认开启](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/lib/db/migrations/046_database_settings.sql#L16-L21)
  - [只有显式 temperature=0 才缓存，可用 Header 绕过](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/lib/semanticCache.ts#L351-L376)
- Reasoning replay 在包含 reasoning/tool-call 的相应路径中，会把最多 10,000 个 JS UTF-16 code units 的 reasoning 内容 best-effort 写入 SQLite，TTL 2 小时；源码常量虽名为 `MAX_ENTRY_BYTES`，实际使用 `string.length`/`slice`，不是字节上限：
  - [Reasoning cache](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/services/reasoningCache.ts#L137-L220)
  - [DB 写入真实 reasoning](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/lib/db/reasoningCache.ts#L69-L96)
  - [Chat 热路径自动捕获](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/handlers/chatCore.ts#L4252-L4263)
- 详细日志打开后可以保存 client request、translated request、provider/client response：
  - [Detailed log schema 与开关](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/lib/db/detailedLogs.ts#L19-L70)
  - [Payload 持久化](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/lib/db/detailedLogs.ts#L72-L98)
- `.env.example` 把 `STORAGE_ENCRYPTION_KEY` 描述为“整个 SQLite 加密”，但实际实现只用于 credential 字段；无 Key 或加密异常时会原样返回明文，bootstrap 生成/持久化 Key 失败也只警告后继续。Semantic/reasoning cache 的 response/reasoning 列直接写 SQLite，未经过这个字段加密 helper。这是文档与安全语义漂移：
  - [.env 的整库加密宣称](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/.env.example#L48-L56)
  - [实际字段级、fail-open 加密实现](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/lib/db/encryption.ts#L1-L8)
  - [无 Key/异常时保留明文](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/lib/db/encryption.ts#L54-L75)
  - [加解密错误的 fail-open 路径](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/lib/db/encryption.ts#L115-L149)

因此准确结论是：**OmniRoute 是 local-first，但不是 content-free persistence；token-station 当前可证明的差异化，是受测 Chat 成功路径的 canary 未进入 `metrics.sqlite`/`requests.log`，且 RequestRecord/Receipt schema 没有通用正文栏位。** 更广义的“任何磁盘路径都绝不落正文”仍应由持续隐私门禁守住，不能从一次测试外推。

#### E. 网络与鉴权默认不适合直接复制

固定提交的 CLI `serve`/Docker 等已核实主路径默认绑定 `0.0.0.0`，数据面默认不要求 API Key，交互式 setup 允许默认跳过管理密码并关闭 Dashboard 登录：

- [默认 bind 0.0.0.0](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/bin/cli/commands/serve.mjs#L167-L183)
- [REQUIRE_API_KEY=false](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/.env.example#L268-L282)
- [跳过密码后 requireLogin=false](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/bin/cli/commands/setup.mjs#L27-L53)

需要平衡说明：OmniRoute 并非没有安全设计。fresh bootstrap 尚未显式设置 `requireLogin=false` 时，非 loopback management bootstrap 有保护；它还有 LOCAL_ONLY、ALWAYS_PROTECTED、普通 MANAGEMENT 三层 Route Guard，spawn/RCE 类接口会额外限制：

- [三层 Route Guard](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/server/authz/routeGuard.ts#L1-L22)
- [本地进程/插件/MITM/VNC 等高风险路由清单](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/server/authz/routeGuard.ts#L30-L74)
- [鉴权关闭时普通 Management 允许匿名](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/server/authz/policies/management.ts#L236-L247)

结论不是“OmniRoute 无安全”，而是它按可远程自托管平台设计；token-station 按本机 Agent 路由器设计，**默认暴露面不应向 OmniRoute 靠拢**。

#### F. 功能广度正在给稳定通道施压

截至 2026-07-27 的 GitHub 信号：

- [`#7132`](https://github.com/diegosouzapw/OmniRoute/issues/7132)：v3.8.48 Windows Electron 在已有 DB/第二次启动时 500/黑屏，多用户与真实 DB 证据，仍开放。
- [`#7346`](https://github.com/diegosouzapw/OmniRoute/issues/7346)：v3.8.48 macOS，后续 Linux 包同类 `ERR_MODULE_NOT_FOUND`；修复已进 3.8.49，正式包未发布。
- [`#8197`](https://github.com/diegosouzapw/OmniRoute/issues/8197)：单用户报告长上下文/代理相关 503；报告者后续确认 3.8.48 也有相似现象，版本边界、根因和中间网关影响均未确认，只能作为 C 级风险信号。
- [`#8540`](https://github.com/diegosouzapw/OmniRoute/issues/8540)：本周期较早提交出现 release-green HARD failure；固定基线 `ed7db3e...` 的 push-mode `--quick` gate 后来已绿，但固定 HEAD 的 full scheduled/dispatch gate 本轮未验证。
- [`#6778`](https://github.com/diegosouzapw/OmniRoute/issues/6778)：Codex/ChatGPT account 与新模型身份/目录同步问题，仍开放。
- [`v3.8.48 release`](https://github.com/diegosouzapw/OmniRoute/releases/tag/v3.8.48) 明说 v3.8.47 npm 包“每次启动都崩”，当日发布 hotfix。

正向信号同样明确：维护者诊断透明、会修正初始判断、问题响应快、发布资产和 SBOM 完整，并在同一周期把固定 HEAD 的 quick gate 修回绿色。

工程判断：**OmniRoute 是很好的“能力地图”和“运行态设计”参考，但当前不是可以整包照抄的稳定性模板。**

#### G. 测试很多，但巨型编排文件和功能深度仍要逐项验证

OmniRoute 有 test-discovery、anti-masking、SAST/SCA、OpenAPI fuzz、package smoke 等很值得借鉴的门禁；但离线树内也有约 3,900 个 test/spec 文件，不能用数量替代关键场景验证。两个核心热路径文件已经达到数千行，Roadmap 也把拆分排到后续版本：

- [`chatCore.ts` 约 4,900 行](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/handlers/chatCore.ts#L4931-L4938)
- [`combo.ts` 约 3,600 行](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/services/combo.ts#L3640-L3647)
- [后续拆分 Roadmap](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/docs/ROADMAP.md#L21-L32)

另一个可核实的“功能存在但闭环断裂”例子是 CLI 加密备份：create 可生成 `.enc`，restore 却只寻找明文文件名，没有 passphrase/key-file 解密路径，测试也只验证加密文件产生。这提醒 token-station：备份验收必须是 **encrypted create → destroy → restore → integrity**，不能只测产物存在。

- [OmniRoute encrypted backup create/restore](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/bin/cli/commands/backup.mjs#L181-L248)
- [Restore file selection](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/bin/cli/commands/backup.mjs#L426-L465)

### 4.3 “90+ free”免费来源供应链专项

#### A. 先把三套计数拆开

“90+ free”是营销标签，不是源码里一个可重算、可审计的供应链集合。固定提交至少有三套互不相等的口径：

| 口径 | 本轮从固定源码重算 | 它实际表达什么 | 不能据此表达什么 |
|---|---:|---|---|
| Provider UI 的 `hasFree` | 126 个 | 只要 Provider 定义被宽松标为“有某种免费可能”就会进入 Free filter | 126 个稳定、官方、无需账号的免费 LLM API |
| canonical `FREE_MODEL_BUDGETS` | 516 行模型、81 个 Provider ID | headline 计算使用的内部规范化目录；其中 Predibase 已在同一提交的 Provider 定义中标为 deprecated，机械扣除后剩 80 个目录 ID | 与 126 个 UI 标记或 README 的“90+”同一集合；也不等于 80 个已逐项验证可用的渠道 |
| headline `poolCount` | 43 个 recurring/keyless pool | `recurring-daily`、`recurring-monthly`、`keyless` 中有 `poolKey` 的去重池数量 | 43 个都有正额度、均可量化或均适合生产的池 |

43 个 headline pool 中，17 个 `keyless` pool 的 `monthlyTokens` 全是 0；另有 5 个 daily/monthly pool 也是 0。也就是说，**只有 21 个 pool 对 1.526B steady headline 实际贡献正数**。13 个 `recurring-uncapped` Provider 因没有公开 token cap，则被列出但既不进入 43 pool，也不进入 token 总和。

两套 Provider 集合本身也漂移：canonical catalog 有 8 个 ID 没有对应 `hasFree=true`，而 UI 的 126 个 `hasFree` 中有 53 个不在 canonical catalog。CI 会校验 steady/first-month headline token 数是否同步到文档，但没有校验 `poolCount`，也没有从独立集合重算并校验“90+”：

- [README 的 90+、43 pool / 516 model 与 headline](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/README.md#L10-L26)
- [README 的 90+ / 40+ 宣称](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/README.md#L485-L489)
- [免费类型、pool 去重与 headline 计算](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/config/freeModelCatalog.ts#L5-L146)
- [516 行 canonical 数据](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/config/freeModelCatalog.data.ts)
- [UI Free filter 只检查 `hasFree`](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/app/%28dashboard%29/dashboard/providers/providerPageUtils.ts#L320-L323)
- [CI 只校验 headline token 数，不校验 poolCount 或 90+ 集合](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/scripts/check/check-docs-counts-sync.mjs#L271-L300)

正式渠道还会进一步改变数字：最新正式版 `v3.8.48` 的源码实际是 101 个 `hasFree`、456 行/69 个 canonical ID、38 pool、约 1.372B steady；tag 内 README 仍写约 1.6B/2.1B。当前 1.526B 是未发布 `release/v3.8.49` 分支的数字，不应倒灌成正式版能力。

#### B. “免费”实际来自七类外部资源

| 来源类型 | 典型渠道 | 用户通常要提供什么 | 主要限制与风险 |
|---|---|---|---|
| 官方 API recurring free tier | Gemini、Groq、Mistral、Cloudflare Workers AI、Cerebras | Provider 账号、Project/API Key；有时需地区/KYC | RPM/TPM/RPD/组织额度、模型差异、可随时调整；通常不是每个 Key 一份额度 |
| 聚合平台免费模型 | OpenRouter `:free`、Kilo Gateway、部分小型网关 | 聚合平台账号/API Key，少数可匿名 | 上游再路由、模型可用性波动、数据处理链更长；免费模型不等于生产 SLA |
| 一次性赠送/试用 | Vertex/云赠金、AgentRouter、部分 inference host | 新账号，有时要付款方式/KYC | 不是 recurring；有效期和资格限制强，不能计入长期供给 |
| OAuth/订阅权益 | Kiro、部分 Coding Agent/订阅 | 用户自己的登录、OAuth 和有效订阅 | 权益通常限定官方客户端/用途；第三方网关适用性要逐项看条款 |
| Web session/Cookie 适配 | Qwen Web、Muse、T3 等 | 用户自己的网页会话/Cookie | 非正式 API、前端协议漂移、账号和 ToS 风险高，不适合企业默认 |
| Public/keyless endpoint | OmniRoute 对 OpenCode Zen URL 的匿名调用、Felo、Duck.ai、AI Horde 等 | 可能无需 Key；有些仍按 IP、匿名 key 或浏览器状态限速 | 最不稳定；可能不是官方 API，可能禁止自动化或代理使用 |
| 本地/自托管模型 | Ollama、LM Studio 等 | 用户自己的 CPU/GPU、存储和电力 | API 费用为 0 不等于算力成本为 0，也不是 OmniRoute 提供的免费算力 |

126 个 `hasFree` 的源码分类正好说明了异质性：97 个 API-key、9 个 Web-cookie、8 个 No-auth、5 个 Search、4 个 OAuth、2 个 Local、1 个 Audio。canonical 81 个 ID 则是 70 个 API-key、4 个 No-auth、4 个 Web-cookie、3 个 OAuth。更容易误读的是：catalog 的 `keyless` 是**预算类型**而非**鉴权类型**，其中仍混有 OAuth、Web session 或需要账户的渠道。

因此“免费”至少要同时回答五个字段，单个 `hasFree: true` 不够：

```text
source_kind        official_api | aggregator | oauth_entitlement | web_session | public_endpoint | local
credential_kind    api_key | oauth | cookie | anonymous | local
grant_kind         recurring | uncapped_rate_limited | one_time | trial | user_compute
quota_scope        account | organization | project | IP | model | unknown
terms_status       allowed | caution | ambiguous | avoid | unknown
```

#### C. 1.526B 是一套估算，不是可兑现 SLA

固定提交的计算逻辑是：共享 pool 只取其中最大值；daily/月度限额换算为月 token；one-time、uncapped 和 OpenRouter deposit boost 分开。按代码实际计算：

| 指标 | 固定提交结果 |
|---|---:|
| steady recurring | 1,526,225,000 tokens/月 |
| 加 recurring credit | 1,527,225,000 tokens/月 |
| 加 one-time signup credit 的 first month | 2,152,725,000 tokens |
| OpenRouter 一次充值 $10 后的源码 `boostMonthlyTokens` | 源码另列 24,000,000 tokens/月；但按其同一假设这是充值后的总量，净增量应为 22,800,000 |
| `tos=avoid` 排除后 | 1,526,200,000 steady；2,141,700,000 first month；30 pool / 423 model |

这个模型比简单把所有 RPM × 24 × 30 相加克制，但仍有三个重要边界：

1. **高度集中。** Mistral 的源码估算 1.0B 占 headline 65.5%；加 LLM7 150M、Nara 150M 后，前三个 pool 占 85.2%；前七个占 94.6%。任何一个大项变动都会显著改变总数。
2. **token 换算不是合同额度。** 一些 Provider 公布的是请求数、神经元、美元 credit 或模型级 rate limit；将其转换成 token 要依赖平均请求长度、价格和 30 天假设。
3. **排除 ToS `avoid` 几乎不改变 steady，不代表风险很小。** 当前 81 个 Provider ID 中有 15 个 `avoid`、43 个 `caution`、7 个 `ambiguous`、14 个 `ok`、2 个 `unknown`。多数 `avoid` 渠道在账面是 0-token keyless pool，所以风险过滤对总量影响小，却会直接影响 zero-config 可用性。
4. **OpenRouter 行既 mis-keyed，又有 boost 数学标注错误。** canonical 行把 `modelId: "auto"` 记进 free pool，但 OpenRouter 当前官方目录的零价路由是 `openrouter/free`；`openrouter/auto` 的价格是动态 sentinel，不是免费保证。因此这 1.2M 只能视为未验证的目录估值，不能由官方 free 限额反向佐证。即使按项目自己的 800 tokens/request 假设，50 RPD 是 1.2M/月、1,000 RPD 是 24M/月，充值后的净增量也应是 22.8M，而不是 README/源码写的 `+24M`：
   - [OmniRoute 把 `auto` 计入 free pool](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/config/freeModelCatalog.data.ts#L346)
   - [OpenRouter 当前官方模型目录](https://openrouter.ai/api/v1/models)

#### D. zero-config `$0` 只有两个默认后端，且都不是 OmniRoute 出资

源码确实意图让 fresh install 的 `auto` 在没有 Key/账号时返回结果，但普通 Auto Combo 对 No-auth Provider 只 allowlist：

```text
opencode
felo-web
```

同一段注释说明其他 No-auth 候选在项目的参考 VPS 上曾返回 429、403、502 或匿名 key 被拒，因此没有进入默认池：

- [zero-config No-auth allowlist 与参考出口结果](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/services/autoCombo/virtualFactory.ts#L136-L155)
- [No-auth Provider 定义](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/shared/constants/providers/noauth.ts#L5-L160)

两者的准确边界是：

- **OpenCode：** OmniRoute 把 `https://opencode.ai/zen/v1` 当作 No-auth 候选并尝试匿名调用；这只能证明 OmniRoute 的实现选择，不能证明 OpenCode 官方支持匿名 API 用法。OpenCode 当前公开文档主流程反而是登录、添加 billing details、生成 API Key，free models 标为 limited-time。OmniRoute 自己把 `opencode` 标为 ToS `avoid`。[OpenCode Zen 官方文档](https://opencode.ai/docs/zen/)
- **Felo：** executor 注释明确写“没有公开 API”，是 reverse-engineered、scrape-style 的网页接口，前端协议变化即可失效；同样被 catalog 标为 `avoid`。[Felo executor 的自我说明](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/executors/felo-web.ts#L5-L20)

所以 `$0` 的准确含义是：**OmniRoute 预接了两个第三方无 Key 入口，并在它们可用时转发请求。** 它不等于 OmniRoute 提供算力、稳定 SLA、官方集成或长期免费承诺。

#### E. 官方资料抽查已经发现额度与条款漂移

本轮没有创建账号，也没有消耗免费额度；以下是对高贡献或高争议渠道的官方资料抽查，不是对全部 81/126 个来源的 live certification：

| 渠道 | OmniRoute 当前目录/宣传 | 当前官方资料抽查 | 结论 |
|---|---|---|---|
| Mistral | 1.0B/月，是 headline 最大项 | 官方只公开 Free mode；具体组织限额要求登录后看 Limits 页面 | 公开证据不足以独立证明 1.0B；必须按账号实测复核。[官方 usage limits](https://docs.mistral.ai/admin/billing-usage/usage-limits/) |
| LLM7 | 5M/day → 150M/月 | 当前 limits 页写 anonymous 500K/day、free-token 1M/day | 与 150M 估算不一致，catalog 至少已漂移。[官方 limits](https://docs.llm7.io/limits) |
| Nara | 5M/day → 150M/月 | 当前页面显示 7M/day | 这里源码反而保守，但仍证明数字会移动。[官方页面](https://router.bynara.id/) |
| Gemini API | 60M/月 | 需 Google project/API Key；free tier 依模型、项目、地区与账号实际额度，限制不是每个 Key 独立一份 | 是正规免费层，但不能用多 Key 放大同一项目额度。[官方 billing](https://ai.google.dev/gemini-api/docs/billing)、[rate limits](https://ai.google.dev/gemini-api/docs/rate-limits) |
| Groq | 15M/月 | 需账号/API Key；RPM/RPD/TPM/TPD 随模型和组织变化 | 是正规免费层，额度应读取官方 header/控制台，不能硬编码为统一值。[官方 rate limits](https://console.groq.com/docs/rate-limits) |
| Cloudflare Workers AI | 30M token 等价估算 | 官方免费 allocation 是 10,000 neurons/day，需 Cloudflare 账号/token | neuron 与 token 不能无条件等价，换算只是估算。[官方 pricing](https://developers.cloudflare.com/workers-ai/platform/pricing/) |
| OpenRouter | 把 `auto` 记为 free pool 的 1.2M/月；源码又把充值 $10 后的 24M/月总量另记为 `+24M boost` | 官方零价路由是 `openrouter/free`，不是 `openrouter/auto`；免费模型 20 RPM，累计购买不足 $10 时 50 RPD，达到后 1,000 RPD | 1.2M 行是 mis-keyed/unverified；24M 是充值后的估算总量，按同一假设净增应为 22.8M。[官方 models](https://openrouter.ai/api/v1/models)、[limits](https://openrouter.ai/docs/api/reference/limits)、[FAQ](https://openrouter.ai/docs/faq) |
| Kiro | 50 credits/月，目录折 25K tokens；`tos=avoid` | 官方明确订阅只用于 Kiro IDE/CLI/Web/ACP/automation，第三方 harness 把请求路由出原生接口不允许 | 这是用户订阅权益，不是通用 Provider 免费 API。[官方说明](https://kiro.dev/docs/billing/related-questions/) |
| Together | canonical 仍计 25M one-time | 同一提交的 Provider 定义已写 former $25 retired、`hasFree=false`；官方要求至少购买 $5 credits | 已确认 stale overcount。[冲突源码](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/config/freeModelCatalog.data.ts#L478)、[官方 billing](https://docs.together.ai/docs/billing-credits) |
| Predibase | canonical 仍计 25M one-time | 同一提交已标 deprecated，理由是服务域名不再解析 | 已确认 catalog 没有及时清理；81 个 canonical ID 中至少一个不应算当前可用渠道。[冲突源码](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/shared/constants/providers/apikey/inference-hosts.ts#L255-L270) |

另有一些 keyless 服务可以合法匿名使用，但仍有严格边界。例如 AI Horde 匿名请求优先级最低，高负载时会受限；Kilo Gateway 的匿名免费模型按 IP 限速，Auto Free 还可能把数据交给允许日志/训练的上游；Duck.ai 条款则禁止自动查询、规避限制或基于服务再提供 AI 服务。它们不能因为“无需 Key”就被统一归类为企业可用的公共 API：

- [AI Horde 官方站](https://www.aihorde.net/)
- [Kilo Gateway authentication](https://kilo.ai/docs/gateway/authentication)
- [Kilo Gateway models/providers](https://kilo.ai/docs/gateway/models-and-providers)
- [Duck.ai 隐私与使用条款](https://duckduckgo.com/duckai/privacy-terms)

#### F. 旧版 Free Tiers Guide 不能作为当前事实依据

固定提交仍保留一份 `FREE-TIERS-GUIDE.md`，其中写有“50+”“unlimited free AI”“Kiro/Cloudflare/Qoder no auth”“OpenCode/Pollinations unlimited”“all official”“production-ready”“no catch”，并直接建议创建多个账号增加额度。它与当前 canonical catalog、自身 ToS 标记和多家官方资料冲突：

- [过时的 unlimited/no-auth 表格](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/docs/getting-started/FREE-TIERS-GUIDE.md#L15-L55)
- [过时的 production/no-catch/multi-account 建议](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/docs/getting-started/FREE-TIERS-GUIDE.md#L198-L258)

这份文档能证明项目曾把“叠加免费额度”当成卖点，**不能证明用户账号的具体来源**。源码能确认的是：

```text
大量异质 Credential/Account
→ 读取余额或套餐窗口
→ 429、额度耗尽或连接被暂时锁定
→ cooldown
→ 自动切换下一个 Connection
→ 重置后重新加入池
```

例如 OpenCode executor 确实实现了本地 slot、可选 proxy、轮询、429 cooldown 与换槽，但这里的 `fingerprint` 是 UI 用 `crypto.randomUUID()` 生成的本地 slot ID，不会作为账号身份发给上游。只有可选 proxy 能区分网络出口；多个没有 proxy 的 slot 仍走同一个匿名直连出口，不能据此证明存在多个真实账号：

- [UI 生成本地 slot ID](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/src/app/%28dashboard%29/dashboard/providers/%5Bid%5D/components/NoAuthProviderControls.tsx#L114-L120)
- [OpenCode slot 与可选 proxy 绑定](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/executors/opencode.ts#L108-L141)
- [轮询与 cooldown](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/executors/opencode.ts#L93-L173)
- [429 后切换下一个 slot](https://github.com/diegosouzapw/OmniRoute/blob/ed7db3ee5f89a144b2d931d8605534522f83de30/open-sse/executors/opencode.ts#L187-L230)

但源码不能坐实“日抛号、假信用卡、账号农场或封禁规避”，也不能把这些可能的用户行为直接归因给项目。本报告不作该推断。

#### G. 对 token-station 的结论：只借合规 Connection 层，不借免费堆叠

token-station 不应把 `hasFree` 当作自动路由真值，也不应复制 OAuth/Web Cookie 抓取、客户端身份模拟、网页内部额度接口、来源不明账号池、高频轮换或封禁规避。最多可以把官方免费层显示为**参考元数据**，并同时带上 `credential_kind`、quota scope、证据 URL/更新时间、ToS 状态与 `unknown`，不得把估算值包装成可兑现余额。

真正可借鉴的是 P2 企业韧性能力：

```text
Provider
  ├─ Credential A：企业主 Key / owner / project / budget scope
  ├─ Credential B：同一组织授权的备用 Key
  └─ Credential C：独立项目与独立预算
```

- 只接组织明确授权的 API Key/OAuth client，Desktop 默认进 OS Keychain；
- 只从正式 Usage API、标准 rate-limit header 或管理员配置读取额度；
- 增加 `Provider → Credential Connection → Model`，但只对 429、明确 quota exhausted、临时 5xx、连接失败和可恢复 refresh error 做有界切换；
- 401、账号禁用、合规拒绝或条款冲突要隔离并告警，不能不断换 Key 掩盖；
- Receipt 记录 credential ID、选择/切换原因、额度证据来源、cooldown 截止时间和 fallback，不记录 Key、Prompt 或 Response。

因此 Roadmap 的准确措辞应保持为：

> 借鉴“同一合规供应商下的多 Credential 连接池、正式配额窗口和有界故障转移”，不借鉴账号聚合、Cookie 抓取、客户端伪装或封禁规避。

这是 `TS-OMNI-P2-009`，不是当前 P1，也不是 token-station 的产品主线。

---

## 5. token-station 现状映射

### 5.1 已经做对、必须保留的部分

#### A. 清晰的本地信任边界

- 默认 loopback，非 loopback 配置拒绝启动；
- 虚拟 Key 鉴权默认开启；
- 上游 Key 优先存 OS 钥匙串；
- Provider Adapter 不持有凭据，只提交受限请求描述；
- Host 校验 Origin/Path 后注入凭据；
- 插件不能直接访问网络和文件系统。

证据：

- [config.rs](../../apps/cli/src/config.rs)
- [protocol HTTP credential boundary](../../crates/protocol/src/http.rs)
- [plugin-api manifest](../../crates/plugin-api/src/manifest.rs)
- [plugin-runtime](../../crates/plugin-runtime/src/runtime.rs)

#### B. 确定、可重放的路由

Router 是纯函数，按 rule → hint → heuristic → default 决策；精确模型只允许同模型跨 Provider fallback；健康状态只在已选 pool 内排序或临时摘除候选，不把用户指定池偷偷替换成别的池。

证据：

- [route.rs](../../crates/router-core/src/route.rs)
- [config.rs](../../crates/router-core/src/config.rs)
- [decision.rs](../../crates/router-core/src/decision.rs)

#### C. 同请求 fallback 的语义边界

Gateway 有全局 attempt budget、请求 timeout、Retry-After 有界等待、错误分类白名单；鉴权/参数/能力等错误不会跨 Provider 重放；流一旦提交首字节，就不再换上游；cancel 不惩罚 Provider 健康。

这不等于所有未建模语义都会 fail closed。OpenAI/Gemini 等入站只映射 Canonical IR 的一部分字段，部分 `extensions` 为空；OpenAI-compatible Provider 也不会转发任意 canonical extensions。对 `presence_penalty`、`frequency_penalty`、`logprobs`、`n`、`seed` 等未纳入显式拒绝矩阵的参数，当前可能静默丢弃。应保留现有关键拒绝矩阵，同时用字段覆盖表和 golden conformance 消除这些盲区。

证据：

- [gateway.rs](../../apps/cli/src/gateway.rs)
- [protocol error.rs](../../crates/protocol/src/error.rs)
- [OpenAI inbound](../../plugins/official/agent-openai/src/lib.rs)
- [Gemini inbound](../../plugins/official/agent-gemini/src/lib.rs)
- [OpenAI-compatible outbound](../../plugins/official/provider-openai-compatible/src/lib.rs)

#### D. 无正文观测

Receipt/Attempt/Conversion 只设计了闭集元数据、决策、耗时、状态、usage 与成本估算，没有通用正文栏位；本轮受测 Chat 成功路径还用 canary 验证了正文没有进入 `metrics.sqlite` 或 `requests.log`。这个结论不字面覆盖 OS swap、crash dump 或未来新增 diagnostics 等全部磁盘路径，因此仍需用 `AC-PRIV-*` 持续守住。

证据：

- [metrics](../../crates/metrics/src/lib.rs)
- [filelog](../../apps/cli/src/filelog.rs)
- [proxy integration tests](../../apps/cli/tests/proxy.rs)
- [RecentReceipts](../../apps/desktop/src/components/RecentReceipts.tsx)

#### E. Agent 配置接管的安全工程

token-station 已有：

- 变更计划、确认、revision/CAS；
- ownership HMAC；
- 加密快照；
- TOCTOU 重检；
- 原子写与失败回滚；
- 只恢复自己拥有的字段；
- symlink/权限拒绝。

这比单纯“帮用户改配置文件”更接近可信配置管理，应继续作为壁垒。

### 5.2 已确认缺口

本节只映射当前实现与本地证据；Finding 的唯一 canonical 定义、状态、修复方向和验收入口统一放在 §9，避免两处定义后续漂移。

#### P1 摘要：Desktop Provider tool 能力证据断链

**状态：BROKEN / P1 / 高置信。**

Desktop 新增 Provider 时，所有模型的 tool/json_schema/vision 都是 `unknown`；`/models` live discovery 只会补 vision；分层 Provider Test 会真实探测 stream/tool/json，但结果只用于展示，不写回模型能力；UI 也只允许手工切 vision。Router 对 tools 必须有 positive evidence，因此“通过 Provider Test”之后，已支持 Agent 的工具调用仍会 fail closed。

JSON probe 结果不回写也是同一条证据链缺口，但四个官方入站 Adapter 当前都不会产生 `response_format=Some`，OpenAI Chat/Responses 还会在 normalize 阶段拒绝 structured output；所以它是 `TS-OMNI-P2-003/008` 的未闭环能力，不能算作已证明的当前 P1 Agent 流量阻断。

证据：

- [Desktop add flow](../../apps/desktop/src-tauri/src/lib.rs)
- [model_catalog.rs](../../apps/desktop/src-tauri/src/model_catalog.rs)
- [ProviderModelManager.tsx](../../apps/desktop/src/components/ProviderModelManager.tsx)
- [Router capability gate](../../crates/router-core/src/route.rs)
- CLI 已能声明能力：[main.rs](../../apps/cli/src/main.rs)、[manage.rs](../../apps/cli/src/manage.rs)

这不是“比 OmniRoute 少一个高级功能”，而是当前 Desktop Agentic 主链被阻断。

对应 Finding：`TS-OMNI-P1-001`。

#### P1 摘要：真实 Agent 安装发现与诊断仍会阻断

**状态：BROKEN / P1 / 高置信。**

最新人工测试确认：

- Claude Code 官方 `~/.local/bin/claude` 因最小环境未恢复 `HOME` 而版本探测失败；
- OpenClaw 在自定义 npm global prefix/Node runtime 下无法发现；
- UI 标题仍把不同根因统一成“暂不可接入”，虽然详情比上一版更好。

证据：

- [人工测试重大体验问题](../../人肉测试重大体验问题.md)
- [discovery.rs](../../apps/desktop/src-tauri/src/agent_integration/discovery.rs)
- [AgentRoutePage.tsx](../../apps/desktop/src/pages/AgentRoutePage.tsx)

对应 Finding：`TS-OMNI-P1-002`。

#### P1 摘要：没有正式可安装/可升级闭环

**状态：MISSING / P1 / 高置信。**

当前快速开始仍以 clone/cargo build 为主；正式 release public key 未注入时，升级命令按设计拒绝。2026-07-27 查询 GitHub Releases，`v0.1.0`、`v0.2.0`、`v0.2.1`、`v0.3.0` 全部仍是 Draft，`publishedAt` 为空；即使 tag/草稿资产存在，也不是普通用户可获得的正式 release。Fail closed 是对的，但“安全地不能升级”还不是可交付分发。

证据：

- [README 快速开始](../../README.zh-CN.md)
- [upgrade.rs](../../apps/cli/src/upgrade.rs)
- [桌面发布文档](../release/桌面App构建签名与发布.md)
- [token-station GitHub Releases](https://github.com/GlimpseEngine/token-station/releases)

对应 Finding：`TS-OMNI-P1-003`。

#### P1 摘要：核心备份/恢复不是一致性事务

**状态：BROKEN / P1 / 高置信。**

CLI 备份只复制 config 和可选 `metrics.sqlite`，未包含 Agent ownership 与加密快照，也没有描述其 OS-keychain master-key/同机恢复边界；live SQLite 使用 raw copy，尽管项目已有 online snapshot helper；恢复先替换 config 再复制 metrics，失败时可能留下混合版本状态。Receipt/Attempt/Conversion 表与 metrics 共用同一个 `metrics.sqlite`，所以选择备份 metrics 时它们会一起被复制，但 raw copy 仍不保证在线一致性。备份目录可复用且本轮没有 metrics 时，旧 `metrics.sqlite` 也不会被清除，之后可能恢复陈旧历史。

P1 只要求 config、Agent ownership 与加密 snapshot 形成可验证的恢复单元，并清楚声明密钥可用性/跨机限制。Revision sidecar 缺失或 fingerprint 变化时当前实现会按外部编辑自愈，Provider tombstone 也不是 active config 的运行必需；二者应先定义恢复语义，而不是一律强制恢复。Metrics/receipts 属于可选历史；catalog cache、诊断和插件索引可以重建；外部已安装插件只要求在 manifest 中记录清单与 hash，缺失时明确告警。

证据：

- [backup.rs](../../apps/cli/src/backup.rs)
- [store online backup helper](../../apps/cli/src/store.rs)
- [Desktop recovery](../../apps/desktop/src-tauri/src/recovery.rs)

对应 Finding：`TS-OMNI-P1-004`。

#### P2 摘要：Provider 出站协议与模态过窄

**状态：PARTIAL / P2（原生协议）+ P3（audio/embeddings/media）/ 高置信。**

当前唯一 Provider Adapter 是 OpenAI-compatible Chat Completions。北向已有 OpenAI Chat、Responses、Anthropic Messages、Gemini 等 Agent 方言，但南向全部落到同一 Chat Completions 管线；README 也将 Anthropic/Gemini 原生 Provider Adapter 列为后续计划。Audio/embeddings 被 Gateway 明确拒绝，canonical stream 还没有原生 reasoning delta。

证据：

- [plugins.rs](../../apps/cli/src/plugins.rs)
- [official OpenAI-compatible Provider](../../plugins/official/provider-openai-compatible/src/lib.rs)
- [canonical chat IR](../../crates/protocol/src/chat.rs)
- [canonical stream IR](../../crates/protocol/src/stream.rs)
- [gateway refusals](../../apps/cli/src/gateway.rs)

对应 Finding：`TS-OMNI-P2-008`。

#### P2 摘要：预算是展示，不是约束

**状态：PARTIAL / P2 战略能力缺口 / 高置信。**

配置和实现都明确说明 Agent 预算仅用于展示/观察，永远不影响 admission/routing；`AttemptBudget.max_cost` 在实际请求构建中也未赋值。账单对账仍是未来项。Observe-only 本身是有效且安全的产品选择；缺口是产品若要承诺“控制成本”，还没有显式的 soft-route/hard-stop 和账户配额语义，不能把它称为当前主链破损。

证据：

- [CLI config budget note](../../apps/cli/src/config.rs)
- [budget.rs](../../apps/cli/src/budget.rs)
- [gateway AttemptBudget](../../apps/cli/src/gateway.rs)
- [metrics billing note](../../crates/metrics/src/lib.rs)

对应 Finding：`TS-OMNI-P2-009`。

### 5.3 重要但次一级的缺口

| Finding ID | 缺口 | 状态 | 影响 |
|---|---|---|---|
| `TS-OMNI-P2-001` | Router 高级规则大多在 Desktop 只读/JSON 展示，没有模拟器和生效域预览 | PARTIAL | 用户无法判断为何命中/未命中 |
| `TS-OMNI-P2-002` | 缺少基于健康、可信价格和延迟元数据的版本化、可解释自适应排序 | MISSING | 现有确定性静态路由无法表达稳定/成本/延迟三种运行态偏好 |
| `TS-OMNI-P2-003` | 模型发现只有 OpenAI `{data:[{id}]}` 形状，主要补 vision，无法形成 tool/JSON/context provenance | PARTIAL | 目录“发现了模型”不等于“可用于 Agent” |
| `TS-OMNI-P2-004` | Agent compatibility 只有 Builtin blocklist；缺失 descriptor 会补空规则，未知/不可解析版本可默认准入 | PARTIAL | 无法独立更新，也容易把“未阻断”误读为“已验证” |
| `TS-OMNI-P2-005` | Plugin Desktop 只能查看；无 install/update/rollback UI | PARTIAL | 强安全架构没有形成产品闭环 |
| `TS-OMNI-P2-006` | Add Provider 检测重复后提示“保存会更新”，但提交仍调用 add，Backend 拒绝重复 ID | BROKEN | 明确 UX 违约 |
| `TS-OMNI-P2-007` | Provider URL 预览展示 `/chat/completions`、`/responses`、`/messages` 三个“最终 URL”，实际唯一出站 Adapter 只用 Chat Completions | BROKEN | 明确误导用户判断协议能力 |
| `TS-OMNI-P2-010` | Provider 预设验收文档仍写 52，当前源码是 41 个默认预设 + 1 个隔离候选 | PARTIAL | 当前验收材料失真 |
| `TS-OMNI-P2-011` | CLI config 变更通常需重启；Desktop 通过显式 apply/restart 补偿 | PARTIAL | 运行态运维不连续 |
| `TS-OMNI-P3-001` | token 估算是 4 ASCII 字符/1 非 ASCII 字符的粗略规则 | PARTIAL | 长代码、JSON、混合语言可能选错档 |

---

## 6. 该借鉴、改造借鉴与明确不复制

### 6.1 直接借鉴原则

| 能力 | 借鉴内容 | 不改变的 token-station 红线 |
|---|---|---|
| Onboarding | 首启检查、Provider test、第一次成功请求、可操作失败原因 | 不自动开放匿名 Provider，不跳过鉴权 |
| Credential/配额状态 | Provider/Connection/Model 三层状态、Retry-After、quota reset | 只接组织授权的 Key；只信正式 Usage API/标准响应头；记录 owner/purpose/budget scope；不抓 Cookie |
| 路由诊断 | 候选排除原因、attempt order、terminal reason、recovery hint | 不记录 prompt/response |
| 分发 | 多平台资产、签名、SBOM、existing-state/second-launch E2E | 更新必须验签，失败拒绝安装 |
| 目录 | 来源、更新时间、已测试、已成功请求、stale/tombstone | 不以数量替代验证深度 |
| Release triage | 公开历史 gate failure、复现数据、修正初始判断、稳定/开发通道分开 | 不以“修复已 merge”或 quick gate 绿色冒充 full release 已验证 |

### 6.2 改造后借鉴

#### 三个而不是十九个自适应预设

首批只做：

1. `稳定优先`：health、近期成功、错误率；
2. `成本优先`：可信价格版本、预算余量；
3. `延迟优先`：TTFT/P95、健康。

约束：

- 用户指定模型、规则、严格本地、能力/上下文 gate 永远优先；
- 权重版本化；
- 输入只用闭集元数据；
- Receipt 输出每项分数；
- 同配置、同健康快照、同请求特征结果一致。

#### 预算模式

从单一 ObserveOnly 改为显式三态：

- `observe`：默认，只提醒；
- `soft-route`：超阈值时只切到用户批准的低成本目标；
- `hard-stop`：返回稳定 402/预算错误，不静默替换模型。

未定价模型必须是“未知”，不能按 `$0` 处理。

### 6.3 明确不复制

| 不复制项 | 原因 |
|---|---|
| 以 Provider 数量为北极星指标 | 支持深度、协议、验证状态不等价；会推高维护状态空间 |
| 默认 `0.0.0.0` + 数据面免 Key | 与本机 Agent 路由器威胁模型冲突 |
| 默认正文/Reasoning 持久缓存 | 破坏“正文不落盘”的核心承诺 |
| Web-cookie、客户端身份伪装、TLS stealth、MITM/TPROXY | ToS、凭据、攻击面与维护成本远超当前产品边界 |
| 账号农场、高频轮换、封禁规避或来源不明 Credential 池 | 不属于合规企业韧性；源码也不能证明 OmniRoute 的账号来源，不能把用户滥用归因给项目 |
| Fusion/Judge、模型生成 handoff 默认进入热路径 | 增加成本、延迟、正文处理和不可重放性 |
| 大量隐式策略与“启用即全局生效” | 容易复现 `auto` vs named combo 的心智错误 |
| MCP/A2A/Memory/媒体全家桶并行开发 | 会在核心 Agent 接入、分发和恢复未稳时稀释资源 |

未来架构约束：如果以后增加 LAN/remote 模式，必须使用独立开关、显式风险确认、精确 Scope 和审计记录；这不是当前本机模式的 Roadmap 交付项。

---

## 7. 隐藏、未完成与容易漏掉的问题

### 7.1 token-station “自动化全绿”仍不代表产品主链全绿

本轮所有自动化测试最终通过，但 Desktop Provider tool 能力证据断链、Claude/OpenClaw 真实安装发现问题都不在普通单元测试的幸福路径中；JSON probe 也尚未形成未来 structured-output 端到端证据。必须补：

- Desktop add → tool test → evidence persisted → Router admits tool 的 E2E；
- Phase 2 按 `AC-PROTO-003` 补 supporting inbound → Provider 的 structured-output conformance；
- 临时 HOME、自定义 npm prefix、外置 Node runtime 的 Agent discovery E2E；
- 从签名安装包首启，而不是只从源码/测试 harness 验证。

### 7.2 7 个 Agent descriptor 不等于 7 个已验证兼容 Agent

当前 registry descriptor 有 7 个；Builtin compatibility 会为缺失 descriptor 自动补空 blocklist，所以最终 entry 数与 registry 一致，不存在简单的“5 对 7 功能错配”。真正的问题是 `CatalogSource` 只有 Builtin，而且 blocklist 模型会把 `99.0.0`、无 SemVer 等未阻断版本在 runnable + preflight 成功时判为可连接。人工测试也证明“识别出 Agent 名字”不等于“该版本已经过验证”。产品文案应区分：

- Descriptor present；
- Installation discovered；
- Version tested；
- Connector preflight passed；
- Successfully connected；
- Real request succeeded。

### 7.3 Provider test 通过不等于 Agent 请求可路由

这是当前最容易误判的缺口：Provider test 已探测 tool/json，但状态没有进入模型能力。对 tool 请求，Router 因缺少 positive evidence 而正确地 fail closed；对 structured output，请求目前会更早被官方入站 Adapter 拒绝。验收必须分别检查“已支持的 tool 测试结果成为可信能力证据”和“未来 structured-output 链路有完整 conformance”，而不只是 UI 显示绿色。

### 7.4 备份不能只围绕 config/metrics

随着 Desktop Agent ownership、快照、revision ledger、tombstone 和目录缓存增加，状态已经跨越多个文件；Receipt/Attempt/Conversion 与 metrics 表则共用同一个 `metrics.sqlite`。若继续把 backup 理解为“直接复制 config + metrics”，恢复后可能出现：

- Agent 配置已接管但 ownership/snapshot 丢失；
- 加密 snapshot 存在但恢复环境没有对应 master key；
- 插件声明与安装状态不匹配；
- metrics/receipts SQLite raw copy 处于不一致状态。
- 复用备份目录时，已经不存在的 live metrics 没有清掉旧备份 DB，之后误恢复陈旧历史。

恢复契约应分三层：config、ownership、加密 snapshot 形成核心事务，并声明 key 的同机/跨机边界；metrics/receipts 作为可选历史使用 online snapshot；catalog cache、诊断和插件索引允许重建，外部插件仅校验 inventory/hash 并报告缺失。Revision ledger 当前能把缺失/fingerprint 变化视为外部编辑后自愈，tombstone 也不是 active config 必需项；二者应明确“保留旧 identity 还是生成新 revision”的语义，而不是直接视作 P1 必备字节。

### 7.5 OmniRoute 的“修复已合入”不等于正式用户已修复

`#7346` 的修复已进入 3.8.49，但最新正式版仍是 3.8.48；同一开发周期又出现 `#8197` 的未定性风险信号和 `#8540` 记录的历史 HARD failure，随后固定 HEAD quick gate 已恢复。竞品分析和自己的发布管理都必须同时跟踪：

- 代码分支状态；
- CI 状态；
- 已签名 release；
- 用户可获得版本；
- 升级/已有状态 E2E。

---

## 8. 有界运行与测试证据

### 8.1 token-station

| 命令/动作 | 结果 | 判定 |
|---|---|---|
| `cargo test --workspace` | PASS；Workspace 全套 Router/Protocol/Gateway/Plugin/Conformance/Metrics/Release 测试通过，仅声明的 ignored case | 已验证 |
| 干净副本 `npm ci` | 成功；717 packages，0 vulnerabilities；有 React peer 和 Node engine warning | 环境可安装 |
| 干净副本 `npm test -- --run` | 20 files / 153 tests PASS | 已验证 |
| `scripts/prepare-desktop-test-plugins.sh` | 5 个测试插件准备成功 | 必需前置 |
| 干净副本 `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml` | 173 lib PASS + 3 regression PASS；1 个真机探测 ignored；0 failed | 已验证 |
| 原工作区首次 Frontend test | 129 tests PASS，3 suite 因现有 `node_modules` 缺声明依赖 `@lobehub/icons` 未加载 | 已在干净 `npm ci` 后排除为本机依赖状态，不记产品 bug |
| Desktop Rust 未准备 `plugins-dist` 的首次运行 | 171 PASS，2 个依赖运行时插件的测试失败 | 按项目规定准备测试插件后全绿，不记产品 bug |
| 2026-07-22/23 人工 Agent 回归 | Claude Code/OpenClaw 真实安装发现仍阻断；通用状态文案仍不足 | 已记录产品 P1 |

### 8.2 OmniRoute

| 动作 | 结果 | 判定 |
|---|---|---|
| 固定官方 default branch | `release/v3.8.49` @ `ed7db3ee...`，`package.json` version 3.8.49 | 源码基线 |
| 固定正式 release | `v3.8.48` @ `7ee5bbc...`，2026-07-13 | 正式版基线 |
| 官方 docs count gate | 报告 290 Provider、19 routing strategies、88 executors、21 OAuth Provider；4 项 soft 漂移。Provider gate 只信任生成的 Reference，人工分项复核仍发现 289/290 与 executor 数不一致 | 生成文档检查 + 源码复核 |
| `node bin/omniroute.mjs --help` in source clone | 因未安装 `update-notifier` 失败 | 只说明 source checkout 未装依赖，不记产品 bug |
| npm registry metadata | `v3.8.48` 解包约 739 MB | 因体积与无真实账号，本轮未完整安装 |
| GitHub release/assets/issues | 多平台资产/SBOM确认；代表性启动/路由/release-green issue 已审阅 | 外部信号 |
| 真实 Provider/Agent E2E | 未执行 | 未验证，不能把 README 宣称写成实测 |

---

## 9. Finding Register

### 9.1 P1

#### `TS-OMNI-P1-001` Desktop Provider tool 测试未形成可路由证据

- 状态：BROKEN
- 影响：Desktop 新增的 Provider 无法承接已支持 Agent 的 tool 请求。
- 根因：新增/发现/测试/UI/Router 五段 tool 能力状态未闭环。
- 修复方向：定义 capability evidence schema；Provider tool test 成功后写入带来源、时间、测试版本的证据；失败不得覆盖更强手工/官方证据。JSON/stream 证据可复用同一 schema，但不扩大本 P1 的影响口径。
- 验收：Desktop 完成新增和 test 后，tool 请求能路由；删除/过期/失败证据后重新 fail closed；Receipt 显示证据来源。

#### `TS-OMNI-P1-002` Agent discovery 与原因级诊断未闭环

- 状态：BROKEN
- 影响：官方 Claude Code 与自定义 npm prefix OpenClaw 无法接入。
- 修复方向：安全恢复必要 HOME/runtime 环境；增加手工选择路径与额外扫描根；reason code/action matrix。
- 验收：见 §12 `AC-AGENT-*`。

#### `TS-OMNI-P1-003` 可安装、首启、升级链未交付

- 状态：MISSING
- 影响：用户必须具备 Rust/Node/源码构建知识；安全升级不可用。
- 修复方向：签名 macOS 包优先；首启自检；发布公钥；可复现资产、SBOM、existing-state/second-launch E2E。
- 验收：见 §12 `AC-REL-*`。

#### `TS-OMNI-P1-004` 核心备份恢复非一致性事务

- 状态：BROKEN
- 影响：恢复后可能形成混合版本与不可逆 Agent ownership 丢失。
- 修复方向：config、Agent ownership、加密 snapshot 使用版本化 manifest，声明 master-key/跨机边界并执行 stage → verify → atomic switch → rollback；可选 metrics/receipts 使用 SQLite online backup；revision/tombstone 先定义 identity 语义；可重建状态不阻断恢复。
- 验收：见 §12 `AC-BACKUP-*`。

### 9.2 P2

#### `TS-OMNI-P2-001` 缺少路由模拟与生效域

- 状态：PARTIAL
- 修复方向：纯 dry-run API；输出命中层、主目标、fallback、排除原因、预算/健康快照、配置 revision。
- 验收：dry-run 与实际 Receipt decision 一致；不访问上游；不记录正文。

#### `TS-OMNI-P2-002` 缺少可解释的运行态排序

- 状态：MISSING
- 修复方向：稳定/成本/延迟三个固定预设；权重版本化；只用元数据。
- 验收：确定性、可显示分数、显式规则优先。

#### `TS-OMNI-P2-003` 模型目录缺 capability provenance

- 状态：PARTIAL
- 修复方向：声明/实时发现/主动测试/成功请求/手工覆盖分层，带 `observed_at` 和 stale。
- 验收：每项能力都有来源；冲突按公开优先级解决；unknown 不冒充 false/true。

#### `TS-OMNI-P2-004` Agent compatibility 不能独立更新

- 状态：PARTIAL
- 影响：Builtin blocklist 会自动补齐缺失 descriptor，但“未命中阻断规则”可把未知/不可解析版本显示为 verified，用户难以区分“未阻断”和“实测通过”。
- 修复方向：签名、版本化、可回滚 compatibility feed；离线内置表兜底；把 blocked、unknown、tested 分开。
- 验收：未知 feed/签名/schema 不替换旧 feed；未知 Agent 版本不冒充实测版本；7 个 Agent 的 registry/catalog 状态可解释。

#### `TS-OMNI-P2-005` Plugin 安全能力没有 Desktop 产品闭环

- 状态：PARTIAL
- 修复方向：只对通过 manifest/conformance/签名验证的包开放 Desktop install/update/rollback；展示权限与 receipt hash。
- 验收：更新失败不破坏旧版本；依赖/方言冲突在写入前阻断。

#### `TS-OMNI-P2-006` Add Provider 重复保存 UX 违约

- 状态：BROKEN
- 修复方向：检测已存在时切换到 update 或明确导航详情页，不能继续调用 add。
- 验收：同 ID 保存不会得到 Backend duplicate；UI 文案与实际动作一致。

#### `TS-OMNI-P2-007` Provider URL 预览误导

- 状态：BROKEN
- 修复方向：只展示当前 Provider Adapter 实际会调用的 southbound URL；northbound Agent 端点另分组。
- 验收：每个“最终 URL”都可由 request trace 证明真实使用。

#### `TS-OMNI-P2-008` 南向原生协议/关键语义覆盖不足

- 状态：PARTIAL
- 影响：当前所有 Provider 都落到 OpenAI-compatible Chat Completions；原生 Anthropic/Gemini/Responses 与 reasoning 等语义无法完整保真。Embeddings/audio/media 另列 P3，不作为当前 Agent 主链缺陷。
- 修复方向：按真实 Agent 需求补原生 Anthropic Messages、Gemini generateContent、OpenAI Responses Provider Adapter；每个先建立 canonical IR、拒绝矩阵和 conformance。
- 验收：见 §12 `AC-PROTO-*`。

#### `TS-OMNI-P2-009` 缺少显式预算模式与账户/配额状态

- 状态：PARTIAL（observe 已存在；control/account 缺失）
- 影响：当前可安全观测成本，但若产品要承诺成本控制或企业多 Key 韧性，还不能执行 soft-route/hard-stop，也没有 Connection quota/reset provenance。
- 修复方向：保留 `observe` 默认；只允许同一合规 Provider 下来源明确、组织授权的多 Credential，记录 owner/purpose/budget scope；仅从正式 Usage API/标准响应头采集额度；对 429、明确 quota exhausted、临时 5xx/连接失败和可恢复刷新错误做有界切换，对 401、账号禁用、合规拒绝隔离并告警；Receipt 只记 credential ID、原因和 cooldown，不记 Key/正文。
- 验收：见 §12 `AC-BUDGET-*`、`AC-QUOTA-*`。

#### `TS-OMNI-P2-010` 文档能力计数漂移

- 状态：PARTIAL
- 影响：2026-07-22 Provider 预设专项与总验收仍写 52，当前 `catalog.ts` 为 41 个默认预设和 1 个隔离 OpenRouter 候选；历史文档未标清快照边界。
- 修复方向：当前 Provider 预设计数由 registry 自动生成，历史验收显式标注对应 commit，CI 阻断当前文档手写数字漂移。
- 验收：见 §12 `AC-DOC-001`。

#### `TS-OMNI-P2-011` CLI 配置生效依赖重启

- 状态：PARTIAL
- 修复方向：明确每项配置的生效域；短期提供可靠的提示和重启动作，长期只对可安全热加载的配置开放 apply。
- 验收：UI/CLI 能准确说明“已保存/已生效/需重启”，重启失败不丢旧配置。

### 9.3 P3 / 延后项

| Finding ID | 项目 | 结论 |
|---|---|---|
| `TS-OMNI-P3-001` | 更准确 token 估算 | 在路由模拟与评测集建立后改，不先引入远程 tokenizer |
| `TS-OMNI-P3-002` | 上下文压缩 | 仅显式 opt-in、确定性、本地、可审计；默认不删正文 |
| `TS-OMNI-P3-003` | MCP/A2A/Memory | 等核心 Agent、Provider、发布、恢复 SLO 达标后再评估 |
| `TS-OMNI-P3-004` | 全媒体端点 | 按真实用户需求和原生 Adapter 逐项进入，不做空壳路由 |
| `TS-OMNI-P3-005` | 团队/远程控制平面 | 当前定位为本机可信层，暂不扩威胁模型 |

---

## 10. 最小方案与理想方案

### 10.1 最小可行整改（建议先完成）

目标：不扩大产品边界，先让“安装 → 接 Agent → 接 Provider → Agentic 请求 → 失败自救 → 安全升级”闭环成立。

1. 修 `TS-OMNI-P1-001` 能力证据回写。
2. 修 `TS-OMNI-P1-002` Claude/OpenClaw discovery 与原因级诊断。
3. 修两个明确 P2 UX 违约：重复 Provider、URL 预览。
4. 发布签名 macOS 包、注入 release key、完成首启和二次启动测试。
5. 修 `TS-OMNI-P1-004` 核心备份一致性。
6. 增加 dry-run 路由模拟与原因级诊断。

预算 hard-stop、账户配额状态和原生协议都进入理想方案的 Phase 2；它们是战略能力，不应挤占当前已破损主链。

### 10.2 理想方案

目标：形成“可信目录 + 原生协议 + 账户/配额状态 + 可解释排序”的完整本机 Agent 路由产品。

1. Provider/Model capability evidence graph。
2. Anthropic/Gemini/Responses 原生 Provider Adapter。
3. Provider → authorized Credential Connection → Model 三层运行态。
4. 稳定/成本/延迟三个可解释权重包。
5. 签名 Agent compatibility feed。
6. 安全 Plugin Desktop marketplace/rollback。
7. macOS/Windows/Linux 分发矩阵、SBOM、可复现构建。
8. 基于真实 Coding Agent workload 的路由评测与 release gate。

理想方案仍明确排除默认正文持久化、Web-cookie/MITM/TLS stealth 和功能数量竞赛。

---

## 11. 差异化与 Roadmap

### 11.1 建议定位

> **token-station 不是功能最多的 AI 网关，而是本机 Agent 最可解释、最小权限、可恢复的可信路由层。**

Roadmap 完成后的四个目标承诺：

1. 路由结果可预览、可解释、可复现；
2. 默认 loopback + 默认鉴权；Desktop/默认快速开始用 OS 钥匙串，CLI 的 env/file SecretSource 明确标风险；
3. Prompt/response 不落盘，Receipt 只存闭集元数据；
4. 接管 Agent 配置前先展示变更，所有写入可恢复。

### 11.2 分阶段 Roadmap

#### Phase 0：修真实主链（0–2 周）

- `TS-OMNI-P1-001` Desktop tool capability evidence（`AC-CAP-*`）；
- `TS-OMNI-P1-002` Claude/OpenClaw discovery（`AC-AGENT-*`）；
- `TS-OMNI-P2-006/007` 两个明确 UX 违约（`AC-UX-*`）；
- `TS-OMNI-P2-010` Provider 预设验收计数自动生成（`AC-DOC-001`）。

退出条件：CI 的确定性 Provider fixture 证明 Desktop add → test → persist → Router admits tool request；临时 HOME、自定义 npm prefix/Node runtime discovery fixtures 通过；重复 Provider 与 URL 预览满足 `AC-UX-*`；文档计数 gate 不再漂移。

#### Phase 1：交付、恢复与自救（2–6 周）

- `TS-OMNI-P1-003` 签名 macOS 安装包、release key、SBOM、首启/升级/二次启动 E2E（`AC-REL-*`）；
- `TS-OMNI-P1-004` 核心状态原子恢复、可选 metrics online snapshot（`AC-BACKUP-*`）；
- `TS-OMNI-P2-001` 路由 dry-run、排除原因、诊断包（`AC-ROUTE-*`）；
- `TS-OMNI-P2-003` Provider/Model capability provenance（`AC-CAP-003`）。

退出条件：新用户在 mock Provider release E2E 中不需 Rust/Node 完成第一次成功请求；升级与核心恢复故障注入可回到旧状态；定义过的路由失败类别都有稳定 reason code，未知失败保留明确兜底而不伪造根因。

#### Phase 2：协议与运行态（6–12 周）

- `TS-OMNI-P2-008` Anthropic/Gemini/Responses 原生 Provider Adapter（`AC-PROTO-*`）；
- `TS-OMNI-P2-009` 合规 Credential/Quota/Reset/Cooldown 与显式预算模式（`AC-QUOTA-*`、`AC-BUDGET-*`）；
- `TS-OMNI-P2-002` 稳定/成本/延迟三个排序预设（`AC-SORT-001`）；
- `TS-OMNI-P2-004` 签名 Agent compatibility feed（`AC-COMPAT-001`）。

退出条件：新增原生协议通过 golden conformance；预算与账户状态在 Receipt 可审计；排序可重放；compatibility feed 可验签、回滚并离线兜底。

#### Phase 3：验证后扩张

- Plugin Desktop install/update/rollback（`AC-PLUGIN-001`）；
- Linux/Windows Desktop；
- 仅在真实需求充分时增加 embeddings/audio/media；
- 再评估 MCP/A2A/Memory。

测试分三层：CI 必跑无凭据的确定性 fixture/conformance；发布前运行凭据隔离的 smoke job，并把“未配置凭据”与失败分开报告；真实 Claude Code/OpenClaw 等 Agent 由人工 release checklist 留证。后两层不能代替 CI，也不要求把真实密钥放进普通测试环境。

---

## 12. 验收矩阵

| AC ID | 验收项 | 成功标准 |
|---|---|---|
| `AC-CAP-001` | Desktop 新 Provider tool evidence | Provider tool test 成功后，模型能力带来源/时间写入，tool request 被 Router 接纳 |
| `AC-CAP-002` | Tool evidence 生命周期 | tool test 失败、证据过期或被删除后重新 fail closed；旧失败不得覆盖更强且有效的人工/官方证据 |
| `AC-CAP-003` | Evidence 冲突 | preset/live/test/manual 冲突按公开优先级解析，Receipt 显示最终来源 |
| `AC-AGENT-001` | Claude 最小环境 | 临时 HOME 中 `~/.local/bin/claude` 能在安全白名单环境完成版本探测 |
| `AC-AGENT-002` | OpenClaw 自定义 prefix | 可自动发现或手工选择；外置 Node runtime 可验证 |
| `AC-AGENT-003` | 原因级 UI | 至少区分入口未找到、runtime 缺失、版本不兼容、配置不可读、漂移 |
| `AC-AGENT-004` | 可操作恢复 | 每个原因提供适用的重扫/选路径/诊断/恢复动作 |
| `AC-UX-001` | 重复 Provider 保存 | 已存在 ID 的“保存”执行 update 或明确导航详情；不再调用 add 后得到 duplicate |
| `AC-UX-002` | Provider URL 预览 | 只把实际 southbound request trace 会调用的 URL 标作“最终 URL”；northbound 端点分组展示 |
| `AC-ROUTE-001` | Dry-run | 不访问上游，输出命中层、候选、fallback、排除原因、revision |
| `AC-ROUTE-002` | 可重放 | 同配置/健康快照/请求特征重复运行结果一致 |
| `AC-ROUTE-003` | Receipt 对齐 | 相同 config revision 与冻结 health/budget snapshot 下，dry-run 与实际请求的首次 decision 一致 |
| `AC-PROTO-001` | 原生协议 conformance | 每个新增 Provider Adapter 对 request、stream、tool、reasoning/error 建立 canonical ↔ provider golden fixtures |
| `AC-PROTO-002` | 不可表示语义 | 未建模或不能保真的关键字段在访问上游前返回稳定 capability/invalid-request 错误 |
| `AC-PROTO-003` | Structured output | 从支持该能力的 inbound 到原生/兼容 Provider 的端到端 fixture 保留 JSON schema；证据过期时 fail closed |
| `AC-BUDGET-001` | Observe | 只记录和提醒，不改变路由 |
| `AC-BUDGET-002` | Soft route | 只切到用户预批准目标；Receipt 记录价格版本与动作 |
| `AC-BUDGET-003` | Hard stop | 超预算稳定返回结构化错误，不静默替换模型 |
| `AC-BUDGET-004` | Unknown price | 不能按 0 成本处理，不能绕过 hard stop |
| `AC-QUOTA-001` | 合规 Credential 状态 | 每条连接记录非秘密 credential ID、owner、purpose、budget scope、Provider/Model、正式证据来源、观测时间与 reset/cooldown；Key 只进批准的 SecretSource |
| `AC-QUOTA-002` | 有界连接切换 | 仅对 429、明确 quota exhausted、临时 5xx/连接失败、可恢复刷新错误切换；401、账号禁用、合规拒绝必须隔离告警，不能无限换 Key 掩盖 |
| `AC-QUOTA-003` | 无正文审计 | Receipt 记录 credential ID、选择/切换原因、额度 evidence 与 cooldown，不记录 Key、Prompt 或 Response |
| `AC-SORT-001` | 可解释排序 | 权重包版本化；同配置、同健康/配额快照、同请求特征得到相同顺序与逐项分数 |
| `AC-COMPAT-001` | Compatibility feed | 签名和 schema/version 均验证；更新失败保留旧 feed；离线使用内置表；unknown 与 tested 状态分开 |
| `AC-BACKUP-001` | 核心 manifest | config、Agent ownership、加密 snapshot 与 master-key 可用性/跨机限制列入同一版本化恢复单元 |
| `AC-BACKUP-002` | SQLite 一致性 | 用户选择保留 metrics/receipts 时使用 online backup；恢复后 integrity check 通过 |
| `AC-BACKUP-003` | 故障注入 | 核心状态 stage/verify/switch 任一阶段失败都恢复原状态，不产生混合版本 |
| `AC-BACKUP-004` | 状态分层 | revision/tombstone 恢复 identity 语义明确；metrics/receipts 是可选历史且不会残留旧 DB；catalog/诊断/插件索引可重建；外部插件 inventory/hash 缺失时告警 |
| `AC-REL-001` | 签名安装 | macOS 安装无需 Rust/Node；签名/公证可验证 |
| `AC-REL-002` | 首启自检 | 覆盖端口、Keychain、恢复目录、Agent、Provider、鉴权 |
| `AC-REL-003` | 更新验签 | manifest/key/artifact 任一不匹配即拒绝，旧版本继续可用 |
| `AC-REL-004` | 状态升级 | 空状态、已有 DB、已有 Agent ownership、二次启动均 E2E |
| `AC-DOC-001` | Provider 预设计数 | 当前验收文档从 registry 自动生成数量并记录 commit，CI 对手写漂移报错 |
| `AC-PLUGIN-001` | Desktop 插件回滚 | 仅安装通过 manifest/conformance/签名的包；更新失败保留旧版本并显示权限/hash |
| `AC-PRIV-001` | 正文不落盘 | canary prompt/response 不出现在 DB、JSONL、Receipt、诊断包原始字节 |
| `AC-PRIV-002` | Key 不泄漏 | Key 不出现在配置、日志、错误、插件内存序列化 |
| `AC-SEC-001` | 网络默认 | 默认只能 loopback；所有数据/管理端点默认鉴权 |

---

## 13. 证据台账、限制与未验证项

### 13.1 高价值证据台账

| Evidence ID | 事实 | 来源 | 等级 |
|---|---|---|---|
| `E-001` | token-station 业务代码基线 `d8b2939...`，当前分支只多人工测试文档 | 本地 Git | A |
| `E-002` | OmniRoute default branch 是未发布 3.8.49；正式 latest 是 3.8.48 | Git refs + release | A |
| `E-003` | OmniRoute 19 个公开策略 | `routingStrategies.ts` | A |
| `E-004` | OmniRoute Auto score 源码有 13 个潜在因子，README 写 12 | `scoring.ts` + README | A/B |
| `E-005` | OmniRoute 官方目录口径 290，package description 仍写 160+，Reference 分类和 executor 计数也有内部漂移 | docs-count gate + Reference + package.json | A/B |
| `E-006` | OmniRoute semantic cache 默认开，显式 temp=0 response 写 SQLite | migration + semanticCache.ts | A |
| `E-007` | OmniRoute reasoning replay 自动写最多 10,000 个 JS UTF-16 code units、2h TTL | reasoningCache + chatCore | A |
| `E-008` | OmniRoute CLI serve 等主路径默认 bind 0.0.0.0、数据面不要求 Key、setup 可跳过密码 | serve/setup/.env | A |
| `E-009` | OmniRoute 有三层 Route Guard，高风险 spawn 类路由额外保护 | routeGuard/management policy | A |
| `E-010` | v3.8.48 提供多平台资产与 SBOM | GitHub release | A |
| `E-011` | v3.8.47 npm 每次启动崩，v3.8.48 当日 hotfix | v3.8.48 release note | A |
| `E-012` | v3.8.48 Windows existing-DB/second launch 回归 | GitHub #7132 | B |
| `E-013` | v3.8.48 macOS/Linux package external module 回归 | GitHub #7346 | B |
| `E-014` | 单用户长上下文/代理相关 503；3.8.48 也有相似现象，版本与根因未定 | GitHub #8197 | C |
| `E-015` | 本周期较早提交曾 release-green HARD failure；固定 HEAD quick gate 已绿，full gate 未验证 | GitHub #8540 + Actions | A/B |
| `E-016` | `auto` 与 named combo 心智模型误解 | GitHub #7992 | B |
| `E-017` | token-station Workspace tests 全绿 | 本轮命令输出 | A |
| `E-018` | token-station 干净 Frontend 20 files/153 tests 全绿 | 本轮命令输出 | A |
| `E-019` | token-station Desktop Rust 176 tests 全绿、1 ignored | 本轮命令输出 | A |
| `E-020` | Desktop Provider tool 测试证据未回写，Router 对 tool 要求 positive evidence；JSON probe 也未持久化，但当前官方入站尚未形成 structured-output 链路 | 本地源码链 | A |
| `E-021` | Claude/OpenClaw discovery 在本机真实安装中阻断 | 人工测试 + discovery 源码 | A |
| `E-022` | token-station 预算永远 observe-only | config/budget/gateway | A |
| `E-023` | token-station backup 范围与一致性不足 | backup/store/recovery | A |
| `E-024` | 受测 Chat 成功路径的 canary 未进入 `metrics.sqlite`/`requests.log`，RequestRecord/Receipt schema 无通用正文栏位 | proxy 集成测试 + metrics schema | A |
| `E-025` | OmniRoute quota fetcher 覆盖远小于目录，hard cutoff 有 last-resort override | usage + resolveAutoStrategy | A |
| `E-026` | OmniRoute 加密实现是字段级且无 Key/异常时 fail open，和 `.env` 整库宣称不一致 | encryption.ts + .env | A |
| `E-027` | OmniRoute encrypted backup create 与 restore 未闭环 | backup command + tests | A |
| `E-028` | OmniRoute Translator Registry 覆盖多种双向格式，但 OpenAPI 版本落后包版本 | formats/bootstrap/openapi/package | A |
| `E-029` | token-station 的 `v0.1.0`、`v0.2.0`、`v0.2.1`、`v0.3.0` GitHub Releases 均为 Draft、无 publishedAt | GitHub API/Releases | A |
| `E-030` | OmniRoute 当前源码的免费口径分别是 126 个 `hasFree`、516 行/81 个 canonical Provider、43 个 headline pool，集合并不相等 | 固定源码全量重算 | A |
| `E-031` | 43 pool 中 22 个对 steady headline 贡献 0，只有 21 个为正；1.526B 的前三个 pool 占 85.2% | catalog + `computeFreeModelTotals` 等价重算 | A |
| `E-032` | fresh `auto` 只 allowlist OpenCode/Felo 两个 No-auth 后端；两者均被 canonical 标为 ToS `avoid`，Felo 明确为逆向网页接口 | virtualFactory/noauth/Felo executor/catalog | A |
| `E-033` | Together/Predibase 已和同提交 Provider 定义或官方资料冲突；LLM7 当前官方 limit 也与 catalog 不符 | 固定源码 + Provider 官方资料 | A/B |
| `E-034` | OpenCode executor 支持本地 slot、可选 proxy、429 cooldown 与换槽；slot ID 是本地 UUID且不发给上游，无 proxy 的多 slot 仍是同一匿名出口，不能据此证明多个账号或账号来源 | 固定源码与证据边界 | A |
| `E-035` | 正式版 v3.8.48 源码为 101 个 `hasFree`、456 行/69 Provider、38 pool、约 1.372B steady，与 tag 内 README 的约 1.6B 宣称漂移 | peeled release commit 全量重算 + tag README | A/B |
| `E-036` | OpenRouter canonical 把 `auto` 错列/至少未证实为 free pool；24M 是充值后总量而非净增量，源码 `+24M boost` 还多算 1.2M | 固定 catalog + OpenRouter 官方 models/limits | A/B |

### 13.2 外部检索限制

Agent Reach 诊断结果：

- GitHub CLI、Jina Reader、V2EX 公共 API、Bilibili 搜索 API、RSS 可用或部分可用；
- Reddit、X/Twitter、小红书、YouTube 没有可用 backend；
- Exa 连通性不稳定，Doctor 未确认；本轮既出现失败也出现成功请求；
- Jina/搜索读取 GitHub 时受网络信誉限制；
- V2EX 没有关键词搜索；
- Bilibili 无评论读取 backend。

因此本报告的用户反馈部分是 **GitHub-centric**，不能泛化为“全网口碑”。未发现站外负评不等于站外没有负评。免费来源专项另外访问了各 Provider 的当前官方定价、限额、认证或使用条款页面；这些是一手资料抽查，不等于 81/126 个来源全部通过 live certification。

### 13.3 未验证项

1. 未安装 200–350 MB 的 OmniRoute 官方 Desktop 资产。
2. 未安装约 739 MB 解包体积的 npm 包。
3. 未登录真实 OAuth/Web-cookie/订阅账号。
4. 未用真实 Provider 执行 OmniRoute `auto`/Combo/媒体/MCP/A2A E2E。
5. 未独立复现 OmniRoute GitHub issue；报告按 issue 证据强度分级。
6. 未对两边做统一真实模型质量/延迟/成本 benchmark，不能给出“谁更省/更快”的量化结论。
7. OmniRoute 3.8.49 是活动分支，后续提交可能改变结论；正式版和分支必须分别复核。
8. 免费专项已全量重算源码分类，但只抽查代表性官方资料；没有逐账号验证全部免费资格、地区/KYC、真实 reset、吞吐、内容政策或长期稳定性。

---

## 最终决策

如果 token-station 现在只有一条资源主线，建议顺序是：

> **能力证据闭环 → Agent 真实接入 → 两个 UX 违约 → 签名分发 → 核心恢复一致性 → 路由模拟/诊断 → 原生协议与合规 Credential/预算 → 三个可解释自适应预设。**

不要先做：

> **Provider/“免费源”数量竞赛、19 种策略、Web-cookie/MITM、账号堆叠、默认正文缓存、MCP/A2A/Memory/媒体全家桶。**

这样才能借到 OmniRoute 真正有价值的产品闭环和运行态经验，同时避免把它当前的复杂度、隐私边界与发布压力一起搬进 token-station。

# Cursor Router × token-station 深度竞品拆解与查漏补缺

> 初稿：2026-07-27；深度复核：2026-07-28（Asia/Shanghai）
> 本轮深度复核窗口：2026-07-28 10:06–10:52（约 45 分钟）
> token-station 研究起始基线：分支 `test/manual-agent-regression-20260723`，commit `d96ea4dbf3b74138d9fda5cd1b59ee9587e70445`
> 交付时 HEAD：`fce85daca36e19a886189bf0fd6b3c37b9548558`（研究期间外部 merge；下列相关路径与起始基线无 diff）
> Cursor 本机版本：3.12.17；Cursor Router 官方发布时间：2026-07-22
> 范围：Cursor 模型选择路由、成本/质量控制、缓存、透明度、隐私边界与评测闭环。
> 动作边界：只读审计、公开资料检索、无请求/无付费的 Cursor UI 查看、现有测试运行；未修改业务代码、真实配置或远端状态。

## 0. 执行摘要

### 0.1 先说最简单的：token-station 应该学什么 / 先修什么

一句话：**学 Cursor 的“模型—任务效果数据、成本档位、线上验证纪律”，不学它的云端黑盒路由、隐藏底层模型和不可控模型池。**

应该学：

1. **把“任务看起来复杂”改成“哪种质量 profile 更可能赢”**：离线用同一 coding task 分别跑 `model × harness_family × reasoning/context profile`，以测试结果、人工偏好和受控 judge 形成标签；热路径运行本地低延迟分类器，输出 profile 胜率，不远程调用 LLM judge。provider 健康/容量另由执行层处理。
2. **加入明确的成本—质量模式**：最小先做 `省钱 / 均衡 / 质量` 三个经过评测的阈值 profile；模式只改变选档阈值与候选效用权重，不改变用户规则、显式模型、隐私和能力门禁。
3. **把会话切模安全和成本一起纳入选档**：先记录 `previous_target / cache tokens / switch_reason`，用保守惩罚避免无谓 cache miss；同时先加会话兼容门禁。Cursor 能跨家族切换，是因为它也切 prompt、编辑工具格式和 tool set；token-station 不能把“wire-compatible”当作“conversation-compatible”。
4. **建立 shadow → 非劣效 → 灰度的发布门**：旧路由实际执行，新路由只记无正文建议；按任务类型报告成功率、成本、延迟、切模率和置信区间，达门槛后再启用。
5. **保持并强化每请求解释**：token-station 已经能展示池、模型、规则/Hint/分数和 attempts；未来补 `task_type / classifier_version / candidate_count / exclusion_counts / harness_family`，底层模型永远不默认隐藏。

最小落地形态：

```text
显式模型 / 用户规则 / Agent Hint（永远优先）
                     ↓
本地 classifier：task_type + P(strong quality profile wins)
                     ↓
mode profile：Cost / Balance / Quality 阈值
                     ↓
能力、隐私、会话兼容硬过滤 → 切模/cache 惩罚 → 健康排序
                     ↓
只持久化数字特征、版本、候选摘要和最终决策；不存 prompt/response
```

失败边界：分类器 artifact 缺失、验签失败、输入不支持或置信度不足时，回到当前确定性启发式；不得静默改用远程分类器。显式模型、用户规则、`local_only` 和 capability filter 永远不能被学习层覆盖。

明确不学：

- 不把 prompt 和代码上下文上传到 token-station 自己的云端做路由；这会破坏本地决策的核心定位。
- 不让厂商动态修改用户的模型池，也不把某个商业模型设为 Router 工作的强制底座。
- 不默认隐藏实际模型；Cursor 社区已经把这视为质量回归、费用和审计的共同障碍。
- 不复刻 Cursor 的全量行为遥测；只做本地或明确 opt-in、无正文、可删除的评测数据。

优先级与主线关系：

- `P1（立即澄清）`：先统一 `auto` 与具体模型的公开契约；当前文档承诺“具体模型直连”，但 exact 路径由默认关闭的全局布尔值控制，且开启后也没有排除 `auto` 哨兵值。
- `P1`：学习型胜率路由 + 评测闭环，是产品主线，但实施顺序必须排在现有发布安全红线之后。
- `P1`：在任何跨家族自动切模前增加 `harness_family / conversation_compatibility_group` 门禁；否则模型虽能接收同一协议，也可能误用旧历史中的工具形状。
- `P1`：先用现有最高优先级规则提供可选“高风险任务不降档”模板；复杂度不能代替风险。
- `P2（立即）`：修正文档漂移和无证据的“不降质省 token”绝对表述，不等待分类器。
- `P2`：模式 profile、cache/switch 回执和解释字段，随 classifier shadow 一起落地。
- `P3`：团队级强制 Auto、组织分组、远程策略分发；只属于未来托管企业控制面，不阻塞本地产品。

证据边界：本地源码和测试能证明 token-station 当前是可重放的规则/Hint/整数启发式路由，不能证明它已做到“不降质”；Cursor 官方能证明存在经 60 万+真实请求训练的 classifier，但没有公开模型类型、参数量、特征编码、延迟、路由准确率或可复现实验，故不能写成“Cursor 使用小 LLM”。

### 0.2 一句话总判断与证据边界

Cursor Router 与 token-station 在“按请求选模型”这一层直接可比，但信任边界相反：Cursor 用托管数据闭环换取更强的模型效果画像，token-station 用本地确定性和用户控制换取隐私、审计与可重放。最值得借的是**评测和产品控制面**，不是照搬其闭源 classifier。

### 0.3 结论总表

| ID | 结论 | token-station 状态 | 建议 | 优先级 |
|---|---|---|---|---|
| BUG-001 | `auto` / 具体模型契约与实现开关不一致 | `broken`：文档承诺具体模型直连，exact 默认关闭 | 以 `model == "auto"` 为选档边界；具体模型必须精确或拒绝 | P1，立即澄清 |
| GAP-001 | 没有学习型模型效用/胜率分类器 | `partial`：启发式可用，训练分类器仅计划 | 复用现有四层骨架，新增本地版本化 classifier | P1 |
| GAP-002 | 没有可证明的质量—成本评测闭环 | `missing`；已有 receipts，但没有任务结果标签 | paired runs + shadow + 非劣效门槛 | P1 |
| GAP-003 | 复杂度与风险没有分开 | `partial`：用户可写高优先级关键词规则 | 提供显式 opt-in 的高风险升档模板 | P1 |
| GAP-004 | 路由不考虑切模/cache 经济性 | `missing`；协议能记 cache usage，Router 不读 | 先观测，再加切模惩罚；不可凭营销话术上线 | P2 |
| GAP-005 | 没有经过评测的 Cost/Balance/Quality 模式 | `partial`：有三档模型，没有优化模式 | 模式映射阈值/效用权重，用户模型池不变 | P2 |
| GAP-006 | 没有会话/harness 兼容性门禁 | `missing`：协议能力过滤不等于跨模型历史兼容 | connector 声明兼容组；不兼容模型不得静默接管旧会话 | P1 |
| DOC-001 | 路由文档与当前代码不一致 | `broken`：文档仍称每工具加分 | 修正文档并加文档—行为回归检查 | P2，立即 |
| DOC-002 | “不降质省 token”缺少当前证据 | `broken`：贡献者文档仍作绝对承诺 | 改为目标；报告基线、样本和置信区间后再宣称 | P2，立即 |
| OBS-001 | 回执缺 classifier/cache 切换解释字段 | `partial`：已有 decision/attempt/features | 增加版本、task、候选排除、前后模型与切换原因 | P2 |
| STR-001 | 每请求可解释与用户覆盖强于 Cursor | `present`，测试通过 | 保持默认可见，不能为“去偏见”隐藏 | 保持 |
| ENT-001 | 团队级模式限制和强制 Auto 尚无 | `missing`，且非本地 MVP 战场 | 留给未来企业控制面 | P3 |

### 0.4 建议实施顺序

严重度排序与施工顺序要分开：`GAP-001/002` 是 P1 主线，但不能抢占现有 P0 发布可靠性工作。

1. **现在，先做**：决定并测试 `BUG-001` 的公开契约。按现有 README 心智，建议仅 `model == "auto"` 进入选档，具体模型只能精确提供或明确拒绝，不能静默换模型。
2. **现在，半天内**：修 `DOC-001/002`，停止把 heuristic 描述成已证明“不降质”的智能路由。
3. **现在，1–2 天**：整理可选高风险关键词模板和 receipts 字段 schema，不自动替用户开启。
4. **学习层真正生效前**：加入会话兼容组；跨组切换只有 Agent connector 明确支持 harness 切换时才允许。
5. **可靠性 Gate 之后**：跑 classifier shadow；现有训练数据声明必须先重新核验来源、授权、去重、execution-profile 主键和标签质量，未经数据卡审计不能直接复用。
6. **shadow 达门槛后**：启用三模式 profile 与低置信度回退；先单机 opt-in，再扩大。
7. **有真实 cache telemetry 后**：增加会话切模惩罚；不要先写 sticky 算法再找证据。
8. **未来企业版**：再做组织模式限制、远程签名策略和团队汇总；正文与 Key 仍不进入控制面。

## 1. 产品边界：这些东西分别是什么

### 1.1 Cursor Router

Cursor Router 是 Cursor 托管的、位于 Agent 请求与底层模型之间的模型选择服务。Teams/Enterprise 的 Auto 外壳提供 Cost、Balance、Intelligence 三个 profile；[官方文档](https://cursor.com/docs/cursor-router)只明确说 Balance/Intelligence 在每个 Agent 请求推理前运行新 classifier，Cost 沿用 previous Auto routing logic。管理员可控制启用、模式、模型访问和实际模型可见性，模型池由 Cursor 管理并随新模型变化。

它的路由单位是 **Agent request**，不是整条 chat/thread 的一次性绑定。一个多轮会话会反复决策，SDK 也明确说底层模型可在 requests 之间变化；因此 session continuity、cache 与 harness handoff 都是 Router 正确性的一部分。

请求路径的信任边界是：代码/上下文进入 Cursor 及其模型/推理子处理方。Cursor 的[隐私与数据治理文档](https://cursor.com/docs/enterprise/privacy-and-data-governance)说明 AI 功能会把 prompt 和代码上下文发送给 OpenAI、Anthropic、Google 等模型提供方；Privacy Mode 约束训练用途和多数模型的 ZDR，但不等于“请求只在本机”。

### 1.2 token-station

token-station 是本地回环网关。Router、规则、指标和 Key 管理在本机；请求正文只发往用户配置的最终模型上游，不额外发送给路由服务。模型池、优先规则、Agent Hint、精确模型、recovery 和 `local_only` 都由用户控制。

当前第三层是手工特征的整数 heuristic，不是学习型 classifier。源码已经把它标为未来 learned classifier 上线前的占位层（`crates/router-core/src/config.rs:46-51`）。

## 2. 比较是否成立：相同战场与不同战场

直接可比：

- 每个 Agent 请求应选哪一档/哪一模型；
- 如何权衡质量、成本、延迟、上下文和缓存；
- 用户能否覆盖、审计和复现路由；
- 新模型上线后如何重新校准；
- 失败、低置信度和能力不满足时如何处理。

不应直接排名：

- Cursor 的 IDE harness、云 Agent、账户计费与 token-station 的本地网关不是同一产品层；
- Cursor 的海量跨用户在线行为数据与 token-station 的本地隐私边界不能用“数据越多越好”一项粗暴比较；

## 3. Cursor Router 的具体优势

### 3.1 产品优势

1. **用户不用持续追新模型**：模型池可随模型发布更新，Router 维护每类任务的模型偏好。
2. **一个清晰的 Pareto 控件**：Cost/Balance/Intelligence 把复杂内部策略压缩成用户能理解的成本—质量选择。
3. **团队治理完整**：可按团队/组启用、限制模式、allow/block 模型、软/硬强制 Auto。
4. **计费与行为绑定**：Cost 为固定 Auto 定价；Balance/Intelligence 按实际选中模型计费。[价格文档](https://cursor.com/docs/models-and-pricing)在 2026-07-28 显示 Auto Cost 为每百万 token：input/cache write `$1.25`、cache read `$0.25`、output `$6`；第三方模型在 Balance/Intelligence 还会叠加每百万 Cursor token `$0.25`，而 Auto Cost 与第一方 Cursor 模型豁免。价格会变，实施不能硬编码本文数字。

### 3.2 技术与数据优势

Cursor [发布博客](https://cursor.com/blog/router)公开的高价值部分是：

- 超过 60 万条 live requests 训练；
- 数百万真实请求在线 A/B；
- 输入涉及 query、context、task complexity、domain 和各模型行为画像；
- 奖励使用 AFC 用户满意度；质量指标还包括生成代码 keep rate；
- 训练/评估考虑了跨模型 cache miss；
- 评测覆盖完整会话，而不只是孤立 prompt benchmark。

这说明 Cursor 的护城河主要是**真实 coding 分布下的质量 profile 效用数据**，不是“classifier”这个名词本身。由于 Cursor 会为模型/版本切换 harness，线上观察到的效果严格说是 `model + reasoning/context 参数 + Cursor harness` 的联合效果，不能无条件解释成裸模型能力；provider 故障/容量则应作为执行层结果单独归因。

官方给出的路由例子是：简单工作去价格效率模型，UI 更新去“taste”更好的模型，复杂长程问题去 frontier reasoning 模型。这些例子证明它不只是长度阈值，但不等于公开了完整 taxonomy 或确定性规则。

但公开证据只够还原到下面这一层：

```text
请求 query/context/task/domain ───────────────┐
管理员 allow/block、区域、required model ─────┼→ 候选约束 + Balance/Intelligence classifier
动态 catalog、模式、模型行为画像 ──────────────┘
                                               │
                              选中模型/参数（本文抽象为 quality profile）
                         │
       对应模型专属 harness（prompt / tools / edit format）
                         │
       统一 inference gateway → provider → failover/retry
                         │
 用户后续行为 + 代码 keep rate + 成本/cache miss → 线上评测
```

这不是 Cursor 公布的逐函数调用图，而是由 Router 文档、SDK 契约、harness 工程文和推理平台职位说明交叉复核出的**分层架构**；图中输入汇合不表示官方公布了过滤与打分的先后顺序。它只画 Balance/Intelligence，新 Cost 的内部路径仍按 previous Auto logic 处理。只有输入、模式、候选约束和结果指标得到 Router 官方直接确认；classifier 内部网络、特征编码、决策头和训练算法仍是黑盒。

### 3.3 “classifier”到底能推到哪一步

能确认：

- Balance/Intelligence 在每个 Agent request 推理前运行 classifier；
- 输入语义包含 query、context、task complexity、domain，以及 Cursor 对各模型行为的画像；
- 官方称它使用 data-driven taxonomy，并用 60 万+ live requests 训练；
- 线上 A/B 用数百万真实请求，以用户后续行为推断的 satisfaction 和代码 keep rate 衡量质量；
- 新模型发布后 Cursor 可以更新 Router。

不能确认：

- classifier 是小 LLM、encoder、树模型、embedding router、规则+模型混合，还是多个 per-model success head；
- `context`具体指完整 prompt、截断文本、摘要、embedding 还是结构化统计；官方只公布语义类别；
- 输出是 task label、模型 ID、各模型胜率、效用向量，还是先分类再查映射表；
- 60 万条数据如何获得跨模型标签。一次生产请求只观察到“被选模型的结果”，没有其他候选模型的反事实结果；要学习“谁更好”，理论上需要随机探索、流量重叠、同题回放、reward model、人工偏好或 contextual-bandit 校正中的一种或多种，但 Cursor 没有披露；
- 是否有置信度拒绝、候选回退、探索率、倾向分数校正、漂移检测、版本回滚和在线学习；
- 新模型/新 harness profile 的冷启动标签来自 offline eval、早期访问、人工映射还是在线探索，以及旧模型版本更新后怎样失效；
- classifier 的模型大小、token 开销、p50/p95 延迟和部署位置。

所以，对“是不是用小模型读意图分类”的严谨答案是：**它确实读请求与上下文做分类；是不是小模型、是不是生成式 LLM，未知。** “data-driven taxonomy”更接近公开事实，但仍不足以判定具体算法。

还有两个 classifier 不能混为一谈：Router classifier 负责**推理前选模型**；Cursor 的 harness 工程文另称，会用一个 language model 读取用户对 Agent 初始输出的后续响应，语义判断满意/不满意。后者是线上评测/奖励信号生成器，不是 Router 本身。两篇官方材料给出的正负例一致，但没有逐字确认该 judge 就是 AFC 的完整实现；也没有说明它的模型、prompt、准确率或 AFC 缩写的完整展开。

公开研究能帮助界定“可能的算法族”，但不能拿来替 Cursor 填空。[RouteLLM（ICLR 2025）](https://proceedings.iclr.cc/paper_files/paper/2025/file/5503a7c69d48a2f86fc00b3dc09de686-Paper-Conference.pdf)对同一目标分别实现过 similarity-weighted ranking、矩阵分解、BERT classifier 和 8B causal LLM classifier，统一预测 `P(strong wins | query)`，再用成本阈值选强/弱模型。这证明“classifier + 成本档位”完全可能不调用生成式小 LLM；也说明三种优化模式可以只是阈值/效用权重不同，而非三个模型。Cursor 没有确认自己采用其中任何一种。

[BaRP 的 bandit-feedback 研究](https://arxiv.org/abs/2510.07429)则专门指出，生产环境通常只看到被选模型的结果，和“每个候选都有标签”的离线全信息数据不同。这支持上面的反事实缺口判断，但仍不能证明 Cursor 用 contextual bandit。对 token-station，安全的第一步是离线 paired run；不应为了在线探索故意把用户请求随机发给未经验证的弱 profile。

### 3.4 Router 不是孤立分类器：模型专属 harness 是前提

Cursor 的[Agent harness 工程文](https://cursor.com/blog/continually-improving-agent-harness)补上了 Router 博客没展开的关键条件：

- 不同 provider、甚至不同模型版本使用定制 prompt；
- OpenAI 模型示例使用 patch 编辑工具，Anthropic 模型示例使用 string replacement；给模型陌生工具会多耗 reasoning token 且更易出错；
- 中途切模时 Cursor 自动切换到目标模型的 prompt 和 tool set；
- 新模型看到由旧模型生成的 conversation history 属于分布外输入，Cursor 会加入“接管旧会话”和“不要调用旧工具”的专门指令；
- cache 是 provider/model-specific；Cursor 试过切换时总结会话以减轻 cache penalty，但深层复杂任务会丢细节，因此官方通常建议没有理由时整段会话保持一个模型。

这改变了架构结论：**一个通用 API 网关即使能转换请求格式，也未必有资格在多轮 Agent 会话里自动换模型。** Router 的质量来自“选模型 + 切 harness + 处理跨模型历史”的组合，不应只复制第一项。

它也改变了样本主键：不能只记录 `model_id`。同一底层模型在不同 reasoning effort、上下文窗口、Agent connector、system prompt 和 tool schema 下可能表现不同；token-station 若汇总成一个“模型胜率”，会产生 Simpson's paradox 式误导。样本必须同时带版本化 quality/execution profile。

为避免和可靠性层混淆，建议内部再拆成：`quality_profile = model + harness + reasoning/context 参数`，`execution_profile = quality_profile + provider target`。classifier 学前者；健康、容量、价格和 failover 选择后者。最终 outcome 可以关联两者，但 provider 429/timeout 不能回灌成“模型不擅长这个任务”。

这个风险不是微小格式差异。Cursor 的[Codex harness 文章](https://cursor.com/blog/codex-model-harness)称，在其 CursorBench 实验中移除 GPT-5-Codex reasoning traces 造成 30% 性能下降（厂商自报、不可直接外推）。token-station 的 canonical IR 会保留已建模或 unknown content part，这对不丢数据很重要，但“字节保留”仍不能保证另一模型家族理解并能续用这些 provider-specific traces。

### 3.5 选择层和执行层必须分开

Cursor 的[Model Routing & Inference 团队职位说明](https://cursor.com/careers/software-engineer-model-routing-inference)显示，其推理平台还负责：

- 用统一 inference gateway 抽象不同 provider API；
- 跨 provider failover；
- backpressure 和 admission control；
- provider capacity 与 economics。

这些材料能证明 Cursor 把推理可靠性与 Router 放在同一平台团队中，但不能证明某次 Balance/Intelligence 的语义选模会按什么精确顺序调用 failover。对 token-station 更稳妥的借鉴是保持两层：

1. **质量/成本选择**：任务—模型效用、模式、用户规则、会话兼容；
2. **执行可靠性**：能力、健康、限流、重试、provider failover。

不能把“某 provider 暂时拥塞”重新解释成“这个任务适合另一个模型”，否则训练标签、回执原因和质量评测都会被污染。

### 3.6 SDK 与控制面的真实契约

官方 [TypeScript SDK](https://cursor.com/docs/sdk/typescript#cursor-router)把 Router 暴露为 `auto-smart`，并要求显式传 `optimize_for=cost|balanced|intelligence`。关键契约是：

- SDK 运行的是 Cursor agent workflow，不是 standalone chat-completions/raw inference API；Router 产品本身就与 Cursor harness 绑定；
- 先调用 `Cursor.models.list()`发现当前 API key/team 可用的模型、参数和 Router mode；不能硬编码 catalog；
- 省略 `optimize_for` 或传 legacy `default` 不受支持；
- `{id: "auto"}`只是具体模型缺失时的 server-selected Auto fallback，不等于显式 Router profile；
- per-run model/profile override 会成为 SDK agent 后续 send 的 sticky selection；
- 但 Router 的**底层模型**仍可在 request 之间变化；需要可复现实验时官方建议固定具体模型。

这里有两个容易混淆的“sticky”：SDK 记住的是 `auto-smart + optimize_for` 这个选择，Router 并不承诺记住它上次选中的底层模型。

本机 Cursor 3.12.17 安装包的只读静态审计也发现了与文档一致的控制面 schema：`SmartAutoSettings` 含 `enabled / optimize_for / show_routed_model`，required model 含 blocked/severity，catalog 元数据含 `visible_in_routed_model_view`，usage event 含 `routed_model`，团队/组设置含 `models_auto_only`。这只能作为“客户端做动态 catalog、策略能力协商与展示”的佐证，**不能证明服务端 classifier 的实现**。审计文件 SHA-256 为 `23ed0b…ec6565`。

### 3.7 工程与 UX 优势

- 模式切换不要求用户重配每个模型；
- Enterprise 模型访问控制、区域可用性和 required models 先约束动态候选集；
- 模式选择与底层模型选择被清楚分成两层；
- 实际执行模型可进入 usage/SDK result；只是 UI 默认隐藏；
- 旧 Cost Auto 的公开算法仍很少。历史论坛维护者提到 provider 负载、区域可用性、成本和可靠性，但这不能证明 Cost 使用新 Router classifier。[旧 Auto 讨论](https://forum.cursor.com/t/why-does-cursor-still-have-an-auto-model/161841)

## 4. 竞品功能三列全拆解

| Cursor 功能 | Cursor 技术方案 | 更优/更适合 token-station 的方案 | token-station 当前状态 | 差距与动作 | 优先级 | 验收 |
|---|---|---|---|---|---|---|
| 请求级模型选择 | 闭源 classifier；query/context/complexity/domain + 模型画像 | 本地 `P(strong quality profile wins)` + task head；规则/Hint 优先 | heuristic 分档 | GAP-001 | P1 | 低置信度回退；显式覆盖 100% 保持 |
| 真实反馈训练 | AFC、keep rate、线上 A/B | paired task outcome + 本地/opt-in 无正文反馈 | 无闭环 | GAP-002 | P1 | 每 task type 报质量非劣效、成本和 CI |
| 三优化模式 | Cost/Balance/Intelligence | 签名 profile 只改阈值和效用权重 | 三模型档，不是三优化模式 | GAP-005 | P2 | 切模式不改模型池、规则和隐私门禁 |
| Cache-aware | 训练和生产评测包含 cache miss；运行时细节未公开 | 先记录切模与 cache token，再加显式惩罚 | 协议有 cache usage，Router 不使用 | GAP-004 | P2 | 相同任务开启后总成本非劣、切模率可解释 |
| 跨模型会话接管 | 自动切目标模型专属 prompt/tools，并加跨历史接管指令 | connector 声明 `harness_family`；不兼容时保持原模型或新建上下文 | 只有 wire capability，无会话兼容维度 | GAP-006 | P1 | 不兼容历史不得静默交给另一模型家族 |
| 底层模型可见性 | 管理员可显示；默认隐藏 | 始终默认显示，可折叠细节 | receipts 已展示 | STR-001 | 保持 | 每请求能看到目标、原因、候选/attempt |
| 模型 allow/block | Enterprise 候选约束；Grok 4.5 为 Router 必需 | 用户模型池即 allowlist，无厂商必选模型 | 已有 pool/capability/local_only | 已有优势 | 保持 | 无配置外模型被选中 |
| 动态模型池 | Cursor 管理并持续变化 | 用户显式 catalog + 版本化建议，更新不自动改路由 | 用户管理 | 不照搬 | — | 更新前后 diff 可预览、可回滚 |
| SDK catalog 协商 | `models.list()`返回团队允许的 model/parameter/mode；`auto-smart`显式带 profile | provider/connector 能力动态发现，但结果版本化并由用户确认 | 静态配置为主 | 借 catalog，不借远程静默变更 | P2 | 未发现/不允许的 mode 明确拒绝 |
| 团队强制 Auto | 软默认/硬锁定、组策略 | 未来签名团队策略；本地用户可查看来源与覆盖权 | 未实现 | ENT-001 | P3 | 策略来源、版本、覆盖权清楚 |
| 隐私治理 | 云处理 + Privacy Mode/ZDR/模型例外 | 本地判定，正文只去最终上游 | 已实现设计边界 | 差异化 | 保持 | Router 无新增外联，持久化无正文 |

## 5. token-station 当前能力映射

### 5.1 已实现且实测通过

- `RouterConfig` 明确分离 rules、hint routes、heuristic、default、exact model、recovery、local-only（`crates/router-core/src/config.rs:35-83`）。
- `Router::route` 只在 `honor_exact_model=true` 时先走 exact override，否则直接选池；这个开关默认为 `false`（`crates/router-core/src/config.rs:52-65`、`route.rs:109-149`）。
- `select_pool` 是 first-match-wins：rule → hint → heuristic → default（`route.rs:248-294`）。
- capabilities 对 tools、vision、JSON Schema、context window 做硬过滤（`route.rs:396-429`）。
- `Decision` 保存选择、原因、fallback 数、数字 features 和 pool，不保存正文（`crates/router-core/src/decision.rs:147-170`）。
- metrics 把原始 decision 与真实 attempts 分开保存（`crates/metrics/src/lib.rs:73-115`）。
- 桌面 receipts 展示实际 upstream/model、池、原因、分数、fallback、attempts、转换和错误诊断（`apps/desktop/src/components/RecentReceipts.tsx:39-129`）。
- UI 已提供用户关键词最高优先级覆盖和严格本地/云 fallback 明示（`apps/desktop/src/pages/HomePage.tsx:210-269`）。

### 5.2 部分实现

- 有 Cost 所需的价格与 usage 基础，但 Router 没有把价格纳入候选效用。
- 有三档模型池，但它表示模型能力档，不是 Cursor 的三个优化目标。
- 有 cache read/write usage 字段，但 route decision 不读取会话前一模型和 cache 状态。
- canonical protocol 能表达 tools/vision/json/context 等 wire capability，但没有 `harness_family`、跨模型 history compatibility 或 connector 是否能切 prompt/tool set 的声明。
- 有 classifier 专项设计，但状态明确是“冻结/暂不排期”，不能算产品能力。
- 有 exact-model 实现和测试，但例子配置没有开启；开启后对 `model="auto"` 也会走 exact 路径，与 README/产品文档的二分契约不一致。

### 5.3 仅文档/计划

- `P(strong quality profile wins)` 本地分类器；
- classifier artifact 的版本化、签名、回滚；
- shadow/non-inferiority/灰度；
- `task_type / classifier_version / candidate_count / exclusion_counts` 回执字段。

### 5.4 未发现

- 任何 classifier artifact 加载或推理代码；
- 模型 × task type 的效果矩阵；
- 路由 A/B 分桶与 outcome join；
- 会话 sticky/switch penalty；
- conversation/harness compatibility group；
- Cost/Balance/Quality profile schema；
- 团队级强制 Auto 或组织组策略。

## 6. 我们没有但应该补的能力

### GAP-001：本地模型胜率分类器

- 现状：heuristic 统计 token、代码块、轮数和词表命中，预测的是表面复杂度。
- 影响：短但高风险的 auth 修改可能被低估；长但机械的批量重命名可能被高估。
- Cursor 借鉴：按真实 task/domain 学各模型效果，而不是只看长度。
- 最小方案：先二分类 `P(high quality profile beats mid quality profile)`、`P(mid quality profile beats low quality profile)`；quality profile 至少绑定 model、connector/harness family、reasoning/context 参数和版本。输出概率和版本，低置信度回 heuristic。
- 输入边界：沿用当前正确做法，把固定 system prompt/tool inventory 与用户对话难度分开；用版本化 harness ID 表达脚手架，不让某个 Agent 的几千 token 固定 prompt 把每条请求都推成“复杂”。
- 理想方案：多模型效用排序 `expected_success - λ_cost - μ_latency - ν_switch_cost`。
- 风险：训练分布漂移、judge 偏差、模型/harness 更新导致 profile 标签失效、新 profile 冷启动时被旧 artifact 误当成同一候选。
- 验收：按任务类型报告强模型调用率、成功率、成本、置信区间；相对固定强模型达到预设非劣效界限。

### GAP-002：评测和反馈闭环

- 现状：receipt 能记录“选了谁、发生了什么”，不能回答“选得对不对”。
- 最小方案：离线 benchmark 将 task ID、候选模型版本、测试终态和人工偏好关联；生产只做 shadow，不采正文。
- 理想方案：用户明确 opt-in 后上传聚合、差分隐私或团队内自托管指标。
- 不建议现在做：从“用户没有继续追问”直接推断成功；这种 AFC proxy 依赖 Cursor 的产品规模和行为语义。
- 验收：任一质量宣称都带样本分布、基线、置信区间、成本和模型版本。

### GAP-003：风险地板

- 现状：最高优先级 keyword rule 已能强制升档，但没有经过审阅的模板。
- 最小方案：提供不默认开启的模板，例如认证/授权、加密、生产迁移、不可逆数据操作；预览将添加哪些关键词和目标档。
- 理想方案：把 `risk_class` 作为与 complexity 分离的显式 Hint/规则维度；高风险只提高最低档，不由 classifier 降低。
- 验收：启用模板后对应 fixture 总是先由 rule 命中；关闭模板后行为完全回到用户配置。

### GAP-004：会话切模与缓存经济性

- 现状：usage 能表达 cache tokens，但 route 输入没有 previous target/cache state。
- Cursor 证据边界：官方只证明训练和评估计入 cache miss，并未公开 sticky 公式或运行时特征。
- 最小方案：先只观测模型切换、切换原因和下一轮 cache usage；基于真实数据设置静态切换惩罚。
- 理想方案：provider/model-specific 的预期 cache 命中、TTL 和输入价格进入效用函数。
- 验收：同一回放集下报告切换率、cache read ratio、总输入成本和质量，不能只报单请求路由价格。

### GAP-005：三优化模式

- 现状：用户配置“强/中/弱模型”，没有表达这次更重成本还是质量。
- 最小方案：三份版本化 profile，只改变 classifier 阈值与效用权重；UI 清楚显示价格可能性。
- 理想方案：连续成本旋钮 + 预算上限 + 管理员允许范围。
- 验收：切换 profile 后规则、Hint、exact、local-only、capability 结果不变；只有学习层选档分布改变。

### GAP-006：会话与 harness 兼容性门禁

- 现状：`ModelCapability` 能过滤 tools、vision、JSON Schema、context window；它没有表达“当前 Agent harness 是否为这个模型准备了合适 prompt/tool shape”，也没有表达“这个模型能否接管由另一模型产生的 history”。
- 影响：跨家族候选可能在线路协议上成功，却在旧 tool call、reasoning block、编辑格式或系统指令上产生隐性质量回退。
- Cursor 借鉴：模型变化时同时切模型专属 harness，并对接管历史加定制指令；即便如此，Cursor 仍建议无理由时整段会话保持一个模型。
- 最小方案：由 Agent connector 声明 `harness_family` 和 `conversation_compatibility_group`。自动路由只在同组内切换；跨组默认保持原目标或明确拒绝，不能由 token-station 擅自重写 system prompt。
- 理想方案：connector 明确提供可审计的 handoff capability，能切 prompt/tools、规范化历史或启动新 subagent/context；Router 记录来源/目标组和 handoff 策略。
- 验收：含工具调用和 reasoning history 的多轮 fixture 中，不兼容候选永不被静默选择；兼容切换后旧工具不会被误调用；每次跨组 handoff 有明确 receipt。

## 7. Cursor 的隐藏能力、未公开项和容易漏测处

### 7.1 已确认但容易漏掉

- Balance/Intelligence 与 Cost 不是同一计费语义；前两者按实际模型价格，平均约为 Cost 的两倍，官方称视模式可达 2–4 倍。
- 文档对新 classifier 的明确表述针对 Balance/Intelligence；Cost 使用“previous Auto routing logic”。不能笼统写“三档都由同一个 classifier 驱动”。
- 新 Router 发布前，有用户猜测旧 Auto 是 heuristic；Cursor 维护者只确认它平衡 intelligence、cost efficiency、reliability 且不支持插入自定义 routing LLM，**没有确认 heuristic 架构**。另一维护者回复提到 provider 负载和区域可用性。这些只说明 Cost 的历史产品目标，不能还原当前 Cost 算法，也不能证明它与 Balance/Intelligence 共用 classifier。[旧 Auto 讨论](https://forum.cursor.com/t/llm-routing-instead-of-heuristic-based-auto-selection/154050)
- Enterprise blocking 过多模型可能禁用 Router；Grok 4.5 是 Router 工作所需的性价比底座。
- Balance/Intelligence 的实际模型默认隐藏，但 Teams/Enterprise 可由管理员开启显示。
- Router 对 Teams 默认启用；Enterprise 需要管理员手动启用，并可按 organization group 配置。
- Router 仅明确面向 Teams/Enterprise；个人计划的 Auto 是邻近但不同的旧/个人产品行为。

### 7.2 未公开

- classifier 是传统 ML、encoder、embedding router、小生成模型还是混合架构；
- 参数量、推理位置、p50/p95 延迟和分类成本；
- task taxonomy、各类别样本量和模型选择分布；
- 新模型冷启动、router/harness/model 三者版本绑定和重训/回滚节奏；
- router accuracy、regret、置信度和拒绝/回退机制；
- cache sticky 的运行时公式；
- AFC 的完整定义、偏差校正和实验分桶细节；
- 训练流量如何获得“其他候选模型本来会怎样”的反事实标签；
- 60 万训练请求是否含 Privacy Mode 流量、各套餐/组织/语言/仓库规模的占比；
- 60% 数字的任务分布、独立复现材料和外部审计。

因此，“Cursor 也用小模型读意图”必须标为**未知**。能确认的最强表述只是“Cursor 运行一个经真实请求训练的 classifier”。

AFC/keep-rate 也不是 ground truth。官方对代理指标的解释有价值，但存在可预见偏差：

- 用户换到下一功能可能是真满意，也可能是放弃；继续纠正可能是失败，也可能只是正常追加范围；
- 与 Router 博客使用相同正负例的 satisfaction evaluator 由另一个 LLM 读取用户响应分类，会带来 judge/model/prompt 漂移；官方未逐字确认它就是 AFC 的全部实现；
- keep rate 会受 formatter、rebase、后续重构、任务类型和观测窗口影响；Cursor 的[团队 Analytics 文档](https://cursor.com/docs/account/teams/analytics)也承认自动格式化会让行签名失效；
- Router 博客没有公布是否对任务难度、用户、组织、语言和会话长度做分层或偏差校正。

因此更可靠的复刻不是优化单一“少追问”，而是像 Cursor 的 CursorBench 文章所述，看多个在线代理指标是否一致移动，并保留离线正确性/测试结果作为护栏。

### 7.3 社区反馈聚类

社区内容是线索，不等同官方事实：

1. **费用迁移惊讶**：Router 发布讨论中有用户称旧 Auto 心智是固定低价，而迁移到 Balance 后出现 frontier/API 费率，要求把旧行为清楚命名为 Auto Cost。[发布讨论](https://forum.cursor.com/t/introducing-cursor-router/166386)
2. **模型不可见**：用户要求每消息显示实际模型、effort 和费用，以定位质量回退；官方旧回复确认当时没有可见入口，新 Router 文档随后提供管理员开关。[可见性请求](https://forum.cursor.com/t/show-which-model-handled-each-step-when-using-auto-mode/164163)
3. **缓存难解释**：用户看到 Auto cache read/write 为零；讨论指出不同模型的缓存与 dashboard 展示口径不同，不能仅凭两个字段判断 Router 没缓存。[缓存反馈](https://forum.cursor.com/t/auto-mode-prompt-caching-not-working/154654)
4. **想要用户自定义规则**：用户希望主 Agent 按任务类型使用自己的模型规则，而不是只接受 Cursor 的黑盒选择。[规则请求](https://forum.cursor.com/t/feature-request-user-configurable-model-routing-rules-for-the-main-agent/166260)
5. **多端 rollout 不齐**：新 `auto-smart` 在 ACP/JetBrains 出现过 ID/usage-limit 报错，也有客户端只暴露旧 Auto、没有 Optimize For 的反馈。这些是发布早期信号，不能推断总体发生率。[ACP rollout bug](https://forum.cursor.com/t/php-storm-cursor-auto-model-set-as-auto-smart/166460)、[JetBrains mode limitation](https://forum.cursor.com/t/why-doesnt-jetbrains-integration-offer-auto-cost/166529)

这些反馈正好对应 token-station 的现有优势：用户控制、实际模型可见、决策原因可审计；不应为了模仿 Cursor 而放弃。

### 7.4 效果图应该怎样读

Router 博客的公开图表可以支持“Cursor 在自己的线上指标上观察到一个成本—满意度 Pareto 前沿”，不能支持独立可复现的普遍收益：

- 质量—成本图把各方案相对 Opus 4.8 归一化，但没有公开样本量、误差条/置信区间、流量分配、任务构成和底层 routed-model mix；
- 三个 early-access 客户的 31%、32%、52% 节省来自两周试用，并以“同一流量全部按 Opus 4.8 API 价格”作为反事实计价，不是实际 paired model run；
- cost per commit 图公开值为 Composer 2.5 `$2.06`、Grok 4.5 `$2.91`、Balance `$4.63`、Intelligence `$6.76`、GPT-5.6 Sol `$6.77`、Opus 4.8 Thinking `$7.34`、Fable 5 `$12.69`；这些对比点不等于官方确认它们全都在 Router 实时候选池；
- Cost mode 没有和 Balance/Intelligence 同口径的质量—成本 A/B 点；文档只说它沿用 previous Auto logic。

另外，Cursor 明确说 Privacy Mode 下 Customer Data 不会用于 Cursor 训练，而关闭时可用 codebase data、prompts、editor actions 等改进 AI 和训练模型；Enterprise 默认开启 Privacy Mode。[Data Use](https://cursor.com/data-use)。Router 博客没有交代 60 万训练请求的隐私模式构成，因此训练集与 Router 企业目标用户之间是否存在分布差异是**未验证项**，不能自行假设已解决。

## 8. 真实实测与证据

### 8.1 token-station 路由内核

- 前置：commit `d96ea4d`，未修改配置。
- 交付复核：研究期间分支前进到 `fce85da`；对 `router-core/protocol/metrics/desktop/docs/product/README.zh-CN/plugins/official` 比较 `d96ea4d..fce85da`，无相关文件变化，因此源码定位和既有测试结果没有被这次 merge 失效。
- 操作：`cargo test -p token-station-router-core`。
- 结果：50 个单元测试 + 23 个集成测试全部通过。
- 覆盖：规则/Hint 优先级、阈值边界、重放、prompt 不进入决策、健康排序、exact model、strict/ordered recovery、local-only 和能力拒绝。

### 8.2 metrics 与桌面回执

- 操作：`cargo test -p token-station-metrics`。
- 结果：3/3 通过。
- 操作：Vitest 运行 `RecentReceipts`、`TierRouteEditor`、`AgentRoutePage`、legacy UI 相关测试。
- 结果：4 个 test files、36 项测试全部通过。

### 8.3 CLI 路由表

- 操作：`cargo run -q -p token-station-cli -- --config apps/cli/example-config.json rule list`。
- 实际结果：按顺序打印 rules、hint routes、`score >= 40` heuristic、default pool 和 pool members。
- 影响：证明 CLI 能解释静态路由表；它还不能对任意输入输出 classifier probability 或候选效用。

### 8.4 Cursor 3.12.17 本机只读 UI

- 前置：本机已有 Cursor 3.12.17 且正在运行；未确认账户套餐。
- 操作：只展开当前聊天的模型选择器，未发送请求、未改设置。
- 实际结果：只显示 `Auto — Balanced quality and speed, recommended for most tasks` 开关；未显示 Cost/Balance/Intelligence selector。
- 结论：该本机账户/界面不能实测 Teams/Enterprise Router 三档，不能把个人 Auto 当作新 Router 的完整 UI 证据。

### 8.5 Cursor 3.12.17 本机静态客户端证据

- 操作：只读检索安装包 `workbench.desktop.main.js`；未反编译后端、未改文件。
- 文件 SHA-256：`23ed0b021697bbe8a3f472cfeae0a0c26c9e2cc631b32a3aaa781da963ec6565`。
- 安装包文件时间：2026-07-17，早于 Router 公开发布 5 天；只能视为 launch 前客户端 schema 快照。
- 发现：Router 控制面 protobuf/字段包括 `SmartAutoSettings(enabled, optimize_for, show_routed_model)`、required model 的 blocked/severity、group/team 的 `models_auto_only`、catalog 的 `visible_in_routed_model_view` 与 usage `routed_model`。
- 解释：与官方文档/SDK 的动态 catalog、团队策略、底层模型展示一致，提升了控制面结论置信度；这些字段不包含 classifier 网络、特征或训练算法，不能用来宣称已还原服务端原理。

### 8.6 无法实测

- 没有 Teams/Enterprise 管理员权限，未测试 Router enable、mode restrictions、model allow/block、soft/hard impose 和 underlying-model visibility。
- 为避免费用和外部写入，未发送对照 Agent 请求，无法独立测量路由模型、缓存、质量和成本。
- Reddit/X 无可用登录态后端；Reddit 搜索摘要仅作为弱线索，没有用于关键事实。

### 8.7 有测试账户后，怎样黑盒逼近原理

单看一次 routed model 没有意义。最低可辨识实验应固定 Cursor 版本、仓库快照、完整 history 和 profile，每格重复运行，并让管理员开启 underlying model：

| 实验 | 只改变什么 | 观察 | 能回答 | 仍不能回答 |
|---|---|---|---|---|
| Query paraphrase | 表述，不改任务 | model 分布、延迟 | 对措辞是否敏感 | 内部 feature |
| Context ablation | 附件/历史/仓库片段 | model 分布 | context 是否实际影响 | context 如何编码 |
| Complexity ladder | 同领域逐步增加 horizon | 各 mode 的转折点 | 是否存在稳定档位边界 | classifier 类型 |
| Domain pair | UI 与后端同复杂度任务 | model 分布 | domain specialization | 是 taxonomy 还是直接打分 |
| Multi-turn switch | 同会话连续简单→复杂→简单 | switch、cache、首 token 延迟 | sticky/hysteresis 的外显行为 | 服务端公式 |
| Allowlist removal | 管理员逐个 block 候选 | fallback/拒绝 | 候选约束与 required model | 全量私有池 |
| Fixed-model paired run | 相同任务固定各候选 | 测试结果、成本、keep proxy | Router regret 的样本估计 | 官方训练过程 |

应预注册主指标、样本量和停止规则，保存 `auto-smart profile / routed model / model version / effort / context mode / Cursor version / harness surface`。否则模型更新、动态 catalog 和 harness rollout 会把实验污染成不可复现的时间切片。

## 9. 问题清单（P0–P3）

本次限定在 Cursor Router 对比，没有发现新的 P0。P1 是产品能力缺口，不代表应越过当前仓库已有的发布可靠性 P0。

| ID | 类别 | 状态 | 用户影响 | 根因 | 最小动作 |
|---|---|---|---|---|---|
| BUG-001 | correctness | broken | 具体模型请求可能被选档替换；简单开开关又会把 `auto` 当精确模型 | 路由分支看全局 bool，没有实现文档所述的 `auto`/具体值语义 | 明确契约并加两类端到端测试 |
| GAP-001 | feature | partial | 选档可能与真实 profile 胜率不一致 | 手工 heuristic 无 outcome 标签 | 本地分层胜率 classifier shadow |
| GAP-002 | engineering | missing | 无法证明“不降质” | receipts 没有任务结果 join | paired eval + 非劣效门 |
| GAP-003 | reliability | partial | 短小高风险任务可能被低估 | complexity 与 risk 混用 | 可选高风险规则模板 |
| GAP-004 | cost | missing | 频繁切模可能吃掉节省 | Router 无会话/cache 输入 | 先加观测与切换原因 |
| GAP-005 | ux | partial | 用户只能配档，不能表达成本偏好 | 缺优化 profile | 三模式版本化 profile |
| GAP-006 | correctness | missing | 跨模型家族接管可产生隐性工具/历史不兼容 | 只有 wire capability，没有 harness/history compatibility | connector 声明兼容组并 fail closed |
| DOC-001 | docs | broken | 用户看到的打分公式已过期 | 代码取消 per_tool 后文档未同步 | 修文档并加检查 |
| DOC-002 | trust | broken | 把目标误认为已验证事实 | 旧北极星文案未按冻结决策收口 | 改为“目标”，附证据门槛 |
| OBS-001 | observability | partial | classifier 上线后难定位错路由 | receipt schema 尚未扩展 | 增解释字段 |
| ENT-001 | enterprise | missing | 团队无法统一模式 | 本地 MVP 无组织控制面 | 未来企业版再做 |

## 10. 最小方案 / 理想方案

### 10.1 最小方案：不改变信任边界的 Router V1

数据结构建议：

```rust
struct LearnedRouteObservation {
    task_type: ClosedTaskType,
    classifier_version: ArtifactDigest,
    mode_profile: ClosedModeProfile,
    classifier_latency_micros: u64,
    strong_win_probability_ppm: u32,
    threshold_ppm: u32,
    confidence_ppm: u32,
    previous_target: Option<ConfiguredTargetId>,
    selected_quality_profile: ArtifactBoundQualityProfileId,
    previous_execution_profile: Option<ExecutionProfileId>,
    selected_execution_profile: ExecutionProfileId,
    previous_harness_family: Option<ClosedHarnessFamily>,
    selected_harness_family: ClosedHarnessFamily,
    handoff_strategy: ClosedHandoffStrategy,
    switch_penalty_micros: Option<u64>,
    candidate_count: u32,
    exclusion_counts: ClosedExclusionCounts,
}
```

约束：所有字符串来自配置或闭集；正文只在当前进程内用于特征/推理，不能进入 receipt。artifact 缺失、版本未知、验签失败、confidence 低时回 heuristic。

### 10.2 理想方案：本地多目标效用路由

```text
utility(execution_profile, task, session)
  = expected_success(execution_profile.quality_profile, task)
  - lambda(mode) * expected_cost(execution_profile, tokens, cache)
  - mu(mode)     * expected_latency(execution_profile, health/load)
  - nu(mode)     * switch_cost(previous_profile, execution_profile)
```

`local_only`、能力、组织 allowlist、会话兼容和高风险最低档属于硬约束，不进入可被权重抵消的 utility。若 connector 不声明可执行的 handoff，跨 `harness_family` 候选在计算 utility 前就应排除。

## 11. 差异化定位与 Roadmap

### 必须补齐

- 用 outcome 校准路由，而不只靠长度/词表；
- 质量宣称的评测门；
- 文档与实现一致；
- classifier 与 cache/switch 的可解释 receipt；
- 跨模型会话的 harness/history 兼容门禁。

### 值得借鉴

- Cursor 的三模式心智模型；
- task/domain × model 的效果画像；
- 完整会话而非孤立 prompt 的评测；
- 成本 per completed work，而不仅 per token/request。

### 应形成差异化

- 本地 classifier、正文不落盘；
- 用户模型池和强覆盖规则；
- actual model 默认可见；
- 路由可重放、artifact 可验签回滚；
- 分类器失效时确定性降级，而不是服务端静默换策略。

### 暂不做

- 云端全量行为学习；
- 默认隐藏模型；
- 强制某一家模型作为成本底座；
- 自动上传 code keep rate；
- 未经独立评测就宣称 Cursor 同等级 30–60% 节省。

Roadmap：

| 阶段 | 交付 | 依赖 | 风险 | 退出条件 |
|---|---|---|---|---|
| R0 | 明确 `auto`/exact 契约、文档纠偏、风险模板草案、receipt/harness compatibility schema | 产品契约决策 | 修错分支可破坏现有 Agent 配置 | `auto` 和具体模型 E2E 均通过，文档与代码一致 |
| R1 | 离线 paired eval 与标签审计 | 任务集、候选模型预算 | judge/分布偏差 | 数据卡与基线完成 |
| R2 | 本地 classifier shadow；只观察跨组建议，不执行 | artifact 签名/加载、connector 兼容声明 | 延迟、漂移、错误 handoff | 非劣效 + 成本目标达到，跨组建议可审计 |
| R3 | 三模式 opt-in 灰度 | UI/profile schema | 用户误解费率 | 每请求解释和回滚可用 |
| R4 | cache/switch 效用 | 真实 telemetry | provider 缓存口径差异 | 总会话成本非劣 |
| R5 | 企业策略控制面 | 托管产品与签名策略 | 远程状态/隐私 | 本地可验证、可拒绝 |

## 12. 验收矩阵

| 维度 | 最小用例 | 验收标准 |
|---|---|---|
| Override | 显式模型、关键词规则、Agent Hint | classifier 不得覆盖，receipt 原因准确 |
| Model sentinel | `model="auto"` 与一个已配置具体模型 | `auto` 进入选档；具体名只精确路由或拒绝，永不换名 |
| Capability | tools/vision/json/context 不满足 | 候选被硬排除，不能用高 utility 绕过 |
| Privacy | prompt 中放 canary | DB、日志、artifact 输入缓存均找不到 canary |
| Artifact | 缺失、损坏、签名错、旧版本 | fail closed 到 heuristic，用户收到解释 |
| Reproducibility | 同 config/artifact/features/candidates 回放 | 选择和分数完全一致 |
| Quality | 按 task type 对照固定强模型 | 达预设非劣效界限并报告 CI |
| Cost | 同任务集比较固定强模型 | 报总 token/cost，包含 cache miss 与重试 |
| Router overhead | classifier 开/关同一批回放 | 单独报告 classifier p50/p95 与总请求 TTFT，不把上游延迟混入 |
| Switching | 多轮会话模型切换 | 每次切换有 reason；切换率和 cache ratio 可统计 |
| Harness handoff | 含旧工具调用/reasoning block 的跨模型历史 | 同兼容组可续跑；跨组默认拒绝或使用 connector 明示 handoff，绝不静默替换 |
| Modes | Cost/Balance/Quality 切换 | 只改变学习层分布，不改变硬约束 |
| Drift | 候选模型版本更新 | 旧 artifact 拒绝或明确兼容；需重评后启用 |
| UI | 错路由调查 | 单请求能看到模型、档、概率、阈值、排除和 attempts |
| Rollback | 灰度质量下降 | 一次动作回 heuristic，已有 receipts 保持可读 |

## 13. 证据账本、局限和未验证项

### 13.1 证据账本

| ID | 结论 | 类型 | 位置 | 日期/版本 | 置信度 | 复现 |
|---|---|---|---|---|---|---|
| E-001 | Cursor Router 每请求分类 task type/complexity | OFFICIAL_DOC | [Router docs](https://cursor.com/docs/cursor-router) | 2026-07-27 读取 | high | yes |
| E-002 | 60 万+训练、数百万 A/B、AFC/keep rate | OFFICIAL_DOC | [Router blog](https://cursor.com/blog/router) | 2026-07-22 | high（存在）；效果数字为 vendor-reported | yes |
| E-003 | Cost 与 Balance/Intelligence 计费不同，当前单价见 §3.1 | OFFICIAL_DOC | [Models & Pricing](https://cursor.com/docs/models-and-pricing) | 2026-07-28 | high | yes |
| E-004 | Cursor 会把 prompt/code context 发给模型/推理方 | OFFICIAL_DOC | [Privacy governance](https://cursor.com/docs/enterprise/privacy-and-data-governance) | 2026-07-27 | high | yes |
| E-005 | 旧 Auto 精确算法不公开、同 session 可逐 step 换模型 | STAFF_FORUM_REPLY | [Forum](https://forum.cursor.com/t/auto-model-mechanism-in-cursor/159697) | 2026-05，Router 发布前 | medium-high；不代表新 classifier | no |
| E-006 | 费用、透明度、缓存、自定义规则和多端 rollout 是反馈主题 | USER_FEEDBACK | Cursor forum 讨论聚类 | 2026-03–07 | medium | no |
| E-007 | 当前 third layer 是 heuristic 占位 | LOCAL_CODE | `router-core/src/config.rs:46-51` | `d96ea4d` | high | yes |
| E-008 | 路由优先级、精确模型、recovery、能力门禁 | LOCAL_CODE | `router-core/src/route.rs:109-429` | `d96ea4d` | high | yes |
| E-009 | 决策与 attempts 不含正文 | LOCAL_CODE/TEST | `decision.rs`、`metrics/src/lib.rs` | `d96ea4d` | high | yes |
| E-010 | router-core 73 项测试通过 | MANUAL_TEST | `cargo test -p token-station-router-core` | 2026-07-27 | high | yes |
| E-011 | metrics 3 项、桌面 36 项通过 | MANUAL_TEST | cargo + Vitest | 2026-07-27 | high | yes |
| E-012 | 本机 Cursor 只显示单一 Auto 开关 | MANUAL_TEST | Cursor 3.12.17 model picker | 2026-07-27 | medium；账户套餐未知 | yes |
| E-013 | 路由文档仍称 per_tool 参与打分 | LOCAL_DOC vs CODE | `docs/product/路由机制.md:47-63` vs `config.rs:286-289` | `d96ea4d` | high | yes |
| E-014 | 文档承诺具体模型直连，但 exact 由默认关闭的 bool 控制，且未排除 `auto` | LOCAL_DOC vs CODE | `docs/product/路由机制.md:11-15`、`README.zh-CN.md:53`、`config.rs:52-65`、`route.rs:116-120` | `d96ea4d` | high | yes |
| E-015 | SDK Router ID 为 `auto-smart`，profile 参数必须显式且应动态发现 | OFFICIAL_DOC | [TypeScript SDK](https://cursor.com/docs/sdk/typescript#cursor-router) | 2026-07-28 读取 | high | yes |
| E-016 | SDK profile selection sticky，但底层 routed model 可逐请求变化 | OFFICIAL_DOC | [TypeScript SDK](https://cursor.com/docs/sdk/typescript#per-run-model-override) | 2026-07-28 读取 | high | yes |
| E-017 | Cursor 为模型/版本定制 prompt 和 tools，跨模型接管需专门处理 history | OFFICIAL_DOC | [Agent harness](https://cursor.com/blog/continually-improving-agent-harness) | 2026-04-30 | high | yes |
| E-018 | 推理平台包含统一 gateway、跨 provider failover、backpressure/admission control | OFFICIAL_DOC | [Model Routing & Inference 职位](https://cursor.com/careers/software-engineer-model-routing-inference) | 2026-07-28 读取 | medium-high；职位职责不是调用链规范 | yes |
| E-019 | 本机客户端含 Router 设置、必需模型、catalog 可见性和 routed-model usage schema | LOCAL_BINARY_STATIC | Cursor 3.12.17 `workbench.desktop.main.js`，SHA-256 `23ed0b…ec6565` | 文件 2026-07-17；审计 2026-07-28 | medium-high；launch 前客户端控制面 | yes |
| E-020 | Privacy Mode 禁止 Cursor 用 Customer Data 训练；关闭时可用 prompts/editor actions；Router 训练集构成未披露 | OFFICIAL_DOC + ABSENCE | [Data Use](https://cursor.com/data-use)、[Privacy governance](https://cursor.com/docs/enterprise/privacy-and-data-governance) + Router blog | 2026-07-28 | high（政策）；训练集差异为未知 | yes |
| E-021 | 官方效果图缺原始样本、CI、流量分配与 routed-model mix | VENDOR_CHART_AUDIT | Router blog 三张图 | 2026-07-28 | high（公开材料缺失） | yes |
| E-022 | 同类 learned router 可用 ranking/MF/BERT/causal LLM 等不同实现，故 `classifier` 不推出“小 LLM” | PRIMARY_RESEARCH | [RouteLLM, ICLR 2025](https://proceedings.iclr.cc/paper_files/paper/2025/file/5503a7c69d48a2f86fc00b3dc09de686-Paper-Conference.pdf) | 2025 | high（算法族）；不代表 Cursor | yes |
| E-023 | Cursor 用 LLM 读取用户后续响应估计满意度，并建议多个在线指标一致移动 | OFFICIAL_DOC | [Agent harness](https://cursor.com/blog/continually-improving-agent-harness)、[CursorBench](https://cursor.com/blog/cursorbench) | 2026 | high；judge 细节仍未知 | yes |
| E-024 | Cursor 的 AI 行检测可在本机做、只上传计数元数据，formatter 可使签名失效 | OFFICIAL_DOC | [Team Analytics](https://cursor.com/docs/account/teams/analytics) | 2026-07-28 读取 | high；Router keep-rate 是否复用同管线未知 | yes |
| E-025 | Cursor 自报移除 GPT-5-Codex reasoning traces 在 CursorBench 下降 30% | OFFICIAL_DOC/VENDOR_TEST | [Codex model harness](https://cursor.com/blog/codex-model-harness) | 2025-12-04 | medium-high；不可外推其他模型 | yes |
| E-026 | 生产路由只有被选模型反馈，反事实标签需额外设计 | PRIMARY_RESEARCH | [BaRP](https://arxiv.org/abs/2510.07429) | 2025 | high（一般问题）；不代表 Cursor | yes |
| E-027 | 研究期间 HEAD 前进，但路由相关审计路径无差异 | LOCAL_GIT_DIFF | `git diff d96ea4d..fce85da -- crates/{router-core,protocol,metrics} apps/desktop docs/product README.zh-CN.md plugins/official` | 2026-07-28 | high | yes |

### 13.2 关键证据判断

- **已确认**：Cursor Router 是训练 classifier；新 Router 有三优化模式；官方效果数字存在；token-station 当前不是 learned classifier。
- **合理推断**：Cursor 的主要竞争优势来自真实流量和模型效果画像，而不是某一种 classifier 架构。
- **架构交叉佐证**：Router 依赖动态候选控制、模型专属 harness 与统一推理平台；但官方没有公布它们在单请求中的精确调用顺序。
- **不能确认**：Cursor 使用小 LLM、BERT、embedding、树模型或混合架构；如何生成跨模型反事实训练标签；运行时是否显式 sticky；60% 是否可迁移到 token-station 用户分布。
- **厂商自报**：60% savings、30–50% early-access savings、cost per commit 和“无质量下降”。公开资料未提供可下载数据、模型分配比例或独立复现。

### 13.3 研究限制

- Cursor Router 为闭源托管服务，没有源码调用链可审计。
- 没有 Teams/Enterprise 测试账户，管理面与实际三模式请求未实测。
- 未进行付费 A/B，无法比较真实模型、质量与 cache。
- 社区样本新且数量有限，不能推断总体发生率。
- Privacy Mode 与 60 万训练请求的交集未披露，无法验证训练分布是否代表 Enterprise 默认隐私流量。
- 当前报告没有审计独立训练仓 `orion-shield-train`；本仓文档中对其样本量的描述仅作为计划线索，启动分类器项目前必须单独验证。

### 13.4 来源

官方：

- [Cursor Router 文档](https://cursor.com/docs/cursor-router)
- [Introducing Cursor Router](https://cursor.com/blog/router)
- [Cursor Router Changelog](https://cursor.com/changelog/router)
- [Cursor Models & Pricing](https://cursor.com/docs/models-and-pricing)
- [Cursor Privacy and Data Governance](https://cursor.com/docs/enterprise/privacy-and-data-governance)
- [Cursor Data Use & Privacy Overview](https://cursor.com/data-use)
- [Cursor TypeScript SDK：Cursor Router](https://cursor.com/docs/sdk/typescript#cursor-router)
- [Continually improving our agent harness](https://cursor.com/blog/continually-improving-agent-harness)
- [How we compare model quality in Cursor](https://cursor.com/blog/cursorbench)
- [Cursor Team Analytics](https://cursor.com/docs/account/teams/analytics)
- [Improving Cursor’s agent for OpenAI Codex models](https://cursor.com/blog/codex-model-harness)
- [Software Engineer, Model Routing & Inference](https://cursor.com/careers/software-engineer-model-routing-inference)

算法边界参考（不代表 Cursor 实现）：

- [RouteLLM: Learning to Route LLMs with Preference Data（ICLR 2025）](https://proceedings.iclr.cc/paper_files/paper/2025/file/5503a7c69d48a2f86fc00b3dc09de686-Paper-Conference.pdf)
- [Learning to Route LLMs from Bandit Feedback: One Policy, Many Trade-offs（BaRP）](https://arxiv.org/abs/2510.07429)

社区线索：

- [Cursor Router 发布讨论](https://forum.cursor.com/t/introducing-cursor-router/166386)
- [Auto 实际模型可见性请求](https://forum.cursor.com/t/show-which-model-handled-each-step-when-using-auto-mode/164163)
- [Auto prompt caching 反馈](https://forum.cursor.com/t/auto-mode-prompt-caching-not-working/154654)
- [用户自定义主 Agent 路由规则请求](https://forum.cursor.com/t/feature-request-user-configurable-model-routing-rules-for-the-main-agent/166260)
- [Cursor Auto 模型机制讨论](https://forum.cursor.com/t/auto-model-mechanism-in-cursor/159697)
- [旧 Auto / custom router 讨论（heuristic 为用户猜测）](https://forum.cursor.com/t/llm-routing-instead-of-heuristic-based-auto-selection/154050)
- [ACP `auto-smart` rollout bug](https://forum.cursor.com/t/php-storm-cursor-auto-model-set-as-auto-smart/166460)
- [JetBrains 暂无 Optimize For selector](https://forum.cursor.com/t/why-doesnt-jetbrains-integration-offer-auto-cost/166529)

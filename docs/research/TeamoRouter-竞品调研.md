# TeamoRouter 竞品调研

> 调研日期：2026-07-09
> 缘起：[AI 编码工具配置切换器竞品全景](./AI编码工具配置切换器竞品全景.md) §五 把 TeamoRouter 标为「🔴 最高优先级：平台侧 + 客户端的在位竞争者，可能是最直接的业务竞品」，并建议专项核实。
> **本文的结论是：该判断应当撤回。** TeamoRouter 不构成 token-station 平台侧的实质威胁。
>
> ⏳ 状态：一手证据部分已完成核实；定价实证、Teamo Desktop 是否真实发布、团队功能实证三项的深度调研仍在进行，见 §六。

---

## 零、结论

**TeamoRouter 是一个域名注册仅 8 周的 API 中转站，运行的是 `new-api` 开源中转站软件的 fork，同时公开发布了 Reddit 刷号操作手册。**

它自称「不同于典型的 API 中转服务（Unlike typical API relay services）」，而它的 GitHub 组织里就放着 `QuantumNous/new-api` 的 fork——那正是中文圈开中转站的标准开源软件。这一条由 GitHub API 直接证实，不需要推断。

对 token-station 的含义有两层，方向相反：

1. **威胁下调**：它不是平台侧在位者。「集中计费 + 团队管理 + BYOK + 智能路由 + 用量分析」这串词出现在它的赞助文案里，但截至目前没有任何一项拿到独立产品证据；其可信度受制于 8 周域名年龄、自造评测与刷号手册。原竞品全景里「护城河来自平台侧、而平台侧已有在位者、故护城河不安全」的推论，**前半段仍成立，后半段撤回**。
2. **威胁上调（另一个方向）**：cc-switch README 里那近 30 家中转商的赞助位是一门**真实存在且值钱**的流量生意。真正值得警惕的不是 TeamoRouter 这一家，而是「**11.5 万 star 的本地客户端 = 中转站的获客入口**」这一结构。token-station 若走「注册前置 + 闭源分发」，等于主动放弃了这个入口，而竞争对手可以用返佣把它买下来。

---

## 一、厂商自述（cc-switch README 赞助区，逐字）

> Thanks to TeamoRouter for sponsoring this project! TeamoRouter is an enterprise-grade Agentic LLM gateway built for developers, AI teams, and businesses. Without requiring any subscriptions, it lets you access Claude Code, Codex, Gemini CLI, OpenAI Codex, and other popular AI agents through a single unified API, while offering API pricing at **discounts of up to 90%**. **Unlike typical API relay services**, TeamoRouter aggregates hundreds of official model providers and trusted infrastructure partners, including OpenAI, Anthropic, Vertex, Azure, and AWS bedrock. Every provider is verified for 100% Agent protocol compatibility, cache performance, and request traceability, ensuring stable quality **instead of reverse-engineered or diluted endpoints**. The platform delivers near-official TTFT, **99.6% SLA**, enterprise-scale throughput up to **5,000 QPM**, and industry-leading cache hit rates... TeamoRouter also offers enterprise features including **centralized billing, team management, BYOK, smart routing, usage analytics**, dynamic provider optimization, and dedicated support. For an even simpler experience, **Teamo Desktop** lets you use Claude Code, Codex, Gemini CLI, and other popular AI agents with one-click setup—no API key management or manual gateway configuration required. Register via [this link](https://teamorouter.com/?utm_source=cc_switch&utm_medium=referral&utm_campaign=ai_directory) as a new user to receive 10% off your first top-up.

**这整段是营销文案，不是证据。** 下面逐条对质。

---

## 二、一手证据（本人经 GitHub API / WHOIS 直接核实，2026-07-09）

### 2.1 它运行的就是它声称自己不是的软件 ⛳ 决定性

```
repos/teamo-lab/new-api
  fork   = true
  parent = QuantumNous/new-api
  source = QuantumNous/new-api
  pushed = 2026-07-09T06:34:18Z   ← 今天仍在推送
  license= AGPL-3.0
```

`QuantumNous/new-api`（41,600★）是**多租户 API 聚合网关**——有用户体系、令牌配额、渠道分组、计费，是中文圈搭建 API 中转站的标准开源软件（本仓库 [竞品全景 §一](./AI编码工具配置切换器竞品全景.md) 已将其归入 L3 网关层）。

赞助文案说 *"Unlike typical API relay services"*。**它不是「不同于中转站」，它运行的就是中转站软件。**

> ⚠️ **AGPL-3.0 合规提示**：new-api 是 AGPL-3.0，带网络 copyleft 条款——通过网络对外提供修改版服务，须向使用者提供对应源码。TeamoRouter 作为商业服务运营其 fork，是否履行该义务未经核实。这不是 token-station 的问题，但可作为该竞品的合规风险记录。

### 2.2 域名只有 8 周大

| 域名 | 创建日期 | 到期 | 注册商 |
|---|---|---|---|
| `teamorouter.com` | **2026-05-15** | 2027-05-15 | DNSPod, Inc.（腾讯） |
| `teamolab.com` | 2026-03-10 | 2027-03-10 | DNSPod, Inc. |

主域名注册至今 **不足 2 个月**，且只续了 1 年。一个 8 周大的服务声称 *99.6% SLA*、*enterprise-scale throughput up to 5,000 QPM*、*dedicated support*。

**中转站跑路是这个赛道的典型风险**，而域名年龄与续费年限是最直接的风险信号。

### 2.3 它公开发布了 Reddit 刷号操作手册 ⛳ 决定性

仓库 `teamo-lab/mkt-skills`（MIT），README 逐字：

> Marketing skills for Claude Code — battle-tested playbooks for community growth and user acquisition.
>
> ### reddit-karma-farming
> How to warm up a new Reddit account and gain karma safely. Covers:
> - New account warm-up timeline (0 → 500+ karma in 4 weeks)
> - Which subreddits accept 0-karma accounts
> - Safe posting frequency per account age
> - **Anti-AI-detection writing rules**
> - Karma threshold reference (what each level unlocks)
> - **Red flags that trigger spam detection**
> - Technical implementation (JS fetch API / Python requests)

组织仓库描述亦逐字含 *"Reddit ops, cold start playbooks"*。

**直接后果**：任何关于 TeamoRouter 的 Reddit / 社区「用户好评」都必须视为**可能是自造的**。这条从根本上污染了该产品的第三方口碑证据——**不是「暂时没找到差评」，而是「好评本身不可采信」**。

### 2.4 「第三方评测」是自己写的

`teamo-lab/blog` 仓库 `has_pages = true`，描述逐字为 *"TeamoRouter Blog — Save up to 50% on LLM API costs with smart routing for OpenClaw. Guides, tutorials, and comparisons."*

因此 `teamo-lab.github.io` 域名下的所有对比文章——包括搜索结果里出现的 *"TeamoRouter vs ClawRouter vs OpenRouter: Which LLM Router Should Your OpenClaw Use in 2026?"* 与 *"What Is the Best LLM Router for OpenClaw in 2026? A Detailed Comparison"*——**是厂商自述，不是第三方评测**。

### 2.5 组织画像：全新、空心、营销导向

`teamo-lab` 组织创建于 **2026-02-15**，13 个公开仓库，**star 数全为 0 或 1**。

| 仓库 | ★ | 创建 | 性质 |
|---|---|---|---|
| `new-api` | 0 | 2026-04-03 | **QuantumNous/new-api 的 fork**（中转站软件） |
| `mkt-skills` | 0 | 2026-03-28 | **Reddit 刷号手册** |
| `blog` | 0 | 2026-03-25 | 自营「评测」博客（GitHub Pages） |
| `teamorouter-skill` | 1 | 2026-03-25 | 安装脚本 |
| `teamoclaw` | 0 | 2026-03-05 | OpenClaw 相关 |
| `Awesome-ccIDE` | 0 | 2026-05-09 | macOS IDE |
| 其余 7 个 | 0 | — | skill / CI 测试 / 杂项 |

全 GitHub 搜索 `teamorouter` 仅 **4 个仓库**命中（含 `sophiaashi/teamorouter-resources`，0★，无 license，2026-03-25 创建后再无推送）。

**没有任何 `teamo-desktop` 仓库** —— Teamo Desktop 不开源，其是否真实发布尚待核实（§六）。

### 2.6 自家口径互相矛盾：90% vs 50%

| 出处 | 折扣声称 |
|---|---|
| cc-switch README 赞助文案 | *"discounts of **up to 90%**"* |
| `teamo-lab/blog` 仓库描述 | *"Save **up to 50%** on LLM API costs"* |
| `teamo-lab/teamorouter-skill` 仓库描述 | *"**up to 50%** off official prices"* |
| `sophiaashi/teamorouter-resources` 仓库描述 | *"cut AI API costs **30-50%**"* |

**同一家厂商，付费赞助位上的数字是自家仓库口径的近两倍。** 折扣声称随渠道浮动，是典型的营销注水特征。

---

## 三、声称 vs 证据

| 厂商声称 | 证据等级 | 核实结果 |
|---|---|---|
| "Unlike typical API relay services" | 🔴 **被推翻** | 运行 `new-api` fork（GitHub API 实证） |
| "aggregates hundreds of official model providers... OpenAI, Anthropic, Vertex, Azure, AWS Bedrock" | ⚪ 未找到依据 | 无任何一手证据；new-api 的渠道机制不区分官方与转售 |
| "not reverse-engineered or diluted endpoints" | ⚪ 未找到依据 | 无第三方实测；自证 |
| "99.6% SLA" | ⚪ **纯自报** | 无独立验证；8 周域名不足以支撑 SLA 统计 |
| "up to 5,000 QPM" | ⚪ **纯自报** | 无独立验证 |
| "near-official TTFT" / "industry-leading cache hit rates" | ⚪ **纯自报** | 无独立验证 |
| "discounts of up to 90%" | 🟠 **自相矛盾** | 自家仓库口径为 50% / 30–50% |
| "centralized billing, team management, BYOK, smart routing, usage analytics" | ⏳ 待核实 | 无独立产品证据（截图 / 文档 / 定价页）；注：**new-api 本身自带用户体系、令牌配额、渠道分组与计费**，这些「企业功能」可能就是 new-api 的开箱功能 |
| "Teamo Desktop... one-click setup" | ⏳ 待核实 | 无开源仓库；是否真实发布未证实 |
| 第三方评测背书 | 🔴 **被推翻** | `teamo-lab.github.io` 为厂商自有 |
| 社区口碑 | 🔴 **不可采信** | 厂商公开发布 Reddit 刷号 + 反 AI 检测手册 |

图例：🔴 被一手证据推翻 ｜ 🟠 自相矛盾 ｜ ⚪ 纯自报，无独立验证 ｜ ⏳ 深度调研进行中

---

## 四、对 token-station 的竞争含义

### 4.1 撤回「平台侧在位者」判断

[竞品全景 §五](./AI编码工具配置切换器竞品全景.md) 曾写：

> **集中计费 + 团队管理 + BYOK + 智能路由 + 用量分析 + 自带桌面客户端** —— 这是 token-station 平台侧 + 客户端的完整画像 ... 本产品「护城河来自平台侧」的推论并不安全。

**撤回后半句。** 那串功能词全部来自厂商付费赞助位的营销文案，无一项有独立产品证据；而其技术底座是一个开源中转站软件的 fork。一个 8 周大的 new-api 套壳，不构成 token-station 平台侧的实质威胁。

「护城河来自平台侧」这个观察本身**仍然成立**——它是从 CCR / CCS / TokenTracker 的对比中得出的，与 TeamoRouter 无关。

### 4.2 但结构性威胁是真的：入口生意

真正该记住的不是这一家，而是这个结构：

- cc-switch README 的赞助区，逐字抓取到 **29 家** API 中转 / 网关服务商，每家带专属返佣码（如 PackyCode 的 `cc-switch` 促销码、Cubence 的 `CCSWITCH`、ZetaAPI 的 `/go/ccs`）；
- 一个 MIT、匿名可装、11.5 万 star 的本地客户端，成了整个中转站行业的**获客入口**；
- 中转商愿意为这个入口付费，说明它的转化价值已被市场定价。

**对本产品的直接含义**：token-station 拍板的「注册前置 + 闭源分发」（[个人模式本地客户端需求 §五](../features/个人模式本地客户端需求.md)），等于主动放弃了这个入口位置——而竞争对手可以用返佣把它买下来。原需求文档把闭源的代价记为「人群 2 的获客摩擦」，**这里出现了第二笔代价：放弃了成为他人入口、或占据入口的可能性**。这是一个此前未被记录的取舍面。

### 4.3 差异化候选的检验结果

| token-station 差异化候选 | TeamoRouter 是否具备 |
|---|---|
| 本地 / 离线路由 | ❌ 未找到依据（其形态是云端网关） |
| 本地模型上游（Ollama / vLLM） | ❌ 未找到依据 |
| 独立对账 | ❌ 未找到依据（中转站的商业模式与「让用户独立核验账单」天然冲突） |
| BYOK 与平台账户在同一客户端并存 | ⏳ 待核实（声称有 BYOK，但无证据） |

这三项**仍是空白**，与 [竞品全景 §四](./AI编码工具配置切换器竞品全景.md) 的结论一致。

### 4.4 一条可用的正向情报

「**内容不出本地 + 独立对账**」这类信任叙事，在这个赛道里是**稀缺品**。TeamoRouter 这类玩家的存在（域名 8 周、刷号手册、口径矛盾）恰恰说明市场里充斥着不可验证的承诺。token-station 的[闭源信任验收机制](../features/个人模式本地客户端需求.md#二密钥边界平台批发-key-永不下发到本地)（网络边界可查、离线可用、同步白名单、本地审计输出）如果做实并对外讲清楚，是**可以直接对比出优势的**——前提是我们自己不犯同类毛病（自报数字、自造评测）。

---

## 五、风险与观察点

| 项 | 说明 |
|---|---|
| **跑路风险** | 域名 8 周、续费 1 年、DNSPod 注册、无公司主体信息。中转站跑路在此赛道为高频事件 |
| **AGPL 合规** | 商业运营 `new-api`（AGPL-3.0）fork，是否向用户提供源码未核实 |
| **口碑污染** | 厂商公开发布含「反 AI 检测写作规则」的 Reddit 刷号手册；其社区好评不可采信 |
| **自造评测** | `teamo-lab.github.io` 上的「vs OpenRouter」类对比为厂商自有内容 |
| **数字注水** | 折扣声称 90%（赞助位）vs 50%（自家仓库） |

**不建议**将 TeamoRouter 列入季度跟踪。**建议跟踪的是结构**：cc-switch 赞助位的构成变化——如果某家赞助商开始具备真实的团队管理与桌面客户端产品证据，那家才值得专项。

---

## 六、待补（深度调研进行中）

以下三项尚未拿到一手证据，深度调研 workflow 仍在运行，结果回来后补入本文：

1. **定价模型实证**：是充值 credits 还是订阅？「up to 90%」适用于哪些模型？折扣来源是官方批发、转售、还是缓存/降级？
2. **Teamo Desktop 是否真实发布**：形态、下载渠道、支持工具、路由粒度（应用级切换 vs 请求级分流）、BYOK 与平台账户能否并存、key 存储位置。
3. **team management / centralized billing 的产品证据**：定价页、文档、截图；以及它们是否只是 `new-api` 的开箱功能。

此外未核实：公司主体、注册地、团队、成立时间、融资；`sophiaashi` 与 `teamo-lab` 的关系。

---

## 参考来源

**一手（本人于 2026-07-09 直接核实）**

- `gh api repos/teamo-lab/new-api` → `fork=true, parent=QuantumNous/new-api, pushed=2026-07-09T06:34:18Z, license=AGPL-3.0`
- `gh api orgs/teamo-lab` → `created=2026-02-15T05:02:08Z, public_repos=13`
- `gh api repos/teamo-lab/mkt-skills/readme` → reddit-karma-farming 手册全文
- `gh api repos/teamo-lab/blog` → `has_pages=true`
- `whois teamorouter.com` → `Creation Date: 2026-05-15T11:56:45Z, Registrar: DNSPod, Inc.`
- `whois teamolab.com` → `Creation Date: 2026-03-10T12:45:36Z`
- cc-switch README 赞助区逐字文案：https://raw.githubusercontent.com/farion1231/cc-switch/main/README.md

**厂商自述（非证据，仅记录其主张）**

- https://teamorouter.com ｜ https://teamolab.com ｜ https://teamo-lab.github.io/blog/ （**厂商自有，非第三方**）
- https://github.com/teamo-lab ｜ https://github.com/sophiaashi/teamorouter-resources

**第三方独立来源**：截至目前 **未找到任何一个**。（enterprisedna.co 的目录页为聚合型收录，不构成评测。）

**方法说明**：本文的决定性结论（new-api fork、域名年龄、刷号手册、自造评测、口径矛盾）均由 GitHub REST API 与 WHOIS 直接查询取得，不依赖搜索引擎摘要或二手博客。定价与 Teamo Desktop 部分的深度调研（5 路检索 → 抓源 → 3 票对抗验证）仍在进行。

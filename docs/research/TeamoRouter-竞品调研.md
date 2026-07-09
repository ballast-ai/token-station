# TeamoRouter 竞品调研

> 调研日期：2026-07-09
> 缘起：[AI 编码工具配置切换器竞品全景](./AI编码工具配置切换器竞品全景.md) §五 曾把 TeamoRouter 标为「🔴 最高优先级：平台侧 + 客户端的在位竞争者」，并建议专项核实。
> **结论：该「最高优先级业务竞品」判断撤回。** TeamoRouter 是一家 API 中转站，不构成 token-station 平台侧的实质威胁。
>
> 证据分级贯穿全文：**一手证据**（我经 GitHub API / WHOIS / 官网页面直接核实）｜**厂商自述**（README、营销页、自营博客）｜**第三方独立来源**｜**未找到依据**。所有性能与折扣数字均标注是否经独立验证。

---

## 零、结论

**TeamoRouter 是一家域名注册仅 8 周的 API 中转站（relay），其赞助文案在多个可核查点上与自家产品页面直接矛盾。**

但必须同时说清楚一件对它有利的事：**独立第三方检测显示，它在被测端点上没有掺水。** 「架构上是中转站」与「是否降级/伪造模型」是两个独立问题，前者成立，后者在现有证据下**不成立**。

对 token-station 的含义有两层，方向相反：

1. **威胁下调**：它不是平台侧在位者。赞助文案里的「集中计费 + 团队管理 + BYOK + 智能路由 + 用量分析」，在其**自家定价页上一个字都找不到**。其桌面客户端真实存在，但只是把编码工具重定向到网关的「接入壳」，不自带本地路由。原竞品全景「护城河来自平台侧、而平台侧已有在位者」的推论，**前半段仍成立，后半段撤回**。

2. **威胁上调（另一方向）**：cc-switch README 里 29 家中转商的赞助位是一门**真实且被市场定价**的流量生意。真正值得警惕的不是这一家，而是「**11.5 万 star 的本地客户端 = 中转站行业的获客入口**」这个结构。token-station 的「注册前置 + 闭源分发」等于主动放弃这个位置，而对手可以用返佣买下它。

---

## 一、厂商自述（cc-switch README 赞助区，逐字）

> Thanks to TeamoRouter for sponsoring this project! TeamoRouter is an enterprise-grade Agentic LLM gateway... while offering API pricing at **discounts of up to 90%**. **Unlike typical API relay services**, TeamoRouter aggregates hundreds of official model providers and trusted infrastructure partners, including OpenAI, Anthropic, **Vertex, Azure, and AWS bedrock**. Every provider is verified for 100% Agent protocol compatibility... ensuring stable quality **instead of reverse-engineered or diluted endpoints**. The platform delivers near-official TTFT, **99.6% SLA**, enterprise-scale throughput up to **5,000 QPM**, and industry-leading cache hit rates... TeamoRouter also offers enterprise features including **centralized billing, team management, BYOK, smart routing, usage analytics**, dynamic provider optimization, and dedicated support. For an even simpler experience, **Teamo Desktop** lets you use Claude Code, Codex, Gemini CLI... with one-click setup. Register via [this link](https://teamorouter.com/?utm_source=cc_switch&utm_medium=referral&utm_campaign=ai_directory) as a new user to receive 10% off your first top-up.

**这是付费赞助位上的营销文案，不是证据。** 下面逐条对质。

---

## 二、声称 vs 证据（总表）

| 厂商声称 | 判定 | 依据 |
|---|---|---|
| "Unlike typical API relay services" | 🔴 **推翻** | 独立第三方 Veridrop 将其归类为**中转站**；其 GitHub 组织维护活跃的 `new-api` fork |
| "aggregates... **Vertex, Azure, and AWS bedrock**" | 🔴 **推翻** | 其自家定价页的供应商标签仅 `All / OpenAI / Anthropic / Google / DeepSeek / GLM`，**全页无 Vertex / Azure / Bedrock** |
| "not reverse-engineered or **diluted** endpoints" | 🟢 **支持（对它有利）** | Veridrop 加密签名真伪验证 **100% 一致**，被测端点未检出伪造 |
| "discounts of up to **90%**" | 🟠 **自相矛盾** | 自家 GitHub 仓库口径为 "up to **50%**"、"30-50%" |
| "**99.6% SLA**" / "5,000 QPM" / "near-official TTFT" / "industry-leading cache hit rates" | ⚪ **纯自报** | 无任何独立验证；缓存命中率官网自标 *"Measured on a stress-test dataset"*（自测） |
| "**centralized billing, team management, BYOK**" | ⚪ **无产品证据** | 定价页与下载页**均无**这些功能的任何痕迹 |
| "**smart routing**" | ⚪ **机制未知** | 无公开文档；「按价格选最便宜」这一假说在对抗验证中被 **0-3 否决** |
| "**Teamo Desktop**" | ✅ **属实** | macOS `.dmg` + Windows `.msi` 真实可下载（见 §四） |
| 「第三方评测」背书 | 🔴 **推翻** | `teamo-lab.github.io` 为厂商自有仓库（`has_pages=true`） |
| 社区口碑 | 🔴 **不可采信** | 厂商公开发布 Reddit 刷号 + 反 AI 检测手册（见 §三） |

图例：🔴 被证据推翻 ｜ 🟠 自相矛盾 ｜ 🟢 证据支持厂商 ｜ ⚪ 无独立验证 / 无依据 ｜ ✅ 属实

---

## 三、一手证据（本人经 GitHub API / WHOIS 直接核实，2026-07-09）

### 3.1 域名只有 8 周大

| 域名 | 创建日期 | 到期 | 注册商 |
|---|---|---|---|
| `teamorouter.com` | **2026-05-15** | 2027-05-15 | DNSPod, Inc.（腾讯） |
| `teamolab.com` | 2026-03-10 | 2027-03-10 | DNSPod, Inc. |

主域名注册至今**不足 2 个月**，只续 1 年。一个 8 周大的服务在赞助位上声称 *99.6% SLA*、*enterprise-scale throughput*、*dedicated support*。

**中转站跑路是本赛道高频事件**，域名年龄与续费年限是最直接的风险信号。

### 3.2 GitHub 组织维护 new-api 的活跃 fork

```
repos/teamo-lab/new-api
  fork   = true
  parent = QuantumNous/new-api
  pushed = 2026-07-09T06:34:18Z   ← 今天仍在推送
  license= AGPL-3.0
```

`QuantumNous/new-api`（41,600★）是**多租户 API 聚合网关**——用户体系、令牌配额、渠道分组、计费一应俱全，是中文圈搭建 API 中转站的标准开源软件。

> ⚠️ **表述精度**：fork 的存在证明他们维护 new-api 代码，**不直接证明生产环境跑的就是它**。但结合 §三.3 的独立分类与其 `teamo_key` + 钱包绑定的接入模型，「中转站」定性已由第三方独立成立，不依赖这条推断。
>
> ⚠️ **AGPL-3.0 合规**：new-api 带网络 copyleft，商业运营修改版须向使用者提供源码。TeamoRouter 是否履行未经核实。这是该竞品的合规风险记录，非 token-station 的问题。

### 3.3 独立第三方：Veridrop 的检测结果 —— 双向证据

**这是本次调研找到的唯一真正的第三方独立来源**，它同时给出对 TeamoRouter 不利与有利的两面：

| 项 | 结果 |
|---|---|
| 分类 | *"teamorouter.com **中转站**测评"*，收入其「中转站红黑榜」 |
| 得分 | **94 / 100**（"优秀"） |
| 测试日期 | 2026-06-30（OpenAI 协议）、2026-07-02（Claude 协议） |
| 方法 | *"协议字段 + 加密签名 + 长上下文 needle-in-haystack 三层验证"*；*"加密级真伪验证（thinking signature · 业内唯一密码学不可伪造）"* |
| 独立性 | *"代码完全开源"*（AGPL-3.0，`canarybyte/veridrop`）；页面标注 teamorouter.com *"未与 Veridrop 合作"* |

**读法必须精确**：
- Veridrop **确认**了「TeamoRouter 是中转站」——推翻其 *"Unlike typical API relay services"* 的自我定位；
- Veridrop **同时否定**了「它在掺水」——在被测端点上，加密签名一致性 100%，未检出伪造 / 逆向 / 降级端点。它的 *"not diluted endpoints"* 这一条**站得住**。

**「是中转站」不等于「掺水」。** 前者是架构事实，后者是行为指控。混淆两者是竞品分析里最容易犯的错。

### 3.4 赛道系统性风险（非针对 TeamoRouter）

CISPA 论文《**Real Money, Fake Models: Deceptive Model Claims in Shadow APIs**》（arXiv:2603.01919，2026-03-02 提交；作者含 Michael Backes、Yang Zhang）摘要逐字：*"identity verification failures in $45.83\%$ of fingerprint tests"*。

**该数字是对 3 家代表性中转站 24 个端点的市场级发现，与 TeamoRouter 无关。** 它证明的是赛道风险，不是对本竞品的指控——而 TeamoRouter 恰恰是通过了 Veridrop 检测的那一类。

（arXiv ID 与摘要已由本人 fetch 验真，非二手转述。）

### 3.5 它公开发布了 Reddit 刷号操作手册

仓库 `teamo-lab/mkt-skills`（MIT），README 逐字：

> Marketing skills for Claude Code — battle-tested playbooks for community growth and user acquisition.
>
> ### reddit-karma-farming
> How to warm up a new Reddit account and gain karma safely. Covers:
> - New account warm-up timeline (0 → 500+ karma in 4 weeks)
> - Which subreddits accept 0-karma accounts
> - **Anti-AI-detection writing rules**
> - **Red flags that trigger spam detection**
> - Technical implementation (JS fetch API / Python requests)

**后果**：任何关于 TeamoRouter 的 Reddit / 社区「用户好评」都**不可采信**。这不是「暂时没找到差评」，而是「好评本身在证据上无效」。

（实际情况是：Reddit / V2EX / 知乎 / GitHub issues 上**既没有找到真实好评，也没有找到跑路投诉**——其声誉尚未建立。证据缺失，而非清白。）

### 3.6 「第三方评测」是自己写的

`teamo-lab/blog` 仓库 `has_pages = true`，描述逐字 *"TeamoRouter Blog — Save up to 50% on LLM API costs..."*。

因此 `teamo-lab.github.io` 下的所有对比文章——*"TeamoRouter vs ClawRouter vs OpenRouter"*、*"What Is the Best LLM Router for OpenClaw in 2026?"*——**是厂商自述，不是第三方评测**。

### 3.7 组织画像：全新、空心、营销导向

`teamo-lab` 组织创建于 **2026-02-15**，13 个公开仓库，**star 数全为 0 或 1**。全 GitHub 搜索 `teamorouter` 仅 4 个仓库命中。

| 仓库 | ★ | 性质 |
|---|---|---|
| `new-api` | 0 | `QuantumNous/new-api` 的 fork（中转站软件） |
| `mkt-skills` | 0 | Reddit 刷号手册 |
| `blog` | 0 | 自营「评测」博客（GitHub Pages） |
| `teamorouter-skill` | 1 | 安装脚本 |
| 其余 9 个 | 0 | skill / CI 测试 / 杂项 |

**无 `teamo-desktop` 仓库** —— 桌面客户端闭源。

### 3.8 自家口径互相矛盾：90% vs 50%

| 出处 | 折扣声称 |
|---|---|
| cc-switch README 赞助文案 | *"discounts of **up to 90%**"* |
| 官网定价页 | *"Rates **from 10%** official prices"* |
| `teamo-lab/blog` 仓库描述 | *"Save **up to 50%** on LLM API costs"* |
| `teamo-lab/teamorouter-skill` 仓库描述 | *"**up to 50%** off official prices"* |
| `sophiaashi/teamorouter-resources` 仓库描述 | *"cut AI API costs **30-50%**"* |

**同一厂商，付费赞助位上的数字是自家仓库口径的近两倍。** 折扣声称随渠道浮动，是营销注水的典型特征。

---

## 四、产品实态（官网页面直读，2026-07-09）

### 4.1 定价：预付费 credits，折扣随上游成本浮动

定价页（`teamorouter.com/pricing`）逐字：

- *"Pay only for what you use. Your account is billed directly in USD."*
- *"Billed in USD · No minimum spend · Buy credits anytime"* —— **无订阅，预充值 credits**
- *"Rates from 10% official prices, automatically applied by model."*
- *"**Discounts may vary with upstream costs.**"*

最后这句是关键：**折扣随上游成本浮动 = 转售差价模型**，而非「与官方谈成的固定批发折扣」。若真是官方批发协议，价格不会随上游波动。

供应商筛选标签逐字为：`All / OpenAI / Anthropic / Google / DeepSeek / GLM`。

> 🔴 **赞助文案称聚合 Vertex / Azure / AWS Bedrock —— 这三个词在其定价页上完全不存在。** 实际上游包含 DeepSeek、GLM（智谱）等中国厂商。

> 🔴 **定价页上没有 team management、centralized billing、member seats、BYOK 的任何痕迹。** 赞助文案里的「企业功能」清单，在产品页面上零证据。

### 4.2 Teamo Desktop：真实存在，但是「接入壳」

下载页（`teamorouter.com/download`）逐字：

| 项 | 事实 |
|---|---|
| 形态 | macOS `.dmg` + Windows `.msi` 原生应用 |
| Linux | **不提供** |
| 分发主机 | `ama-download.floatai.cn`（中国 CDN，非 GitHub Releases） |
| 开源 | **无源码链接、无开源声明** |
| 支持工具 | *"It plugs TeamoRouter into Claude Code, Codex, and other agents in one click"* |
| BYOK / 本地模型 / Ollama / 团队管理 / 对账 | **全部 not present** |

**它是把编码工具重定向到网关的接入壳，不自带本地路由。** 佐证：官方 Codex 安装文档（`/docs/install-codex`）整篇教用户手改 `~/.codex/config.toml`、`export OPENAI_API_KEY`、去网页 dashboard 建 key，**通篇不提 Teamo Desktop**——桌面端只是把同一套网关接入流程点击化。

> 🔒 **安全说明**：本调研仅读取页面元数据，**未下载、未执行**任何二进制。调研过程中有子 agent 因探测该 CDN 上的二进制而触发安全策略拦截，其相关输出未被采信。

### 4.3 路由粒度、BYOK、smart routing：未知

| 问题 | 结论 |
|---|---|
| 路由粒度（应用级 vs 请求级） | **未找到依据** |
| BYOK 是否支持？与平台账户能否并存？key 存哪里？ | **未找到依据** |
| smart routing 机制（按价格 vs 按质量/难度） | **机制未知**。「按价格选最便宜通道」这一假说在 3 票对抗验证中被 **0-3 否决**，无公开文档支撑任一方向 |

**这三项在赞助文案里都是卖点，在产品证据里都是空白。**

### 4.4 公司主体

> ⚠️ 以下来自调研 agent，**未经本人独立核实**，仅作线索记录：
> 下载 CDN `floatai.cn` 备案为 **北京提莫快跑科技有限公司**（京ICP备2023013274号-1）。创始人对外仅以化名「**夕小瑶**」出现（知乎作者、AI Agent 领域创业者），无可核实真名、履历或融资信息。

---

## 五、对 token-station 的竞争含义

### 5.1 撤回「平台侧在位者」判断

[竞品全景 §五](./AI编码工具配置切换器竞品全景.md) 曾写：

> **集中计费 + 团队管理 + BYOK + 智能路由 + 用量分析 + 自带桌面客户端** —— 这是 token-station 平台侧 + 客户端的完整画像 ... 本产品「护城河来自平台侧」的推论并不安全。

**撤回后半句。** 那串功能词全部来自付费赞助位的营销文案；查其定价页与下载页，**centralized billing、team management、BYOK 一个都找不到**。其技术底座是开源中转站软件，桌面端是网关接入壳。一个 8 周大的中转站不构成 token-station 平台侧的实质威胁。

「护城河来自平台侧」这个观察本身**仍然成立**——它是从 CCR / CCS / TokenTracker 的对比中得出的，与 TeamoRouter 无关。

### 5.2 结构性威胁是真的：入口生意

- cc-switch README 赞助区逐字列出 **29 家** API 中转 / 网关服务商，每家带专属返佣码（PackyCode 的 `cc-switch` 码、Cubence 的 `CCSWITCH`、ZetaAPI 的 `/go/ccs`）；
- 一个 MIT、匿名可装、11.5 万 star 的本地客户端，成了整个中转站行业的**获客入口**，且这个入口已被市场定价；
- **token-station 的「注册前置 + 闭源分发」等于主动放弃这个位置**，而对手可以用返佣买下它。

原需求文档把闭源的代价记为「人群 2 的获客摩擦」。这里出现**第二笔代价**：放弃了成为他人入口、或占据入口的可能性。**这是此前未被记录的取舍面，建议回到 [个人模式本地客户端需求 §五](../features/个人模式本地客户端需求.md) 重新掂量。**

### 5.3 差异化候选的检验结果

| token-station 差异化候选 | TeamoRouter 是否具备 |
|---|---|
| 本地 / 离线路由 | ❌ 无（云端网关 + 接入壳） |
| 本地模型上游（Ollama / vLLM） | ❌ 下载页明确 not present |
| 独立对账 | ❌ 无。且**中转站的转售差价模型与「让用户独立核验账单」天然冲突**——费率随上游成本浮动，用户无从核账 |
| BYOK 与平台账户在同一客户端并存 | ⚪ 无任何证据（声称有 BYOK，产品页零痕迹） |

这三项**仍是空白**，与 [竞品全景 §四](./AI编码工具配置切换器竞品全景.md) 的结论一致。

### 5.4 一条可用的正向情报

「**内容不出本地 + 独立对账**」这类信任叙事，在这个赛道是**稀缺品**。TeamoRouter 的存在方式（8 周域名、刷号手册、口径矛盾、企业功能无证据）说明市场充斥不可验证的承诺。

token-station 的[闭源信任验收机制](../features/个人模式本地客户端需求.md#二密钥边界平台批发-key-永不下发到本地)（网络边界可查、离线可用、同步白名单、本地审计输出）若做实并讲清楚，**可以直接对比出优势**。

**前提是我们自己不犯同类毛病**——不自报未经验证的 SLA 数字，不自造第三方评测。Veridrop 这类**开源、可复现、密码学级**的检测工具，是值得主动送检的对象，而不是回避的对象。

---

## 六、风险与观察点

| 项 | 说明 |
|---|---|
| **跑路风险** | 域名 8 周、续费 1 年、DNSPod 注册、二进制走中国 CDN 而非 GitHub Releases。中转站跑路为赛道高频事件 |
| **AGPL 合规** | 商业运营 `new-api`（AGPL-3.0）fork，是否向用户提供源码未核实 |
| **口碑污染** | 厂商公开发布含「反 AI 检测写作规则」的 Reddit 刷号手册；其社区好评不可采信 |
| **自造评测** | `teamo-lab.github.io` 上的「vs OpenRouter」类对比为厂商自有内容 |
| **数字注水** | 折扣声称 90%（赞助位）vs 50%（自家仓库）vs "from 10% official prices"（定价页） |
| **反向公允** | Veridrop 独立检测 94/100、加密签名一致性 100%——**它没有在掺水**。不应把「中转站」与「骗子」划等号 |

**不建议**将 TeamoRouter 列入季度跟踪。

**建议跟踪的是结构**：cc-switch 赞助位的构成变化——若某家赞助商开始具备**真实的**团队管理与桌面客户端产品证据（定价页 / 文档 / 截图，而非赞助文案），那家才值得专项。

---

## 七、遗留问题

- BYOK 是否真实支持？key 存本地还是云端？——所有来源无证据。
- smart routing 的真实机制？路由粒度是应用级还是请求级？——无公开文档，价格优化假说已被否决。
- team management / centralized billing / usage analytics 是否为真实上线功能？——产品界面零证据。
- 是否会出现针对 TeamoRouter 本身的真实用户实测 / 跑路投诉？——目前声誉未建立，需持续观察。
- 公司主体、创始人真名、融资：仅化名「夕小瑶」与 ICP 备案线索，未独立核实。

---

## 参考来源

**一手证据（本人于 2026-07-09 直接核实）**

- `gh api repos/teamo-lab/new-api` → `fork=true, parent=QuantumNous/new-api, pushed=2026-07-09T06:34:18Z, license=AGPL-3.0`
- `gh api orgs/teamo-lab` → `created=2026-02-15T05:02:08Z, public_repos=13`
- `gh api repos/teamo-lab/mkt-skills/readme` → reddit-karma-farming 手册全文
- `gh api repos/teamo-lab/blog` → `has_pages=true`
- `whois teamorouter.com` → `Creation Date: 2026-05-15T11:56:45Z, Registrar: DNSPod, Inc.`
- `whois teamolab.com` → `Creation Date: 2026-03-10T12:45:36Z`
- https://teamorouter.com/pricing → 定价模型、供应商标签、**无 Vertex/Azure/Bedrock**、**无 team management/BYOK**
- https://teamorouter.com/download → macOS `.dmg` / Windows `.msi`、无 Linux、无源码、CDN `ama-download.floatai.cn`
- cc-switch README 赞助区逐字：https://raw.githubusercontent.com/farion1231/cc-switch/main/README.md

**第三方独立来源**

- **Veridrop**（AGPL-3.0 开源，`canarybyte/veridrop`，页面标注「未与 Veridrop 合作」）：https://veridrop.org/leaderboard/teamorouter.com —— 归类为中转站；94/100；2026-06-30 / 07-02 两次检测；加密签名一致性 100%
- **CISPA 论文**：《Real Money, Fake Models: Deceptive Model Claims in Shadow APIs》，arXiv:2603.01919（2026-03-02；Yage Zhang, Yukun Jiang, Zeyuan Chen, Michael Backes, Xinyue Shen, Yang Zhang）—— *"identity verification failures in 45.83% of fingerprint tests"*。**市场级发现，非针对 TeamoRouter**

**厂商自述（非证据，仅记录其主张）**

- https://teamorouter.com ｜ https://teamolab.com ｜ https://teamo-lab.github.io/blog/ （**厂商自有，非第三方**）
- https://github.com/teamo-lab ｜ https://github.com/sophiaashi/teamorouter-resources

**方法说明**：决定性结论（中转站定性、域名年龄、刷号手册、自造评测、口径矛盾、企业功能无证据）由 GitHub REST API、WHOIS 与官网页面直读取得。深度调研（5 路检索 → 22 源 → 3 票对抗验证）提供了 Veridrop 与 CISPA 论文两条线索，二者均由本人二次 fetch 验真后采信；其自动综合结论中与 `refuted` 列表冲突的部分未采信。全程未下载或执行任何厂商二进制。

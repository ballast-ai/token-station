# AI 编码工具配置切换器 / 本地路由器竞品全景

> 调研日期：2026-07-09
> 上位文档：[cc-switch 桌面配置切换器竞品调研](./cc-switch-桌面配置切换器竞品调研.md)（单点深挖）；本文是其**品类横向展开**，并**修正**了该文的两处结论（见 §四、§五）
> 参照系：[个人模式本地客户端需求](../features/个人模式本地客户端需求.md)、[智能路由能力](../features/智能路由能力.md)
> 与 [开源 token 路由系统全景借鉴](./开源token路由系统全景借鉴.md) 的边界：那篇看的是**服务端网关 / 学习型路由器**（LiteLLM、OpenFugu、RouteLLM），本文看的是**跑在开发者机器上的单机切换器与本地代理**
>
> 数据核实口径：star 数、许可证、最近提交时间均为 2026-07-09 经 GitHub REST API 实测，非引自二手博客。

---

## 零、三句话结论

1. **请求级路由已不是差异化，是入场券。** claude-code-router（35,706★）与 CCS（2,675★）都已实现按 token 数 / 场景把同一会话内的不同请求分流到不同上游。原 cc-switch 单点调研把「请求级混合路由」列为本产品对人群 1 的核心差异化——**该判断作废**（§四）。

2. **「账号制多设备用量归集」也不是空白。** TokenTracker（954★，MIT）已做到 opt-in 云同步 + 按账号合并多机用量视图，且其隐私口径（「只传 token 数与时间戳，绝不传 prompt、response、文件内容」）与本产品需求文档 §三 的字段白名单几乎完全重合。区别只在：**它不在数据路径上**（被动读各工具的本地日志），不做路由（§五）。

3. **cc-switch 已经商业化了，只是不靠付费墙。** 其 README 设 `❤️Sponsor` 专区，逐字列出 **29 家** API 中转 / 网关服务商，每家附专属返佣码——**变现方式是导流返佣**。一个 11.5 万 star 的 MIT 本地客户端，是整个中转站行业的**获客入口**，且这个入口已被市场定价（§五）。

**推论**：本产品相对本品类的护城河，几乎全部来自 **token-station 平台侧的存在**（平台账户上游 + 独立对账），而非本地客户端本身。本地客户端单独与 CCR / CCS 对比，是劣势方（闭源、注册下载 vs MIT、匿名安装）——**且闭源+注册前置还意味着放弃了「入口」这个位置**。

---

## 一、品类分层：按「切换机制」分三层

切换机制决定了工具在不在**数据路径**上，进而决定它能不能做请求级路由、用量统计与故障转移。

| 层 | 切换机制 | 是否在数据路径 | 路由粒度上限 | 代表项目 |
|---|---|---|---|---|
| **L1 配置改写 / 环境变量层** | 改写各工具的原生 JSON/TOML，或注入 env 后再启动工具 | ❌ 否，改完即退出 | 应用级单一活跃供应商 | ClaudeWarp、clother、ccc、ccman |
| **L2 本地代理层** | 起本地进程，把工具 base_url 指向 `127.0.0.1`，请求经本地转发 | ✅ 是 | **请求级 / 按场景分流** | claude-code-router、CCS、cc-desktop-switch、cc-switch（代理模式） |
| **L3 网关 / 协议转换层** | 独立服务（本机或云端），对外暴露统一端点 | ✅ 是 | 池化 / 规则路由 | AIClient2API、one-api / new-api、claude-code-proxy、y-router |
| **L0 旁路观测层** | 不切换、不转发，只读各工具已产出的日志 / SQLite / JSONL | ❌ 否 | — | TokenTracker、CodexBar |

三层的关键差别不是「谁更强」，而是**用户要不要多养一个常驻进程**。L1 零成本但只能应用级；L2/L3 拿到了数据面，代价是进程常驻 + 延迟。L0 零侵入但永远只能事后统计。

> **L3 是否算同类？** one-api（35,589★）/ new-api（41,600★）本质是**多租户 API 聚合网关**——有用户体系、令牌配额、渠道分组、计费，面向「开中转站的人」而非「本机开发者」。它们与本品类共享「多上游聚合」机制，但用户画像与部署形态不同，归入 [闭源商业 token 服务功能面调研](./闭源商业token服务功能面调研.md) 的谱系更合适。本文只做边界标注，不展开。

---

## 二、竞品明细

> star / license / 最近提交 均为 2026-07-09 GitHub API 实测值。

### 2.1 第一梯队

| 项目 | ★ | 许可证 | 最近提交 | 形态 | 切换机制 | 路由粒度 |
|---|---|---|---|---|---|---|
| [farion1231/cc-switch](https://github.com/farion1231/cc-switch) | 114,988 | MIT | 2026-07-09 | Tauri 2 桌面 GUI + 托盘 | 配置改写（可选本地代理） | **应用级**单活跃 |
| [QuantumNous/new-api](https://github.com/QuantumNous/new-api) | 41,600 | AGPL-3.0 | 2026-07-08 | 服务端网关 | 统一端点 | 渠道池（*不同层*） |
| [musistudio/claude-code-router](https://github.com/musistudio/claude-code-router) | 35,706 | MIT | 2026-07-08 | 本地代理（CLI 启动） | 本地代理转发 | **请求级场景路由** |
| [songquanpeng/one-api](https://github.com/songquanpeng/one-api) | 35,589 | MIT | 2026-01-09 | 服务端网关 | 统一端点 | 渠道池（*不同层*） |
| [steipete/CodexBar](https://github.com/steipete/CodexBar) | 17,280 | MIT | 2026-07-09 | macOS 菜单栏 | 不切换，只观测 | —（*L0 观测层*） |
| [justlovemaki/AIClient2API](https://github.com/justlovemaki/AIClient2API) | 8,401 | GPL-3.0 | 2026-07-07 | 本地守护进程 + Web UI | 本地代理转发 | **供应商池 + fallback 链** |
| [SaladDay/cc-switch-cli](https://github.com/SaladDay/cc-switch-cli) | 4,074 | MIT | 2026-07-08 | CLI + TUI（Rust）+ 本地代理 | 配置改写 + 代理 + env + OAuth | 应用级单活跃 |
| [fuergaosi233/claude-code-proxy](https://github.com/fuergaosi233/claude-code-proxy) | 2,718 | MIT | 2026-03-12 | 本地代理 | 协议转换 | 大小模型映射 |
| [kaitranntt/ccs](https://github.com/kaitranntt/ccs) | 2,675 | MIT | 2026-07-08 | CLI + 本地代理 | 本地代理转发 | **请求级场景路由** |

### 2.2 长尾（形态有参考价值）

| 项目 | ★ | 许可证 | 形态 | 切换机制 | 路由粒度 | 备注 |
|---|---|---|---|---|---|---|
| [mm7894215/TokenTracker](https://github.com/mm7894215/TokenTracker) | 954 | MIT | 菜单栏 + 托盘 + 本地 dashboard | 不切换（被动读日志） | —（L0） | **唯一具备账号 + 多设备用量归集的项目**，见 §五 |
| [lonr-6/cc-desktop-switch](https://github.com/lonr-6/cc-desktop-switch) | 770 | MIT | 常驻本地网关 + 管理 UI | 持久本地网关转发 | 按模型槽位别名映射 | Claude Desktop 专用 |
| [Laliet/cc-switch-web](https://github.com/Laliet/cc-switch-web) | 470 | MIT | Web 服务器 + Tauri GUI | 改写原生配置（+proxy） | 应用级单活跃 | 第三方 Web 化；**有自动 failover + 完整用量 Dashboard** |
| [luohy15/y-router](https://github.com/luohy15/y-router) | 381 | MIT | Cloudflare Worker / Docker | 协议转换 | — | **仓库已归档**，见 §六 |
| [jolehuit/clother](https://github.com/jolehuit/clother) | 372 | MIT | CLI（Go 单二进制） | env 注入 + 包装二进制 | 应用级单活跃 | 仅 Claude Code |
| [SakuraByteCore/codexmate](https://github.com/SakuraByteCore/codexmate) | 319 | Apache-2.0 | CLI + Web UI（:3737） | 代理桥接 + 独立本地库 | 应用级单活跃 | 明确宣称 "No telemetry, no cloud accounts" |
| [guyskk/claude-code-config-switcher](https://github.com/guyskk/claude-code-config-switcher) | 83 | MIT | CLI | env 注入（`--settings`） | 应用级单活跃 | 仅 Claude Code |
| [belingud/claudewarp](https://github.com/belingud/claudewarp) | 76 | LGPL-3.0 | CLI + PySide6 GUI | env 注入 | 应用级单活跃（`current_proxy`） | 仅 Claude |
| [2ue/ccman](https://github.com/2ue/ccman) | 51 | 未声明 | CLI | 配置改写 | 应用级单活跃 | 四工具配置管理 |

> **血缘澄清**：`cc-switch-cli`（4,074★）自称 "CLI fork"，署名 "Original architecture and core functionality from farion1231/cc-switch"，但维护者是另一人，**上游 README 完全未提及或背书它**——它是**独立第三方 fork，不是官方 CLI 版**。`cc-switch-web`（470★）同理，是第三方 Web 化实现。

---

## 三、特色功能拆解（回答问题 1）

### cc-switch —— 广度与「一站式」

- 覆盖面最广：**7 款工具**（Claude Code、Claude Desktop、Codex、Gemini CLI、OpenCode、OpenClaw、Hermes），50+ 内置供应商预设；
- 唯一真源 `~/.cc-switch/cc-switch.db`（SQLite），配置改写用临时文件 + rename 原子写入；
- 自 v3.9 起内置本地代理数据面：热切换、格式转换、**自动故障转移 + 三态熔断器**（Closed/Open/Half-Open，失败阈值 Claude 8 / 通用 4）、供应商健康监控；
- 跨供应商用量统计（支出 / 请求数 / token）；
- 云同步 = **文件同步**（WebDAV / S3 / Dropbox / OneDrive / iCloud / 坚果云 / NAS），**无账号体系**；
- MCP / 提示词 / 技能统一面板，会话管理器。

**差异化本质**：不是路由做得深，而是**把七个工具的配置面统一了**——这是 11 万 star 的真实来源。

### claude-code-router —— 请求级场景路由的事实标准

- 本地代理默认监听 `http://localhost:8080`，通过 `ANTHROPIC_BASE_URL` 把 Claude Code 指过来；
- **六类场景路由**，每个请求实时判定：

  | 场景 | 触发条件 | 典型配置 |
  |---|---|---|
  | `default` | 常规编码 | 主力模型 |
  | `background` | 模型名含 haiku（低成本任务） | 便宜 / 本地模型 |
  | `think` | 开启 thinking | 推理模型 |
  | `longContext` | **输入 token > 60000**（阈值可配） | 大上下文模型 |
  | `webSearch` | 带搜索工具 | 支持搜索的模型 |
  | `image` | 带图像 | 多模态模型 |

- 支持自定义 router 脚本按请求内容分流；
- 可在**同一工作流内混用** OpenAI 兼容 / Anthropic / Gemini / OpenRouter / DeepSeek / SiliconFlow / Moonshot / Mistral / Z.AI / Bailian 及自定义供应商。

**差异化本质**：它就是「本地版的规则路由器」，且已被 3.5 万 star 验证。

### CCS（kaitranntt/ccs）—— 与本产品重叠度最高

- 本地 Anthropic 兼容代理：`/v1/messages` → OpenAI chat-completions → 再翻译回 SSE；
- 产品主张直接把 cc-switch 当靶子：**「停止改写配置文件、不打断活跃会话」**；
- `profile:model` 选择器 + `proxy.routing` 场景路由：`background→ollama`、`think→deepseek-reasoner`、`longContext(>60000)→openrouter/gemini-2.5-pro`；
- **原生支持本地模型**（Ollama / llama.cpp）；
- 支持 Codex / Kiro / Claude / Kimi 的 **OAuth 登录**（用订阅额度而非 API key）。

**差异化本质**：CLI 形态 + 请求级路由 + 本地模型 + OAuth。**这是与本产品人群 1（BYOK 多上游开发者）与人群 2（本地模型玩家）重叠度最高的项目**——且它是 MIT、匿名可装。

### AIClient2API —— 把「客户端专属额度」变成 API

- 本地守护进程 + Web 管理界面（Dashboard / 配置 / 供应商池 / 实时日志）；
- 把只能在客户端里用的模型额度（Gemini、Antigravity、Codex、Grok、Kiro）封装成本地 OpenAI 兼容接口；
- **供应商池路由**：多账号轮询、遇 429 / 不健康自动切换、跨类型 fallback 链（`gemini-cli-oauth → gemini-antigravity`）。

**差异化本质**：套利型产品——把订阅额度转成 API 额度。其 README 自报的「99.9% 可用性」是营销数字，未采信；但池化与 fallback 有代码级实证（`provider-pool-manager.js` 的 `acquireSlotWithFallback()`）。

### TokenTracker —— 唯一的「账号 + 多设备归集」

- **L0 观测层**：被动读取各工具已产出的文件（SQLite / JSONL / OTEL export / session log），覆盖 **25 款**工具；不做代理、不做切换、不在数据路径上；
- 默认 100% 本地、无账号、无网络调用；
- **opt-in 云同步**：登录后把多台机器（laptop + desktop + server）的用量合并为单一视图；
- 上传口径逐字为：**"Only token counts and timestamps. Never prompts, responses, or file contents."**
- 形态：CLI + 本地 dashboard（`localhost:7680`）+ macOS 菜单栏 / Windows 托盘 + 桌面小组件。

---

## 四、路由粒度全景（回答问题 2）

**结论：CCR 的 `default/background/think/longContext` 场景路由，算请求级路由。**

判据：路由器对**每一个进来的请求**按其属性（token 数、模型名、thinking 标志）实时判定目标上游，而不是查一个静态的「当前活跃供应商」。CCR 核心代码 `packages/core/src/gateway/claude-code-router-plugin.ts` 每请求计算 `tokenCount` 并喂给 `resolveConfiguredRouteDecision`——这是货真价实的按请求分流。

三档粒度：

| 档 | 定义 | 项目 |
|---|---|---|
| **① 应用级单活跃** | 一个工具同一时刻只有一个上游生效；切换 = 改配置 / 换 env | ClaudeWarp、cc-switch（含代理模式）、cc-switch-cli、cc-switch-web、codexmate、clother、ccc、ccman |
| **② 请求级 / 场景路由** | 每请求按属性分流到不同上游，同一会话内混用多供应商 | **claude-code-router**、**CCS** |
| **③ 池化 / 规则网关** | 供应商池 + fallback 链 / 按槽位模型别名 | AIClient2API、cc-desktop-switch、one-api / new-api |

### 三个必须澄清的伪信号

判断竞品是否真的进入路由数据面时，以下三者**都不是**请求级路由：

1. **为单一活跃供应商配置大小模型名。** ClaudeWarp 的 `BIG_MODEL`/`SMALL_MODEL`、cc-switch v3.15 的 sonnet/opus/haiku 角色映射，只是**在同一个上游内做模型名翻译**——上游还是那一个。
2. **故障转移。** cc-switch-web 的 "Backup Auto-failover"、cc-switch 的熔断器队列，是「主上游挂了整体切到备用」，任一时刻仍只有一个活跃上游。**failover 是可用性机制，不是路由粒度。**
3. **本地代理只做协议适配。** codexmate 与 cc-switch-cli 都起了本地代理，但代理只负责 Anthropic↔OpenAI 格式转换，不做请求级分流——**在数据路径上 ≠ 做了请求级路由。**

同理，cc-switch 即便开启代理，转发逻辑仍是「识别请求来源应用 → 查该应用当前启用的**那一个**供应商 → 转发」，属 ①。

### 对本产品的直接影响 ⚠️

原 [cc-switch 单点调研](./cc-switch-桌面配置切换器竞品调研.md) §六 的判断是：

> 本产品对人群 1 的差异化收窄为：请求级混合路由（vs 应用级切换）、多上游统一在一条数据面内并存、对账核验。

**这条现在站不住了。** 请求级混合路由已被 CCR（35.7k★）与 CCS（2.7k★）实现并普及。它不是差异化，是**入场券**。

该文的「跟踪信号：代理模式出现按模型 / 按规则路由 → 重叠面扩大」——**这个信号不必等 cc-switch 演进就已在品类内触发了**，只是触发者是别的项目。以 cc-switch 单一项目为跟踪对象，是原调研的方法论盲区。

本产品对人群 1 的真实差异化只剩：

| 候选差异化 | 是否成立 | 说明 |
|---|---|---|
| 请求级混合路由 | ❌ **作废** | CCR / CCS 已有 |
| 本地模型上游（L3 隐私） | ❌ **作废** | CCS 原生支持 Ollama / llama.cpp |
| 三类上游并存 | 🟡 **部分成立** | CCS 有 BYOK + 本地模型 + OAuth 订阅，**但没有平台账户上游**——因为它没有平台 |
| 账号制多设备用量归集 | 🟡 **部分成立** | TokenTracker 已做（见 §五），但它不在数据路径上 |
| 独立对账（平台账单 vs 本地指标库 diff） | ✅ **成立** | 无任何竞品具备，因为无任何竞品有平台侧 |

后两条都**依赖平台侧存在**。

---

## 五、演进方向与商业化（回答问题 3）

### 两条并行向量

1. **GUI 降门槛**：cc-switch（Tauri 桌面）、ClaudeWarp（PySide6）、AIClient2API（Web 控制台）、codexmate（Web UI）、cc-switch-web（Web 复刻）。cc-switch 11 万 star 证明这条路天花板最高。
2. **路由数据面下沉**：CCR / CCS 为纯本地路由器；cc-switch 从纯配置改写**加装**了带故障转移 / 熔断 / 健康监控的本地代理；AIClient2API 做供应商池。

**关键信号：连 GUI 配置切换器（cc-switch）都已越过配置层进入数据面。** 「只改配置不碰请求」的 L1 形态正在被淘汰——不碰数据面就做不了用量统计与故障转移，而这两项是留存点。

### 账号体系 / 多设备用量归集：已被 TokenTracker 占据

原以为空白，**实际不是**：

| | TokenTracker | 本产品个人本地版 |
|---|---|---|
| 账号体系 | opt-in（默认无账号、纯本地） | 注册前置（下载即注册） |
| 多设备用量归集 | ✅ 按账号合并多机视图 | ✅ 账号制元数据归集 |
| 上传白名单 | "Only token counts and timestamps. Never prompts, responses, or file contents." | 设备标识 / 时间段 / 上游名 / 模型名 / token 数 / 延迟 / 状态码 / 规则 ID / 成本估算 / 版本 |
| 是否在数据路径 | ❌ 被动读日志 | ✅ 本地代理 |
| 路由 | ❌ 无 | ✅ 请求级 |
| 覆盖工具数 | 25 | 取决于 adapter |
| 许可 / 分发 | MIT、匿名安装 | 闭源、注册下载 |

**它的隐私叙事与我们几乎一字不差，但它是 MIT 且零门槛。** 这对人群 2（隐私敏感者）的获客摩擦，与 cc-switch 对人群 1 的摩擦是同一性质的问题。

本产品的位置仍然独特：TokenTracker 从**用量观测侧**逼近，本产品从**路由数据面侧**逼近——两者在中间相遇。差别是 TokenTracker 拿不到路由决策、上游归因与成本口径（它只能读工具吐出来的数字），而我们在数据路径上，能记录「路由命中规则 ID」这类它拿不到的字段。**这一点是可辩护的，但需要在产品叙事里说清楚，否则用户会问「为什么不用 TokenTracker」。**

### 商业化：已经发生了，形态是返佣而非付费墙

原判断「品类无商业化」**错误**。核实结论：

- **cc-switch 工具本体**：MIT、免费、无账号、无付费墙 —— 这部分成立；
- **但其 README 设 `❤️Sponsor` 专区**，逐字列出 **29 家 API 中转 / 网关服务商**，每家附专属折扣码（如 PackyCode: *"enter the `cc-switch` promo code during first recharge to get 10% off"*；Cubence 用 `CCSWITCH`；ZetaAPI 用 `/go/ccs`）。**cc-switch 的变现方式是导流返佣。** 同类的 `ccc`（83★）也带 GLM / MiMo 的推荐返利码。
- **注意 `ccswitch.ai` ≠ 官方站**。官方 README 逐字声明 *"The Only Official Website: ccswitch.io"*。`ccswitch.ai` 是同一款开源 App 的**第三方非官方仿制落地页**（其 og:image 仍指向 `http://localhost:4321/og.svg`），不卖东西、无账号、无订阅，**不能作为品类商业化的证据**。

### 真正重要的不是某一家赞助商，而是「入口生意」这个结构

cc-switch 的赞助商之一 **TeamoRouter**，其赞助文案自称带 *centralized billing, team management, BYOK, smart routing, usage analytics*，还有 *Teamo Desktop* 桌面客户端——读上去正是 token-station 平台侧 + 客户端的完整画像。

**该线索已专项核实，结论是虚警**：TeamoRouter 的域名注册仅 8 周，其 GitHub 组织运行的是 `QuantumNous/new-api`（中转站开源软件）的 fork，且公开发布了 Reddit 刷号手册。它不是平台侧在位者。详见 **[TeamoRouter 竞品调研](./TeamoRouter-竞品调研.md)**。

但这次核实暴露了一个**结构性**事实，比单个竞品更重要：

- cc-switch README 赞助区逐字列出 **29 家** API 中转 / 网关服务商，每家带专属返佣码；
- 一个 MIT、匿名可装、11.5 万 star 的本地客户端，成了整个中转站行业的**获客入口**，且这个入口已被市场定价；
- **本产品「注册前置 + 闭源分发」等于主动放弃这个入口位置**，而竞争对手可以用返佣把它买下来。

原需求文档把闭源的代价记为「人群 2 的获客摩擦」。这里出现**第二笔代价**：放弃了成为他人入口、或占据入口的可能性。这是此前未被记录的取舍面。

---

## 六、其他值得注意的信号

1. **y-router 已归档**（381★，MIT，最后提交 2026-01-11）。纯协议转换代理这一层正在被**内置了格式转换能力的路由器**吃掉——CCR、CCS、cc-switch 代理都自带 Anthropic↔OpenAI 转换。**独立协议桥不再是一门生意**，这对本产品 `protocol` crate 的定位是提示：**协议转换是必备底座，不是卖点。**

2. **cc-switch 生态长出形态分身**：`cc-switch-cli`（4,074★，第三方 fork）与 `cc-switch-web`（470★）。一个 GUI 项目被复刻成 CLI 与 Web，说明**管理面形态是用户偏好而非产品本质**——这反过来支持本产品「命令行唯一本地管理面」的拍板：形态可以后补，数据面先立住。但也提示，若本产品闭源，别人**无法**这样为你补形态。

3. **CCS 把「不改配置文件」当卖点打 cc-switch**。产品叙事上，「改写用户配置文件」正在从「便利」变成「侵入」。本产品走本地代理路线，天然站在这一叙事的正确一侧。

4. **OAuth 订阅额度接入正在成为标配**（CCS 支持 Codex / Kiro / Claude / Kimi OAuth；AIClient2API 整个产品建立在此之上；cc-switch-cli 也有托管 OAuth 账号复用）。这类「用订阅额度而非 API key」的上游，在本产品需求文档的三类上游里**没有对应位置**——它既不是 BYOK（不是 API key），也不是平台账户（不是 token-station 的账户）。**这是一个需求空白，建议评估是否补第四类上游。**

5. **codexmate 明确宣称 "No telemetry, no cloud accounts"** 并把 "Zero cloud, local-first control plane" 写进项目描述。「无遥测」正在成为该品类的**营销卖点**，而非默认假设。本产品「指标库默认开启」即便纯本地，也需在文案上主动防御。

---

## 七、跟踪信号（触发重估）

| 信号 | 含义 | 优先级 |
|---|---|---|
| CCR 或 CCS 引入账号体系 + 多设备用量归集 | 直接侵入本产品仅存的非平台差异化 | 🔴 高 |
| TokenTracker 增加供应商切换 / 路由能力 | 从 L0 观测层跨入 L2 数据面，与本产品正面相遇 | 🟠 中高 |
| **cc-switch 赞助位构成变化** | 若某赞助商开始具备**真实**的团队管理 + 桌面客户端产品证据，那家才值得专项（TeamoRouter 已核实为虚警） | 🟠 中高 |
| cc-switch 代理模式出现按模型 / 按规则分流 | 品类第一大项目进入请求级路由（跟踪 `proxy/provider_router.rs`、`model_mapper.rs`） | 🟡 中（已非独家信号） |
| 任一项目出现托管端点 / 订阅 | 从工具竞品升级为业务竞品 | 🟡 中 |
| cc-switch 出现 Pro / 付费版 | 品类**直接**商业化破冰（返佣模式已存在） | 🟢 低 |

建议每季度复核，跟踪对象**从 cc-switch 单点扩展为 CCR + CCS + TokenTracker 三线**，外加 cc-switch 赞助位构成这一结构性指标。

> **方法论教训**：TeamoRouter 曾因一段付费赞助文案被误判为「最高优先级业务竞品」。厂商自述**不是证据**——凡「集中计费 / 团队管理 / 智能路由」这类功能词，须见到定价页、文档或产品截图方可入账。参见 [TeamoRouter 竞品调研 §三](./TeamoRouter-竞品调研.md) 的声称-证据对照表。

---

## 参考来源

**一手（GitHub 仓库 / 官方文档）**

- cc-switch：https://github.com/farion1231/cc-switch ｜ 官网 https://ccswitch.io（README 逐字声明为唯一官方站）｜ 用户手册 `docs/user-manual/zh/4-proxy/4.2-routing.md`、`4.3-failover.md`
- claude-code-router：https://github.com/musistudio/claude-code-router ｜ 路由配置文档 https://musistudio.github.io/claude-code-router/docs/server/config/routing/
- CCS：https://github.com/kaitranntt/ccs ｜ https://docs.ccs.kaitran.ca
- AIClient2API：https://github.com/justlovemaki/AIClient2API
- TokenTracker：https://github.com/mm7894215/TokenTracker
- cc-switch-cli：https://github.com/SaladDay/cc-switch-cli ｜ cc-switch-web：https://github.com/Laliet/cc-switch-web
- codexmate：https://github.com/SakuraByteCore/codexmate ｜ clother：https://github.com/jolehuit/clother ｜ ccc：https://github.com/guyskk/claude-code-config-switcher
- ClaudeWarp：https://github.com/belingud/claudewarp ｜ cc-desktop-switch：https://github.com/lonr-6/cc-desktop-switch ｜ y-router：https://github.com/luohy15/y-router（已归档）

**元数据**：star / license / pushed_at 于 2026-07-09 经 GitHub REST API `repos/{owner}/{repo}` 实测。

**方法说明**：5 路并行检索 → 22 个来源抓取 → 105 条声明抽取 → 25 条经 3 票对抗验证（25 confirmed / 0 refuted）；此后由 3 路定向核实 agent 复核 TokenTracker、ccswitch.ai / 商业化、5 个长尾项目的路由粒度；全部量化字段以 GitHub API 复核。

**已知偏差与修正**：
- 二手博客普遍引用 cc-switch 的陈旧 star 数（89k / 104k）；本文一律以 API 实测值 **114,988** 为准。首轮自动调研曾采信 89k，已修正。
- 首轮自动调研结论「品类未出现账号体系 / 商业化」**已被推翻**：TokenTracker 有 opt-in 账号 + 多设备归集；cc-switch 靠赞助返佣变现。
- AIClient2API 的「99.9% 可用性」为厂商自报，未采信。
- TokenTracker 云同步的后端形态（自建服务 vs 用户自备网盘）README 未逐字言明；因其含全球排行榜，推断为项目方自建云，**属推断而非实证**。
- one-api / new-api 归属「服务端聚合网关」层，本文未展开。
- TeamoRouter 曾据 cc-switch README 赞助文案被列为「🔴 最高优先级业务竞品」，**该判断已于 2026-07-09 专项核实后撤回**——其为 8 周新域名 + `new-api` fork + 自造评测，见 [TeamoRouter 竞品调研](./TeamoRouter-竞品调研.md)。
- 赞助商家数：首轮 agent 估为「40+」，本文以逐字抓取核定的 **29 家**为准。

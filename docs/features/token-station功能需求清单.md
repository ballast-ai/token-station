# token-station 完整功能需求清单

> 场景：token-station 定位「统一 token API 服务 + 智能模型路由器」（独立产品，类 OpenRouter）。本文对标**闭源商业服务**（OpenRouter/Portkey/Helicone）与**开源自托管网关**（LiteLLM/new-api/Bifrost/LibreChat）两侧的完整功能面，收口出 token-station 该做哪些功能、每个功能域归 token-station 还是 [glm5.2-platform](../../../glm5.2-platform/README.md)、必备等级几何。
>
> 整理日期 2026-07-05。素材来源：[闭源商业token服务功能面调研](../research/闭源商业token服务功能面调研.md)、[开源token路由系统全景借鉴](../research/开源token路由系统全景借鉴.md)、[OmniRoute 分析](../research/omniroute-开源系统借鉴分析.md)、[层A 计费网关详细报告](../../../glm5.2-platform/docs/research/开源token路由调研-层A-计费网关详细报告.md)。路由方法学与透明能力细节不在此重复，见 [智能路由能力](./智能路由能力.md) 与 [路由透明与用户信任](./路由透明与用户信任.md)。

---

## 零、两条判定原则（先立规矩，避免功能塞错项目）

**原则 A —— 两个产品各自完整、可独立销售**：token-station 与 [glm5.2-platform](../../../glm5.2-platform/README.md) 是**两个能各自独立卖、各自发展用户**的产品——**各有一套完整的账号/鉴权与计费/额度/充值体系**（用户群可不同，账号不绑定、鉴权不互通，2026-07-05 拍板）。一个只买 token-station（路由器）不碰 GLM-5.2 的用户，能在 token-station 里充值、订阅、看账单；一个只用 glm5.2-platform（自营模型）不用路由器的用户同理。**不存在「token-station 依附 glm5.2 计费」这回事**（这是本文 v1 的错误假设，已修正）。类比：OpenRouter 有自己的 credits 体系，不依附任何单一模型供应商的计费。

据此，功能归属只看**产品能力边界**，不再有「计费归 glm5.2」的默认：
- **token-station 自有**：统一 API 门面、模型/供应商聚合、质量侧+供给侧路由、路由透明、BYOK 直连、多租户，**以及自己的一整套计费**（credits/订阅/充值/额度/发票/限额）。即「**这个路由器产品怎么独立运营、独立收钱**」。
- **glm5.2-platform 自有**：GLM-5.2 部署与产能溢出、自营模型的信任/数据留存框架，**以及它自己的一整套计费**。即「**自营模型这门生意怎么独立运营、独立收钱**」。
- **两套计费不共用、不互相扣减**。唯一交汇点是「token-station 路由到 glm5.2-platform」时的结算（glm5.2 只是 token-station 众多上游之一），见 §六。

**原则 B —— 治理 ≠ 内容安全（功能域划分）**：OpenRouter 的实践印证——「谁能用什么模型、花多少、数据能否留存」（治理 guardrail）和「PII/审查/越狱」（内容 guardrail）是两套东西，本清单分列在 §5 与 §7，不混在一个「安全」大筐。

> **等级图例**：🔴 必备（MVP 就要）· 🟡 差异化（拉开与竞品差距的点）· 🟢 可选（后期/特定套餐）。

---

## 一、统一 API 与协议兼容层 —— 🔴 归 token-station

路由器的入口门面，一切的地基。

| 功能点 | 等级 | 说明 | 参照 |
|--------|------|------|------|
| OpenAI 兼容端点（chat/completions + stream） | 🔴 | 所有客户端的最大公约数 | 全部产品 |
| Anthropic Messages 原生格式端点 | 🔴 | 让 Claude Code 直接接入。**2026-07-08 拍板升 🔴**：Z.AI 官方为 GLM-5.2 专设 Anthropic 兼容端点（`api.z.ai/api/anthropic`）零改造接住 Claude Code 流量——需求强度已被官方验证；Claude Code 是当前最大的编码代理流量入口，不接住等于放弃最肥的客户端生态 | new-api、OmniRoute 协议互译 |
| Gemini 原生格式端点 | 🟡 | 让 Gemini SDK 直接接入（需求强度未获同等验证，维持差异化档） | new-api、OmniRoute 协议互译 |
| 跨格式互译（OpenAI ⇄ Claude ⇄ Gemini） | 🟡 | 一套上游、多种客户端协议 | new-api、OmniRoute |
| Embeddings / Rerank / 多模态（图像/音频/视频）端点 | 🟢 | 扩品类，非路由核心 | new-api、Portkey multimodality |
| `/v1/models` 模型目录发现 | 🔴 | 客户端拉可用模型列表 | LiteLLM、OpenRouter |
| 模型名后缀控推理强度（`-thinking`/`-high`） | 🟡 | 自营 GLM 可控 thinking 开关，纯转售做不了 | new-api、OpenRouter `:nitro`/`:floor` |
| SDK（至少 Python/JS）+ OpenAI SDK 直接指向 | 🔴 | 零迁移成本接入 | 全部 |
| MCP / 工具调用 passthrough | 🟢 | 转发上游工具能力 | Portkey MCP |

> **不做**：不自建私有 API 协议，一切以 OpenAI 兼容为默认，其余为增值。

---

## 二、模型与供应商聚合 + BYOK —— 🔴 归 token-station

| 功能点 | 等级 | 说明 | 参照 |
|--------|------|------|------|
| 多供应商渠道接入（官方 API + 自建端点 + glm5.2-platform） | 🔴 | glm5.2-platform 只是众多路由目标之一 | new-api 渠道、LiteLLM |
| 模型统一目录（价格/上下文/吞吐/精度对比） | 🔴 | 用户选型依据 | OpenRouter 模型库 |
| 模型语义别名（fast/smart/cheap） | 🟡 | 客户端传别名、平台侧按套餐解析档位 | plano、见 [全景借鉴](../research/开源token路由系统全景借鉴.md) |
| **BYOK：自带 provider key 直连** | 🔴 | 分两种口径：**服务端 BYOK** = 用户主动把 key 托管给 SaaS/私有部署网关代理直连；**客户端 BYOK** = key 只存在本地客户端、永不上传云端 | OpenRouter BYOK、OmniRoute |
| BYOK 三态 fallback 开关（always-use / fallback / 默认回退） | 🟡 | `always-use` = 不使用平台批发凭证；在客户端/私有部署语境兑现 L2，在 SaaS 服务端 BYOK 语境只代表上游账号与计费归属，不代表平台不可见内容 | OpenRouter |
| 渠道分组、优先级、权重、标签 | 🔴 | 供给侧路由的配置底座 | new-api、LiteLLM |
| 渠道测活 / 自动禁用 / 自动恢复 | 🔴 | 上游挂了自动摘除 | new-api、Bifrost 健康分 |

---

## 三、智能路由（质量侧）—— 🔴 归 token-station（详见专文）

**本域的方法学与四层框架已在 [智能路由能力.md](./智能路由能力.md) 完整覆盖**，此处只列功能点做索引，不重复：

| 功能点 | 等级 | 指向 |
|--------|------|------|
| 第 ① 层 用户自定义规则（声明式 + 语义匹配） | 🔴 | [智能路由能力 §一①](./智能路由能力.md) |
| **规则的服务端集中管理（Preset 形态）** | 🟡 | 本清单新增，见下方说明 |
| 第 ② 层 启发式复杂度打分 | 🔴 | 智能路由能力 §一② |
| 第 ③ 层 学习型分类器路由（含本地小模型分发） | 🟡 | 智能路由能力 §一③ |
| 第 ④ 层 级联兜底（仅非流式） | 🟢 | 智能路由能力 §一⑤ |
| Auto Router（一键智能路由端点） | 🟡 | OpenRouter `openrouter/auto` 形态 |
| 单阈值成本旋钮（0=全便宜…1=全SOTA，做成套餐参数） | 🟡 | RouteLLM 思想 |

> **路由规则的服务端集中管理（借 OpenRouter Presets）**——用户自定义规则不应散在客户端 header，而应是仪表盘里可创建/版本化的命名配置（`@preset/xxx`），客户端一个别名引用整套「模型+参数+provider偏好+路由策略」，改配置不改代码。这是第 ① 层「用户自定义规则」的最佳交付形态，也是套餐差异化载体，同时兑现路由透明的「可覆写兜底」。已写入 [智能路由能力.md 第 ① 层「交付形态」](./智能路由能力.md#-静态规则路由用户自定义规则的载体)。

---

## 四、供给侧路由 / 负载均衡 / 容灾 —— 🔴 归 token-station

质量侧选定模型后，「该模型由哪个副本/供应商/key 来服务」。已有 [OmniRoute 分析](../research/omniroute-开源系统借鉴分析.md) 深挖，此处收口为功能清单：

| 功能点 | 等级 | 说明 | 参照 |
|--------|------|------|------|
| provider 偏好排序（order / only / ignore） | 🔴 | 用户对供给侧的直接控制 | OpenRouter |
| 负载均衡（默认价格平方倒数加权 / 多 key 分摊） | 🔴 | 无配置时的默认派单 | OpenRouter、LiteLLM |
| 按 price/throughput/latency 排序路由 | 🟡 | 用户显式选「贵但快/慢但便宜」 | OpenRouter sort、LiteLLM |
| `require_parameters` 能力过滤 | 🟡 | 只派给支持本请求全部参数的端点 | OpenRouter |
| `max_price` / `quantizations` / `data_collection` / `zdr` 路由约束 | 🟡 | **路由决策直接消费成本/精度/隐私约束**；`quantizations` 通用化自营「锁 fp8」溢出语义 | OpenRouter |
| 三层正交容灾（熔断/冷却/模型锁定） | 🔴 | 单一熔断粒度会误伤，见 OmniRoute 分析 §2.1 | OmniRoute、Bifrost |
| 语义化 fallback 分离（容量型 vs 能力型 vs 内容策略） | 🔴 | 失败类型不同、兜底链不同 | LiteLLM |
| 重试策略（按错误类型细分次数 + backoff） | 🔴 | | LiteLLM、new-api |
| 配额感知路由（fill-first 自建 / headroom / reset-aware） | 🟡 | 自建产能空闲即浪费 → 默认填满自建 | OmniRoute |
| Canary（新模型/新 provider 灰度上线） | 🟢 | 先给 5% 流量验证再全量 | Portkey |

> **与 glm5.2-platform 的边界**：glm5.2 的**内部产能溢出**（本地满 → 溢出到别的 GLM-5.2 供给、锁 fp8）是 glm5.2 **自己产品内部的事**，token-station 作为它的下游客户既不感知也不介入——那套 `overflow_eligible` 计费语义留在 glm5.2 的 [计费接入架构](../../../glm5.2-platform/docs/architecture/glm-5.2-计费接入架构.md)。token-station 眼里 glm5.2 就是一个会返回结果（或返回慢/失败）的普通上游，用通用的健康分/fallback（本节上面那些能力）处理即可，**不需要专门对接 glm5.2 的队列水位**。两个产品在此是「客户—供应商」松耦合，各自独立。见 [路由透明 §三](./路由透明与用户信任.md)。

---

## 五、Key / 认证 / 多租户 / 治理 —— 🔴 归 token-station（自有完整体系）

| 功能点 | 等级 | 归属 | 说明 | 参照 |
|--------|------|------|------|------|
| Virtual Key 发放 / 撤销 / 过期 / 轮换 | 🔴 | token-station | key 白名单模型、允许 IP、绑定 preset | LiteLLM、new-api |
| 组织 → 团队 → 用户 → key 层级 | 🔴 | token-station（结构）| Workspace 式租户隔离 | LiteLLM 七层、OpenRouter Workspaces |
| **每租户独立装 routing/preset/BYOK** | 🟡 | token-station | 兑现「三模式不共用一套路由器」约束 | OpenRouter Workspaces |
| RBAC（admin/member 角色） | 🔴 | token-station | | LiteLLM、OpenRouter |
| 团队额度公平切分（DRR + 会话粘性） | 🟡 | token-station | 小 Team 模式一份 token-station 订阅多人共享；扣减用 token-station 自己的额度体系（§六） | OmniRoute Quota-Share |
| **治理 guardrail：模型/provider 访问白黑名单、支出上限、数据策略强制** | 🔴 | token-station | 「谁能用什么、花多少、数据能否留」——按 key/租户强制 | OpenRouter Guardrails |
| SSO / OIDC / SAML | 🟢 | token-station | token-station 自有登录体系；企业模式接客户 IdP | LiteLLM Enterprise、new-api OIDC |
| 邀请/返利、实名认证 | 🟢 | token-station | token-station 自己的运营侧 | new-api |

---

## 六、计费 / 额度 / 充值 —— 🔴 token-station 自有一整套（可独立销售）

> **定位（原则 A）**：token-station 是能独立卖的路由器产品，**必须有自己完整的计费体系**，不依附 glm5.2-platform。下表全部功能都要在 token-station 里自建——glm5.2-platform 那边有它自己的一套同类功能，两套并行、不共用。参照对象是 OpenRouter（credits 体系）和 new-api（倍率/充值/兑换码运营侧），实现可直接抄 [层A 计费网关详细报告](../../../glm5.2-platform/docs/research/开源token路由调研-层A-计费网关详细报告.md) 里 LiteLLM/new-api 的设计（那份报告虽落在 glm5.2 项目下，但计费设计两个产品通用）。

| 功能点 | 等级 | 说明 |
|--------|------|------|
| **自有 credits / 钱包体系** | 🔴 | 用户在 token-station 直接充值、扣费，独立账本 |
| 逐请求成本 + 反事实价格回显（`X-Cost`/`X-Cost-Saved`/`X-Routed-Reason` 响应头） | 🔴 | 路由透明三件套之一，见 [路由透明](./路由透明与用户信任.md) |
| 七层预算 + budget_duration 周期重置 | 🔴 | 映射订阅（周期重置）+ PAYG（不重置），抄 LiteLLM |
| 倍率计费 / 按量 / 按次 / 缓存命中计费 | 🔴 | 抄 new-api 词汇表 |
| 充值 / 订阅 / 兑换码 / 发票 / Stripe / 易支付 | 🔴 | 独立运营侧，token-station 自己收钱 |
| soft_budget 告警 / 硬上限拒绝 | 🔴 | |
| 定价表外置（不写死代码） | 🔴 | token-station 自己维护一份（含各上游成本 + 自己的售价），算反事实价与结算都用它 |
| 单阈值成本旋钮做成套餐参数 | 🟡 | 路由策略即套餐差异化，见 §三 |

### 6.1 唯一交汇点：路由到 glm5.2-platform 时怎么结算

glm5.2-platform 是 token-station **众多上游供应商之一**（README 已定调）。当路由命中它，两种结算方式都支持，用户自选：

| 结算方式 | 机制 | 钱的流向 |
|---------|------|---------|
| **A. token-station 代采购转卖** | glm5.2-platform 把 token-station 当**普通下游客户**，发个 API key、按量对 token-station 结算；token-station 用**自己的 credits** 向终端用户收钱（可加价） | 用户 → token-station 钱包；token-station → glm5.2（两笔独立账） |
| **B. 用户 BYOK 自带 glm5.2 key** | 用户在 glm5.2-platform 自己充值拿 key；SaaS/私有部署网关可按**服务端 BYOK**托管代理，本地客户端则只在本机保存引用与密文 | 用户 → glm5.2（token-station 不代付算力）；token-station 可选收路由服务费或不收 |

- **两套计费系统在此只做「一方是另一方的客户/或完全不接触」的松耦合**，不共享账本、不互相扣减；
- 方式 B 正是 §二 的 BYOK `Always use this key` 旋钮的商业含义——上游账号与算力计费锁在用户自己的 glm5.2 账户内；若走 SaaS 服务端 BYOK，请求内容仍经过 token-station 平台，不能把它描述成 L2 零可见；
- 对其他上游（OpenAI/Anthropic 等）同理：要么 token-station 代采购转卖，要么用户 BYOK。glm5.2 在结算上**没有特殊地位**，就是一个可 BYOK 的普通上游。

---

## 七、内容 Guardrails / 安全 —— 🟡 归 token-station（优先复用开源）

与 §5 治理 guardrail 分开。

| 功能点 | 等级 | 说明 | 参照 |
|--------|------|------|------|
| PII 检测与脱敏 | 🟡 | 优先用 vLLM SR 内置，不自研 | Portkey、vLLM SR |
| 内容审查 / 越狱 / prompt 注入检测 | 🟡 | 同上 | Portkey、vLLM SR |
| Regex / JSON Schema 输出校验 | 🟡 | | Portkey |
| Guardrail Action：拦截 / 异步记录 / **fallback 切模型** | 🟡 | 审查不过 → 走内容策略 fallback 链，与路由层打通 | Portkey、LiteLLM `content_policy_fallbacks` |
| 第三方 guardrail 集成（Aporia/Pillar） | 🟢 | | Portkey |

---

## 八、可观测性 / 路由透明 —— 🔴 路由透明归 token-station，通用可观测收敛

> token-station **不做通用 LLM 可观测平台**（那是 Helicone/Portkey 的独立产品线），只做「路由这一环可验证 + 够用的用量账单」。

| 功能点 | 等级 | 归属 | 说明 | 参照 |
|--------|------|------|------|------|
| 路由决策可见（路由到谁/为什么/命中哪条规则） | 🔴 | token-station | 路由透明三件套 | [路由透明](./路由透明与用户信任.md) |
| 实际模型+provider+精度写入响应 header 并落库 | 🔴 | token-station | 与 glm5.2 模型真实性监控共用标注 | 路由透明 §三 |
| 分类器读了哪些特征（BYOK 证明内容未上云） | 🟡 | token-station | | 路由透明 §三 |
| 调用日志 / 用量统计 / 仪表盘 | 🔴 | token-station | 路由维度 + 自有账单维度，都在 token-station（§六自有计费） | new-api 数据看板 |
| Custom Metadata 标签 + Feedback 采集 | 🟡 | token-station | Feedback 是数据飞轮训分类器的采集入口 | Portkey |
| 导出 / OTel / Langfuse 回调 | 🟢 | 交界 | | LiteLLM |

---

## 九、缓存 / 限流 —— 🟡 归 token-station

| 功能点 | 等级 | 说明 | 参照 |
|--------|------|------|------|
| Prompt caching passthrough（透传上游缓存能力） | 🟡 | 不自建，转发上游（Anthropic 等）缓存 | OpenRouter |
| 精确缓存（相同请求命中） | 🟢 | | Portkey、Helicone |
| 语义缓存（近似请求命中） | 🟢 | vLLM SR 内置 | vLLM SR、OmniRoute |
| 限流（RPM/TPM，按 key/租户/模型） | 🔴 | 准入层粗粒度配额 | LiteLLM、Higress ai-quota |
| 幂等去重窗口 | 🟢 | | OmniRoute |

---

## 十、开发者体验 —— 🟡 归 token-station

| 功能点 | 等级 | 说明 | 参照 |
|--------|------|------|------|
| Playground（跨模型对话测试） | 🟡 | | Portkey、LibreChat |
| 文档 + 快速接入指引 | 🔴 | | 全部 |
| Webhook / 状态页 | 🟢 | | OpenRouter |
| 本地客户端（个人 BYOK 模式的壳，注册下载） | 🟡 | 本地规则引擎+内嵌小分类器+直连官方 API。**归属已拍板（2026-07-06）：由 token-station 开发交付**；glm5.2-platform 为被集成方之一（OAuth 设备码授权 + 只读 API），本地对账工具为其内置功能。**2026-07-08 修订**：仅平台注册后可下载（闭源分发，开源后置再议）；同日二次修订：无本地 Web 界面（界面包取消），指标库随本体默认开启；四次修订：当前范围为 BYOK / 本地模型 / 平台账户三类上游，闭源下需满足网络边界、字段白名单、本地审计与离线可用验收。完整需求见 [个人模式本地客户端需求](./个人模式本地客户端需求.md) | OmniRoute 形态 |
| Prompt 版本管理 / A-B / partials | 🟢 | LLMOps 范畴，非路由核心 | Portkey Prompt Studio |

---

## 十一、企业 / 部署 —— 🟡 归 token-station（独立交付）

> token-station 作为可独立销售的产品，企业能力和部署形态都自成一套，不依赖 glm5.2-platform 一起交付。

| 功能点 | 等级 | 归属 | 说明 |
|--------|------|------|------|
| SSO / 审计日志 / SLA | 🟢 | token-station | 自有登录/审计；企业客户接自己 IdP |
| 私有部署 / 数据驻留 / ZDR | 🟡 | token-station | 企业模式路由器整体部署在客户边界内，保证「路由在企业边界内」 |
| 单二进制 / Docker / DB 依赖 / 集群 | 🔴 | token-station | LiteLLM/new-api/Bifrost 形态 |
| 多机部署 + 状态共享（Redis/gossip） | 🔴 | token-station | LiteLLM Redis、Bifrost gossip |

---

## 十二、MVP 切分建议（🔴 项按上线顺序）

| 阶段 | token-station 自建能力 | 计费 / 结算 |
|------|-----------------------|-------------|
| **M1 门面** | OpenAI 兼容端点 + Anthropic Messages 端点（2026-07-08 升 🔴，接 Claude Code）+ `/v1/models` + SDK 指向 + 多渠道接入 + 服务端 BYOK 直连 | 自有定价表（各上游成本 + 自己售价） |
| **M2 路由** | 第 ①② 层路由 + 供给侧负载均衡 + 三层容灾 + 语义化 fallback + 逐请求成本/反事实价响应头 | glm5.2 及其他上游都当普通渠道，返回慢/失败走通用健康分与 fallback，不特殊对待 |
| **M3 计费+治理** | 自有 credits/订阅/充值/兑换码 + Virtual Key + 租户层级 + RBAC + 治理 guardrail + 限流 | **token-station 独立账本收钱**；路由到 glm5.2 时按 §6.1 的代采购 or BYOK 结算 |
| **M4 提质** | 规则服务端集中管理（Preset）+ 学习型路由 ③ + 路由决策可见/特征可查 + Feedback 采集 | 数据飞轮元数据（受 token-station 自己的「不用内容训练」条款约束）|

> M1–M2 覆盖 OpenRouter/LiteLLM 的核心价值面（统一 API + 精细路由 + 容灾），M3 补齐 **token-station 自有计费** + 多租户治理（这一步让它成为可独立销售的产品），M4 是差异化护城河（透明 + 学习型路由 + BYOK 隐私）。**token-station 全程不依赖 glm5.2-platform 交付**——glm5.2 只是它可路由的众多上游之一，两个产品各自独立演进、各自发展用户。

---

## 参考

- 闭源侧详细调研：[闭源商业token服务功能面调研](../research/闭源商业token服务功能面调研.md)
- 开源侧详细调研：[开源token路由系统全景借鉴](../research/开源token路由系统全景借鉴.md) · [OmniRoute 分析](../research/omniroute-开源系统借鉴分析.md) · [层A计费网关详细报告](../../../glm5.2-platform/docs/research/开源token路由调研-层A-计费网关详细报告.md)
- 路由能力与透明：[智能路由能力](./智能路由能力.md) · [路由透明与用户信任](./路由透明与用户信任.md)

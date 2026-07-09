# OmniRoute 开源系统分析：哪些能搬进 GLM-5.2 平台

> 对象：[diegosouzapw/OmniRoute](https://github.com/diegosouzapw/OmniRoute) —— TypeScript / MIT / 本地优先 LLM 网关，2026-02 创建，5 个月 11.4k star，聚合 237 家供应商，主打「个人开发者把免费/订阅额度榨干且永不断供」。
> 核心结论：**OmniRoute 与我们的智能路由不是同一个问题——它做的是「供给侧路由」（同一个模型请求派给哪个供应商/额度），我们四层路由做的是「质量侧路由」（这个 query 该用哪个模型）。两者正交、可叠加。最值得搬的五件事：三层正交容灾、配额感知路由策略族、Quota-Share 团队额度切分、9 因子在线评分 + 老虎机探索、成本透明响应头。它的本地优先单机形态还是个人模式开源客户端的现成架构蓝本。**
>
> 整理日期 2026-07-05。配套：[glm-5.2-智能路由能力.md](../features/glm-5.2-智能路由能力.md)（四层路由框架）、[glm-5.2-计费接入架构.md](../../../glm5.2-platform/docs/architecture/glm-5.2-计费接入架构.md)（准入/溢出/结算）、[glm-5.2-服务模式定位.md](../../../glm5.2-platform/docs/business/glm-5.2-服务模式定位.md)（三种模式）。

---

## 零、OmniRoute 是什么（一张表）

| 维度 | 事实 |
|------|------|
| 形态 | 本地 HTTP 代理网关（非库）：IDE/CLI → 本机 :20128 → 237 家供应商；Claude Code / Cursor / Cline 等 24+ 工具开箱接入 |
| 技术栈 | Node 22 + Next.js 16 仪表盘；**SQLite 单机存储，零外部依赖**（Redis/Qdrant 仅可选） |
| 路由 | 17 种策略（Combo 体系）+ `auto` 学习型评分路由（9 因子加权 + 多臂老虎机探索） |
| 容灾 | 熔断（供应商级）→ 冷却（账户/key 级）→ 模型锁定（供应商×账户×模型三元组）三层隔离；4 级 fallback 瀑布 |
| 计费 | 无真实收费，做**成本核算**：每 key 美元预算、`X-OmniRoute-Cost` 响应头、定价表同步 LiteLLM 数据源 |
| 兼容层 | OpenAI / Anthropic / Gemini / Ollama 协议互译；SSE + WebSocket；语义缓存 + 幂等去重窗口 |
| 社区 | 4.5k commits、100+ 贡献者但**强单人主导**；营销浓、迭代极快（5 天 3 个版本） |
| 灰色面 | JA3/JA4 TLS 指纹伪装绕地域封锁、免费层批量「借道」——**架构可学，商业模式勿学** |

与竞品的定位差：LiteLLM / Bifrost / Portkey 服务「企业把 LLM 流量管起来」，OmniRoute 服务「个人开发者永不断供」，相当于 OpenRouter 的自托管免费替代。

---

## 一、先厘清：它和我们的「智能路由」不是一个问题

[glm-5.2-智能路由能力.md](../features/glm-5.2-智能路由能力.md) 的四层路由回答的是 **「这个 query 该用哪个模型」**（质量/成本权衡，OpenFugu / RouteLLM 谱系）。

OmniRoute 的 17 种策略几乎全部回答另一个问题：**「模型已定，这次请求派给哪个供应商 / 哪个 key / 哪份额度」**（健康、配额余量、重置窗口、延迟、成本）。它没有 query 难度分类器。

**架构含义**：我们的网关里这是两个串联的路由阶段，应该分开建模——

```
请求 → [质量侧路由]  四层叠加，决定目标模型        ← 智能路由能力.md 已覆盖
     → [供给侧路由]  决定该模型由谁来服务：          ← OmniRoute 的主场，当前只有
         自建副本1/2 · OpenRouter(锁fp8) · 未来其他供应商      「队列水位溢出」一条规则
```

当前 [计费接入架构](../../../glm5.2-platform/docs/architecture/glm-5.2-计费接入架构.md) 的溢出逻辑（队列水位双阈值 + `overflow_eligible`）本质是供给侧路由的**最简版本**。OmniRoute 提供的是这一层长大后的完整形态。

---

## 二、五个直接可搬的设计

### 1. 三层正交容灾（替代单一熔断器）

OmniRoute 把故障隔离拆成三个互不误伤的粒度：

| 层 | 粒度 | 对应到我们 |
|----|------|-----------|
| 熔断器（开/半开/闭） | 供应商级 | OpenRouter 整体、未来第二溢出供应商、IDC 整体 |
| 连接冷却 | 账户/key 级 | 我们在 OpenRouter 上的多个账号/key；被限流的 key 跳过而非熔断整个供应商 |
| 模型锁定 | 供应商×账户×模型三元组 | 「OpenRouter 上某 provider 的 glm-5.2 fp8 挂了」不影响同账户其他模型 |

我们目前的容灾只有「IDC 满 → 溢出 OpenRouter」一跳。上多模型（ASR/TTS）和第二溢出供应商后，单一熔断粒度必然误伤——某 provider 的 fp8 挂掉就把整个 OpenRouter 拉黑是不可接受的。**建议在网关溢出模块里直接按这三层建模**，字段：`breaker(provider)`, `cooldown(provider, key)`, `lock(provider, key, model)`。

### 2. 配额感知路由策略族（headroom / reset-window / fill-first）

OmniRoute 把「额度剩余量」和「配额重置时间」当一等路由信号：`headroom`（剩余配额最多优先）、`reset-window / reset-aware`（快重置的先用掉）、`fill-first`（填满一个再用下一个）。

对我们的映射：**自建 GPU 是「预付费固定产能」，OpenRouter 是「按量现购」**——这正是「订阅额度 vs PAYG」的供给侧镜像。自建产能空闲一秒就是浪费一秒（成本模型里利用率是生死变量），所以供给侧默认策略应该是 **fill-first 自建、headroom 思路管理多副本水位**，而不是简单轮询。未来接多个外购供应商时，reset-aware 直接可用（各家限额窗口不同步）。

### 3. Quota-Share：DRR 团队额度切分 → 小 Team 模式的现成方案

OmniRoute 用 Deficit-Round-Robin 把**一份上游订阅额度**在团队多个 key 之间公平切分，按 `(key, model)` 建 5h/7d 多窗口桶，且带**会话粘性**保 prompt cache 命中。

这几乎是为我们的**个人租用（小 Team）模式**量身定做的（todo.md：「使用个人租用的服务器进行部署，方便个人多设备使用」）：一台租用服务器/一份企业订阅，多设备多成员公平分享额度，DRR 防止一人吃光，会话粘性保证同一会话的 KV cache 不被打散。企业模式的部门级额度分配同理。**结算引擎的 entitlement 扣减目前是账户级，需要下探到 key 级多窗口桶才能支持这个玩法。**

### 4. 9 因子在线评分 + 多臂老虎机：供给侧路由的落地形态

OmniRoute 的 `auto` 策略不训练模型，纯靠运行时遥测加权打分：健康 0.22、配额 0.17、成本⁻¹ 0.17、延迟⁻¹ 0.13、任务适配 0.08、历史成功率 0.05……并保留老虎机探索流量防止「一直用旧路径、新路径永远没数据」。

注意这**不是**智能路由文档里第 ③ 档学习型路由的替代——它不看 query 内容，没有隐私问题，**三种模式可以共用同一套**（不破 BYOK 的 L2 承诺）。定位：质量侧四层路由选出目标模型后，供给侧用这套评分在「自建副本 × 溢出供应商 × key」里派单。相比 RouteLLM 式离线训练，零冷启动、可解释、权重就是配置。

### 5. 成本透明响应头 + 定价表外置

OmniRoute 每个响应回 `X-OmniRoute-Cost` / `X-OmniRoute-Cost-Saved`，定价表直接同步 LiteLLM 的公开数据源。

这正好是智能路由文档「产品化细节 §1：省钱不可见就等于没省」的**具体机制**：逐请求在响应头回传 `X-Cost / X-Cost-Saved / X-Routed-Reason`，账单页聚合即可，无需额外埋点链路。定价表外置（而非写死代码）也让 rate_plan 扩 ASR/TTS 单价时不动网关。

---

## 三、一个战略级参考：本地优先形态 = 个人模式客户端蓝本

智能路由文档的关键约束是**个人 BYOK 模式的路由决策必须在本地完成**。OmniRoute 证明了这个形态工程上完全成立且被市场验证：

- 单命令安装（`npm i -g`）、SQLite 零依赖、本机代理端口、24+ 工具即插即用、本地仪表盘；
- 我们的开源个人客户端 = **同样的壳 + 我们的四层路由（本地规则引擎 + 内嵌小分类器）+ 直连官方 API / 自建端点**；
- 它的协议互译层（OpenAI↔Anthropic↔Gemini）和「无法自定义 header 的客户端走路径别名 `/vscode/KEY/...`」这类兼容性细节，都是踩过坑的现成答案，MIT 许可可直接研读甚至复用代码。

差异点要想清楚：OmniRoute 的存在理由是「白嫖聚合」，我们的个人模式存在理由是「隐私 + 自主」——壳可以像，灵魂不同，**不要跟随它做免费额度聚合**（见第四节）。

---

## 四、明确不借鉴的清单

| 项 | 原因 |
|----|------|
| TLS 指纹伪装（JA3/JA4）绕封锁 | 对上游 ToS 的对抗性规避，商业主体不可碰 |
| 免费层/订阅额度批量「借道」转售 | 供应商封禁风险 + 合规风险，且与我们「自建产能」模式相反 |
| 10 引擎提示压缩管线（宣称省 78–95% token） | 有损压缩动用户内容，与隐私承诺和质量口碑冲突；最多留作用户显式 opt-in 的实验特性 |
| SQLite 单机存储照搬到平台侧 | 它是单用户场景的最优解；我们平台侧已定 Redis + 明细库 + CKafka，不回退 |
| 依赖其项目本身 | 强单人主导、5 天 3 版本、README 与内部文档数字打架——**学设计，不引依赖** |

---

## 五、落点汇总（按现有文档归位）

| 借鉴点 | 应写入 / 修改的文档 | 动作 |
|--------|--------------------|------|
| 质量侧/供给侧两阶段路由的显式区分 | 智能路由能力.md | 在四层框架外补「供给侧路由」一节 |
| 三层正交容灾 | 计费接入架构.md §3 | 溢出链路从单跳升级为三层隔离模型 |
| fill-first / headroom / reset-aware 策略 | 计费接入架构.md §3 | 供给侧默认策略：fill-first 自建 |
| Quota-Share DRR + 会话粘性 | 服务模式定位.md（小 Team / 企业模式） | entitlement 下探 key 级多窗口桶 |
| 9 因子评分 + 老虎机 | 智能路由能力.md | 供给侧路由的实现方案，三模式共用 |
| X-Cost 响应头 + 定价表外置 | 计费接入架构.md §5 | 计量事件之外增加逐请求成本回显 |
| 本地优先客户端形态 | 服务模式定位.md（个人模式） | 开源客户端的工程参照物 |

---

## 参考

- [diegosouzapw/OmniRoute](https://github.com/diegosouzapw/OmniRoute) —— 本体，MIT
- [OmniRoute llm.txt 架构文档](https://github.com/diegosouzapw/OmniRoute/blob/main/llm.txt) —— 内部分层与目录说明
- [omniroute.online](https://omniroute.online/) —— 官网
- 同名但无关：[omnilabs-ai/OmniRouter](https://github.com/omnilabs-ai/OmniRouter)（停更小项目）、[arXiv:2502.20576](https://arxiv.org/abs/2502.20576)（学术论文，代码仓已 404）
- 对比参照：[LiteLLM](https://github.com/BerriAI/litellm) · [RouteLLM](https://github.com/lm-sys/routellm) · [ulab-uiuc/LLMRouter](https://github.com/ulab-uiuc/LLMRouter)

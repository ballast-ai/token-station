# LiteLLM 多租户计费网关调研

> 调研日期：2026-07-08
> 项目地址：https://github.com/BerriAI/litellm （文档 docs.litellm.ai）
> 许可证：主体 MIT，`enterprise/` 目录为商业许可（GitHub API 报 NOASSERTION 即因此双许可结构）
> 热度：约 52.9K stars（2026-07-08 GitHub API 实测），最新稳定版 v1.91.0（2026-07-04），当日仍有提交，迭代极活跃
> 定位：服务端多租户 AI 网关（Python SDK + Proxy Server），管「谁能调、扣谁的钱、满了怎么办」
> 借鉴关系：[全景借鉴总纲](./开源token路由系统全景借鉴.md) 已将其定为**层 A（计费网关）首选借鉴对象**，但明确不整体采用；本文为其独立详细报告，并补充与 [CC Switch](./cc-switch-桌面配置切换器竞品调研.md) 的层次对比

## 一、项目定位：一个项目、两个形态

1. **Python SDK**（`litellm` 包）：统一调用接口，把 100+ 供应商（OpenAI、Anthropic、Bedrock、Vertex、Azure、Cohere、HuggingFace、vLLM……）适配成 OpenAI 格式，`litellm.completion()` 换 model 字符串即换供应商。这一形态是嵌入应用代码的库，与网关无关。
2. **Proxy Server（AI Gateway）**：主体形态。部署在服务端、依赖 Postgres（租户/key/账单数据）+ Redis（多实例限流与路由状态）的多租户网关，含 Admin UI。企业版叠加 SSO、审计日志、JWT 鉴权等。

一句话：**给平台团队用的服务端基础设施，站在所有请求的关键路径上**。它不碰任何本地工具配置——配置管理恰恰是它没有的东西。

## 二、虚拟 Key 与七层预算模型（核心可抄区）

### 虚拟 Key 体系

- 真实供应商凭证收敛在网关侧，租户拿到的是网关签发的虚拟 key（`sk-...`）；
- key 可挂 团队 / 用户 / 模型白名单 / 预算 / 限流 / 过期时间，支持按 key 独立计费。

### 七层预算

预算与限流（TPM/RPM）可以同时挂在多个层级，任一层超限即拒绝：

| 层级 | 说明 |
|------|------|
| 1. Proxy 全局 | 整个网关的总预算 |
| 2. 组织/团队（team） | 团队总额度 |
| 3. 团队成员（team member） | 成员在该团队内的额度 |
| 4. 内部用户（internal user） | 跨团队的个人额度 |
| 5. 终端客户（customer/end-user） | 透传 `user` 字段的外部终端用户额度 |
| 6. Key | 单个虚拟 key 的额度 |
| 7. 模型（per-key per-model） | 同一 key 下按模型细分的额度 |

- **`budget_duration` 自动重置**：预算可声明周期（如 `30d`、`1mo`），到期自动归零重计——这直接映射订阅制 plan 的「月度额度重置」语义，是[全景总纲](./开源token路由系统全景借鉴.md)点名要抄的第一项。
- 超额行为可配：硬拒绝或仅告警（webhook/Slack）。

## 三、路由与 Fallback

### 请求级负载均衡

同一个对外模型名后面可挂多个 deployment（不同供应商/区域/key），路由策略可选：simple-shuffle（默认，支持权重）、latency-based、usage-based（按 TPM/RPM 余量）、least-busy。多实例共享状态走 Redis。

### 三类语义化 Fallback（可抄的接口设计）

LiteLLM 把「失败」按语义分成三条独立的 fallback 链，而非一个笼统的重试列表：

| 类型 | 触发 | 语义 |
|------|------|------|
| `fallbacks` | 限流、超时、5xx | **容量型失败**：换个供应商再试同等模型 |
| `context_window_fallbacks` | 上下文超长 | **能力型失败**：换更大窗口的模型 |
| `content_policy_fallbacks` | 内容审核拒绝 | **策略型失败**：换审核策略不同的模型 |

启示已写入总纲：溢出/降级决策必须区分失败类型——容量型失败该重试同级供应商，能力型失败该升级模型，两者混在一条链里会做出错误决策。这与 token-station 质量侧路由是两套机制，互不替代。

## 四、计量与成本追踪

- 每笔请求按 token 实时算价，落 Postgres `spend_logs`，Admin UI 出账单视图，可按 key/user/team/model/tag 聚合；
- 可观测回调体系（success/failure callback）对接 Langfuse、Prometheus、Datadog、S3 等 20+ 后端；
- **`model_prices_and_context_window.json` 是社区事实标准**：LiteLLM 维护的这份全供应商价格 + 上下文窗口表被大量第三方项目（含 cc-switch 一类客户端的自定义定价参照）直接复用。自研计费的模型单价表可以直接以它为初始数据源，省去手工维护全行业价目。

## 五、架构与已知短板（为什么不整体采用）

- **计费在关键路径里**：预算检查、扣账都同步发生在请求路径上，Postgres/Redis 抖动直接放大为推理延迟。对照 Bifrost 的「计费不进关键路径」（内存预扣 + 异步落账），这是我们自研结算引擎（验证 Redis 预扣 + CKafka 异步落账）的直接反证材料。
- **Python 单体性能上限**：FastAPI 单体，社区与 Bifrost/Portkey 的基准测试多次指出其高并发下的延迟开销与内存占用显著高于 Go/Rust 网关；代码库以功能广度优先，issue 数长期 3.7K+。
- **双许可暗礁**：`enterprise/` 目录代码在主仓库内但非 MIT，整体 fork 需剥离；SSO、审计等多租户刚需在企业版付费墙内。
- 结论维持总纲判断：**抄它的预算模型与 fallback 语义，不抄它的实现**。

## 六、与 CC Switch 的层次对比

两者只有表层重叠（多上游 + 代理 + fallback + 用量统计），产品原点相反：cc-switch 的代理是配置切换器的附属能力，LiteLLM 的代理就是产品本身。

| 维度 | LiteLLM | CC Switch |
|------|---------|-----------|
| 形态 | 服务端网关（Docker/K8s，Postgres+Redis） | 桌面应用（Tauri，本地 SQLite，无服务端） |
| 服务对象 | 平台团队/企业，多租户 | 单个开发者本人 |
| 核心问题 | 计费、配额、多租户准入 | 多工具配置切换 |
| 路由粒度 | **请求级**：模型名后挂多 deployment，负载均衡 + 三类 fallback | **应用级**：一应用同一时刻一个活跃供应商，无请求级分流 |
| 计费 | 真计费：虚拟 key、七层预算、自动重置、落账 | 本地用量仪表板，只看不控 |
| 凭证模型 | key 收敛在网关，租户拿虚拟 key | 用户自己的 key 写回各工具配置 |
| 配置管理 | 无（不碰本地工具配置） | 核心能力（7 工具原生配置写回） |

**结论：两者互不构成竞品，分别是本产品两端的参照物**——LiteLLM 教服务端怎么做网关（对应 glm5.2-platform 结算引擎），cc-switch 教桌面端怎么做壳（对应个人模式本地客户端）。

## 七、对本项目的启示清单

抄：

1. 七层预算模型 → 映射四种 plan.type 的额度层级设计；
2. `budget_duration` 自动重置 → 订阅制月度额度语义；
3. 三类语义化 fallback 分离 → 内部溢出决策按失败类型分链；
4. `model_prices_and_context_window.json` → 自研计费价目表的初始数据源；
5. 超额双模式（硬拒绝 vs 仅告警）→ plan 到期宽限期设计。

不抄：

1. 计费同步进关键路径的架构（用 Bifrost 式异步落账对冲）；
2. Python 单体实现；
3. 整体 fork（双许可 + 企业版付费墙）。

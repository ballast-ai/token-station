# token-station 对标 cc-Switch 与 OpenRouter 的架构学习设计

> 日期：2026-07-17
> 状态：待评审
> 类型：架构对标与演进设计
> 参考对象：[cc-Switch](https://github.com/farion1231/cc-switch)、[OpenRouter](https://openrouter.ai/docs)

## 1. 结论

token-station 应分别学习两个参考项目解决不同边界问题的方法：

- 前半段 Agent 接入侧学习 cc-Switch，重点研究多 Agent 配置发现、导入、投影、原子写入、回填、冲突检测、回滚和恢复。
- 后半段模型供应侧学习 OpenRouter，重点研究模型目录、能力元数据、协议归一、Provider Endpoint 建模、参数兼容、供应级路由、fallback、成本和可观测性。
- token-station 继续保留自己的核心差异：本地部署、密钥本地保管、Canonical IR、WASM Adapter、本地可解释任务路由和内容零落盘。

cc-Switch 与 OpenRouter 都只是研究样本，不是 token-station 的依赖、组件、服务或运行时节点。本设计不要求：

- 调用、嵌入或启动 cc-Switch；
- 复用 cc-Switch 的数据库、配置文件或代码包；
- 调用 OpenRouter API；
- 把 OpenRouter 作为默认、备用或可选上游；
- 使用 OpenRouter 的 Auto Router、账户、Key、余额或托管 Provider；
- 让 token-station 的正确性依赖任一参考项目的可用性。

目标是把参考项目中经过验证的机制抽象出来，由 token-station 独立实现。

## 2. 背景

token-station 当前已经形成以下主链：

```text
Agent / IDE
  → 本地回环服务
  → Agent Adapter
  → Canonical IR
  → 四层本地路由
  → Provider Adapter
  → 模型厂商
```

现有能力包括：

- OpenAI Chat Completions、Anthropic Messages、OpenAI Responses 三种入站协议；
- Rules、Agent Hints、Heuristic、Default Pool 四层路由；
- 能力过滤、上游健康摘除和池内 fallback；
- OpenAI-compatible 出站 Adapter；
- OS Keychain、虚拟 Key、WASM 沙箱和出站外泄闸门；
- 本地模型目录同步、用量与延迟指标；
- Claude Code、Codex、OpenCode 桌面接入入口。

但 Agent 接入侧仍以每个客户端一个专用函数实现，模型供应侧仍主要依赖静态配置和单一 OpenAI-compatible 方言。继续按现有方式逐个增加 Agent 和供应商，会带来重复逻辑、能力误判和协议语义分叉。

因此需要先建立两个可独立演进的子系统：

1. Agent Connector 子系统：负责配置控制面。
2. Provider Runtime 子系统：负责模型供应面。

## 3. 设计目标与非目标

### 3.1 设计目标

1. 把 Agent 配置管理从桌面 App 的专用函数重构为可注册、可验证、可恢复的 Connector。
2. 把模型供应关系从“一个 upstream 配若干模型”扩展为 Provider、Endpoint、Model Deployment 和 Capability 四层模型。
3. 明确区分任务级模型选择与供应级 Endpoint 选择。
4. 保持协议转换、路由、密钥、配置管理和观测之间的职责隔离。
5. 为新增 Agent、原生 Provider Adapter、模型能力、价格和隐私策略提供稳定扩展点。
6. 所有关键操作可解释、可验证、可回滚。

### 3.2 非目标

1. 不实现 cc-Switch 的完整产品功能。
2. 不建设 MCP、Prompt、Skill、会话历史和云同步综合管理器。
3. 不建设 OpenRouter 式充值、账户、市场、托管 Key 或多租户平台。
4. 不在本阶段引入黑盒 LLM 热路径分类器。
5. 不在本阶段覆盖图片生成、音频生成、视频生成和 Embeddings 全协议。
6. 不通过任意 JSON 透传规避 Canonical IR 的语义评审。

## 4. 对 cc-Switch 的学习范围

### 4.1 cc-Switch 解决的问题

cc-Switch 面向 Claude Code、Codex、Gemini CLI、OpenCode、OpenClaw、Hermes 等工具，统一管理不同格式和不同生命周期的 Live 配置。

它的核心价值不是模型智能路由，而是配置控制面的完整性：

```text
内部 SSOT
  ↕ 导入 / 回填
Agent 专属 Live 配置
  ↕ 原子写入 / 快照 / 恢复
真实 Agent 进程
```

### 4.2 值得学习的机制

#### 4.2.1 Agent 类型注册

cc-Switch 用 `AppType` 统一描述不同应用，并区分两种配置行为：

- 切换模式：Claude、Codex、Gemini 只把当前配置投影到 Live 文件。
- 增量模式：OpenCode、OpenClaw、Hermes 允许多个 Provider 条目共存。

token-station 应学习这种“行为策略显式化”，不能继续把差异散落在多个 `connect_*` 函数中。

#### 4.2.2 SSOT 与 Live Projection

配置的真实状态由内部存储持有，Agent 的 JSON、TOML、`.env` 文件只是投影。这样可以独立处理：

- 用户保存的目标配置；
- 当前实际写入 Agent 的配置；
- token-station 拥有的字段；
- 用户或其他工具拥有的字段；
- 代理接管期间的恢复快照。

#### 4.2.3 首次导入

第一次管理某个 Agent 时，应先读取现有 Live 配置，并让用户确认导入、保留或覆盖。缺少导入步骤会让“一键接入”天然具有覆盖风险。

#### 4.2.4 切走前回填

用户可能在 token-station 之外修改 Agent 配置。切换、重连或断开前，应读取 Live 配置，把属于该配置档案的合法变化回填到内部状态，再投影新的目标配置。

回填必须基于字段所有权，不能把代理占位符、临时地址或内部派生字段保存为用户配置。

#### 4.2.5 原子写入和恢复快照

写入应采用“同目录临时文件 → flush → rename”的原子替换方式。涉及多个文件的 Agent，例如 Codex 的 `auth.json` 与 `config.toml`、Gemini 的 `.env` 与 `settings.json`，应先创建完整快照，任一写入失败时恢复整个集合。

#### 4.2.6 配置所有权与冲突检测

token-station 需要知道：

- 哪些字段由 Connector 写入；
- 写入前原值是什么；
- 写入后文件是否被外部修改；
- 断开时哪些字段可以安全删除或恢复；
- 当前 Live 配置是否已经被本地代理接管。

仅保留一个 `.bak` 文件无法回答这些问题。

#### 4.2.7 主流程与附属投影分级

Agent 主配置写入成功后，MCP 等附属投影失败不应把主操作描述成完全失败。应返回 `success + warnings`，并允许附属投影后续自愈。

### 4.3 不学习的部分

token-station 不照搬以下 cc-Switch 产品边界：

- 每次切换模型或供应商都重写 Agent 配置；
- 让 Agent 直接持有不同厂商的上游地址和 Key；
- MCP、Prompt、Skill、会话和云同步综合管理；
- 供应商推广、订阅和账户切换；
- 与 token-station 本地 Router 重叠的代理和路由实现。

对 token-station 而言，Agent 完成一次安全接入后应长期指向本地网关，模型选择和 Provider 调度发生在网关内部。

## 5. 对 OpenRouter 的学习范围

### 5.1 OpenRouter 解决的问题

OpenRouter 把大量模型、Provider 和实际推理 Endpoint 组织在统一 API 后面。它主要解决供应侧异构问题：

```text
统一请求
  → 模型标识与能力目录
  → 参数兼容过滤
  → 同模型多个 Endpoint 排序
  → 供应级 fallback
  → 厂商协议转换
  → 统一响应、用量和成本
```

### 5.2 值得学习的机制

#### 5.2.1 动态模型目录

模型目录不应只有 ID，还应包含：

- canonical model ID 与 alias；
- 上下文窗口；
- 输入、输出模态；
- 支持的请求参数；
- 工具调用和结构化输出能力；
- reasoning 能力；
- 生命周期与弃用时间；
- 价格和条件价格；
- 可用 Endpoint 列表。

OpenRouter Models API 将这些信息标准化，并允许按模态、参数、价格、上下文、吞吐和延迟查询。token-station 应学习其元数据完整性，而不是使用其 API。

#### 5.2.2 Model 与 Endpoint 分离

同一个逻辑模型可能存在多个实际供应 Endpoint，例如厂商直连、区域云平台和第三方推理集群。模型能力与 Endpoint 运行属性不能混在同一个 `upstream` 对象中。

建议关系：

```text
Provider
  └── Endpoint
        └── ModelDeployment
              └── ModelCapability
```

其中：

- Provider 表示供应组织或协议家族；
- Endpoint 表示可请求地址、鉴权和区域；
- ModelDeployment 表示该 Endpoint 上部署的具体模型；
- ModelCapability 表示对 Router 有意义的稳定能力。

#### 5.2.3 参数能力过滤

不同模型和 Endpoint 支持的参数不同。请求携带 `tools`、`tool_choice`、`response_format`、reasoning 或特定采样参数时，供应级路由应只选择满足要求的 Deployment。

未知参数默认静默丢弃会导致行为不可解释。token-station 应支持三种显式策略：

1. `required`：不支持则该候选不可用。
2. `best_effort`：允许 Adapter 丢弃，但必须记录降级。
3. `forbidden`：该参数不允许进入此 Provider 方言。

#### 5.2.4 Provider Endpoint 路由

OpenRouter 会依据健康、价格、吞吐和延迟选择同一模型的不同 Endpoint。token-station 应学习这种二级路由，但保持本地、确定和可解释。

任务级路由回答“这类任务应使用哪个模型”；Endpoint 路由回答“这个模型本次应由哪个部署提供”。二者不能合并为一个评分器。

#### 5.2.5 两层 fallback

需要区分：

- Endpoint fallback：模型不变，切换同模型的其他 Deployment。
- Model fallback：模型发生变化，只能在运营者显式允许的等价或降级链中发生。

Endpoint 的 429、超时、容量不足和 5xx 可以触发 Endpoint fallback。鉴权、请求格式、能力不足和内容策略错误默认不能跨候选重试。

#### 5.2.6 统一观测

一次请求至少需要记录：

- 请求模型与实际模型；
- 选中的 Provider、Endpoint、Deployment；
- 尝试顺序和每次失败分类；
- 输入、输出、缓存和 reasoning token；
- 估算成本与上游报告成本；
- 首 token 延迟、总延迟和吞吐；
- 是否发生参数降级和 fallback。

记录仍不得包含 prompt、response、工具参数或任意可承载内容的自由文本。

#### 5.2.7 隐私和策略约束

Endpoint 元数据应能表达：

- 是否允许数据留存；
- 是否满足零数据保留；
- 区域；
- 是否允许训练或蒸馏；
- 企业策略标签。

这些字段属于候选过滤条件，不应只作为 UI 说明。

### 5.3 不学习的部分

token-station 不照搬以下 OpenRouter 产品边界：

- 云端集中处理用户 prompt；
- 充值、余额、平台 Key、托管 BYOK 和 Workspace 账户系统；
- 模型市场和供应商商业聚合；
- 把云端 Auto Router 作为核心模型选择器；
- 依赖全局黑盒统计做不可重放的路由；
- 默认允许平台替用户隐式改变模型。

## 6. token-station 目标架构

目标运行时只包含 token-station 自己实现的组件：

```text
┌───────────────────────────────────────────────────────────────┐
│                    Agent 配置控制面                            │
│  Agent Registry → Connector → Plan/Diff → Apply → Verify       │
│                                  ↘ Snapshot / Restore           │
└──────────────────────────────┬────────────────────────────────┘
                               │ Agent 长期指向本地网关
                               ▼
┌───────────────────────────────────────────────────────────────┐
│                    入站协议层                                  │
│  OpenAI Chat │ Anthropic Messages │ OpenAI Responses │ ...     │
│                        Agent Adapter                            │
└──────────────────────────────┬────────────────────────────────┘
                               ▼
                      Canonical Request IR
                               ▼
┌───────────────────────────────────────────────────────────────┐
│                    任务级模型路由                              │
│  显式模型 → Rules → Agent Hints → Classifier/Heuristic → 默认  │
│  输出：Logical Model + 解释 + 允许的 Model Fallback Chain      │
└──────────────────────────────┬────────────────────────────────┘
                               ▼
┌───────────────────────────────────────────────────────────────┐
│                    供应级 Endpoint 路由                        │
│  能力/参数/隐私过滤 → 健康 → 价格/延迟/吞吐策略 → fallback      │
│  输出：Provider + Endpoint + ModelDeployment                   │
└──────────────────────────────┬────────────────────────────────┘
                               ▼
┌───────────────────────────────────────────────────────────────┐
│                    出站协议层                                  │
│       Provider Adapter → 鉴权闸门 → HTTP/SSE → 统一响应         │
└───────────────────────────────────────────────────────────────┘
```

### 6.1 Agent Registry

维护支持的 Agent 类型及其稳定元数据：

- 唯一 ID、显示名和支持平台；
- 安装检测方式；
- 配置路径集合；
- 配置模式：exclusive 或 additive；
- 入站协议；
- Base URL 和 Key 的配置方式；
- 生效方式：热加载、重启 Agent 或重启终端；
- 对应 Connector 实现。

### 6.2 Agent Connector

每个 Connector 只负责一个 Agent 的配置控制面，统一暴露：

```text
detect()       → 是否安装、配置路径、版本信息
inspect()      → 当前 Live 配置与接入状态
plan(target)   → 结构化变更计划和风险提示
apply(plan)    → 创建快照并原子写入
verify()       → 从磁盘重读并验证实际状态
disconnect()   → 恢复拥有字段或完整快照
repair()       → 处理部分写入、外部修改和失效代理地址
```

Connector 不参与请求协议翻译，也不选择模型和 Provider。

### 6.3 Agent Adapter

Agent Adapter 继续承担：

- 匹配入站路径和协议；
- 将请求归一为 Canonical IR；
- 提取可信 Agent Hint；
- 将统一响应、流事件和错误渲染回 Agent 方言。

Connector 与 Adapter 的对应关系由 Agent Registry 声明，但两者不能互相调用。

### 6.4 Model Catalog

Catalog 保存逻辑模型和 Deployment 元数据。目录来源可以是：

- Provider 官方目录；
- Adapter 内置静态目录；
- 用户手工声明；
- 经验证的本地缓存。

来源需要带优先级、获取时间和可信度。手工覆盖必须显式，不能被下一次自动刷新静默覆盖。

### 6.5 Provider Registry

Provider Registry 管理：

- Provider 方言与 Adapter；
- Endpoint、鉴权引用和区域；
- ModelDeployment；
- Capability 与支持参数；
- 价格和隐私策略；
- 当前健康与性能统计。

### 6.6 Endpoint Router

Endpoint Router 是纯策略模块，不做网络请求。输入为：

- 已选逻辑模型；
- 请求需要的能力和参数；
- 运营者策略；
- Endpoint 健康与性能快照。

输出为有序 Deployment 列表及解释。Gateway 按列表执行并反馈结果，Health Tracker 更新运行状态。

## 7. 核心数据流

### 7.1 Agent 首次接入

```text
用户选择 Agent
  → detect 安装与配置路径
  → inspect 当前配置
  → 导入或确认所有权
  → plan 生成字段级 diff
  → 创建多文件快照
  → 原子 apply
  → 从磁盘重读 verify
  → 进行最小协议探测
  → 标记 connected
```

任何一步失败都不得把状态标记为 connected。

### 7.2 请求处理

```text
Agent 请求
  → 入站 Adapter 归一
  → 提取 RequestRequirements
  → 任务路由选择 Logical Model
  → Catalog 找到 Deployment 候选
  → Endpoint Router 排序
  → Provider Adapter 构建请求
  → 外泄闸门验证 URL 和凭证引用
  → 执行、fallback、统一响应
  → 写入无内容指标
```

### 7.3 模型目录刷新

```text
读取目录来源
  → 校验响应大小和结构
  → 解析模型、能力、参数、价格
  → 与缓存和手工覆盖合并
  → 标记新增、变化、弃用和冲突
  → 用户确认破坏性变化
  → 原子更新 Catalog
```

目录刷新失败时保留上一个已验证版本，不把空目录视为成功。

### 7.4 fallback

```text
Deployment A 失败
  → Provider Adapter 分类错误
  → Retry Policy 判断是否允许切换 Endpoint
  → 允许：尝试同模型 Deployment B
  → 同模型耗尽后，仅按显式 Model Fallback Chain 决定是否换模型
  → 首字节已经发送后禁止透明换模型
```

## 8. 错误处理与安全约束

### 8.1 Agent 配置错误

- 非法 JSON/TOML/`.env`：拒绝写入，不创建伪成功状态。
- 配置文件不存在：根据 Agent 规则决定创建或提示用户先初始化。
- 外部修改冲突：展示 diff，禁止静默覆盖。
- 多文件部分写入：恢复完整快照。
- 无法恢复：进入 `repair_required`，保留诊断和人工恢复路径。

### 8.2 Provider 错误

- Auth：不重试其他 Key 或 Provider，除非运营者显式配置同账户 Key 轮换。
- Invalid Request：不重试。
- Capability：不重试同一不兼容 Deployment。
- Content Policy：默认不跨模型规避。
- Rate Limit、Capacity、Timeout、5xx：允许按策略切换 Endpoint。
- 流中错误：记录实际失败，不能仅保留已提交的 HTTP 200。

### 8.3 安全约束

- Agent Connector 不读取上游 Provider Key。
- Agent Adapter 不读取任何明文 Key。
- Provider Adapter 只能声明凭证引用，不能获取明文。
- Endpoint URL 在解析凭证前通过外泄闸门。
- Catalog 响应、错误体和 Live 配置不能进入请求日志。
- 配置快照可能包含本地虚拟 Key，必须限制权限并支持加密或字段脱敏。

## 9. 当前实现差距

### 9.1 Agent 接入侧

当前桌面端的 Claude Code、Codex、OpenCode 接入分别由专用函数实现：

- [`connect_cc_at`](../../../apps/desktop/src-tauri/src/lib.rs)
- [`connect_codex_at`](../../../apps/desktop/src-tauri/src/lib.rs)
- [`connect_opencode_at`](../../../apps/desktop/src-tauri/src/lib.rs)

主要差距：

1. 缺少统一 Agent Registry 和 Connector 接口。
2. 缺少标准 `detect/inspect/plan/apply/verify/disconnect/repair` 生命周期。
3. 只有单一备份路径，没有多文件事务快照和轮转。
4. 缺少字段所有权和外部修改检测。
5. 缺少切走前 backfill。
6. 缺少正式断开和自动恢复入口。
7. 缺少 Gemini 等 Agent 的注册式扩展。
8. 文件写入成功后缺少完整 Agent 侧协议验证。

### 9.2 模型供应侧

当前模型发现只解析 `/models` 返回的 `data[].id`，新增模型默认声明 `tool=true`、`context_window=128000`：

- [`model_catalog.rs`](../../../apps/desktop/src-tauri/src/model_catalog.rs)
- [`replace_provider_models`](../../../apps/desktop/src-tauri/src/lib.rs)

主要差距：

1. 缺少模态、能力、参数、价格和生命周期元数据。
2. 缺少 Provider、Endpoint、Deployment 分层。
3. 能力默认值可能造成错误放行和错误拒绝。
4. 出站 Adapter 只覆盖 OpenAI-compatible Chat Completions 子集。
5. `tool_choice`、reasoning、structured outputs 等语义未形成完整 IR 闭环。
6. 缺少供应级价格、延迟、吞吐和隐私过滤。
7. 指标缺少实际 Endpoint、fallback 尝试和成本来源。

### 9.3 路由语义

当前产品文档声称具体模型请求应直达该模型，但 `Router::route` 仍统一进入选池逻辑。设计实施前必须先修复显式模型优先级，否则后续 Model Catalog 和 Endpoint Router 都建立在不稳定语义上。

## 10. 分阶段路线

### 阶段 A：Agent Connector 基座

交付：

- Agent Registry；
- Connector trait；
- Claude Code、Codex、OpenCode 迁移；
- inspect、plan、apply、verify、disconnect；
- 多文件快照、原子写入、恢复；
- 字段所有权和冲突检测；
- Connector conformance 测试。

阶段完成标准：三个现有 Agent 不再由桌面 App 专用分支直接修改配置。

### 阶段 B：Canonical Capability 与 Catalog

交付：

- 扩展 ModelCapability；
- RequestRequirements；
- Catalog 来源、缓存、可信度和覆盖规则；
- 解析上下文、模态、支持参数和价格；
- 修复具体模型直达；
- 能力变化 diff 和弃用提示。

阶段完成标准：不再给所有新模型统一猜测 `tool=true` 和 128K 上下文。

### 阶段 C：Provider、Endpoint、Deployment

交付：

- 新供应侧数据模型；
- Provider Registry；
- Endpoint Router；
- Endpoint 健康、排序和同模型 fallback；
- 现有 upstream 配置迁移；
- 路由解释和审计记录。

阶段完成标准：同一个逻辑模型可以安全绑定多个实际 Deployment。

### 阶段 D：协议语义扩展

交付：

- `tool_choice`；
- structured outputs；
- reasoning；
- 参数支持策略；
- 原生 Provider Adapter 扩展机制；
- 协议 conformance 与真实 E2E。

阶段完成标准：Router 宣称的能力与实际出站请求、响应语义一致。

### 阶段 E：成本、性能与隐私策略

交付：

- 价格表和条件价格；
- 首 token 延迟与吞吐；
- 成本估算和上游成本对账；
- 区域、留存和企业策略过滤；
- Endpoint 策略配置与可视化解释。

阶段完成标准：供应级路由可以按明确策略优化成本、性能或隐私，并能重放解释。

## 11. 验收原则

### 11.1 Agent Connector

1. 非法现有配置绝不被覆盖。
2. 接入前后无关字段保持不变。
3. 多文件操作任一失败可恢复原状。
4. 重复接入幂等。
5. 外部修改能被识别并阻止静默覆盖。
6. 断开后恢复接入前状态或只删除 token-station 拥有字段。
7. 验证失败时不显示 connected。

### 11.2 Model Catalog

1. 目录刷新失败保留旧目录。
2. 能力来源和更新时间可见。
3. 手工覆盖不会被静默覆盖。
4. 不支持工具的模型不能接收工具请求。
5. 上下文窗口按实际元数据过滤。
6. 目录变化可生成稳定 diff。

### 11.3 Endpoint Router

1. 同一输入和状态快照产生相同排序。
2. 不满足能力、参数和隐私约束的候选在排序前剔除。
3. 只有可重试错误触发 fallback。
4. 首字节后不透明换模型。
5. 路由记录不包含内容。
6. 实际 Endpoint、模型、尝试次数和失败原因可审计。

## 12. 设计决策

### 决策 1：参考项目不进入运行时

cc-Switch 与 OpenRouter 只用于架构学习。所有目标能力由 token-station 自研，并接受自己的测试、隐私和兼容性约束。

### 决策 2：Connector 与 Adapter 分离

Connector 管理 Agent 配置文件；Adapter 处理请求协议。两者共享注册元数据，但不共享实现职责。

### 决策 3：任务路由与 Endpoint 路由分离

任务路由选择逻辑模型，Endpoint 路由选择实际部署。模型质量决策不能被价格或短期延迟隐式覆盖。

### 决策 4：Capability 是强约束

能力未知默认视为不支持。允许运营者显式覆盖，但必须记录来源，不能使用无证据的宽松默认值。

### 决策 5：配置和请求都必须可恢复、可解释

Agent 配置变更必须可预览和恢复；模型请求路由必须可解释和审计。两条链路均禁止“静默做了大概正确的事”。

## 13. 参考资料

### cc-Switch

- [项目仓库](https://github.com/farion1231/cc-switch)
- [AppType 与切换/增量模式](https://github.com/farion1231/cc-switch/blob/main/src-tauri/src/app_config.rs)
- [Provider 切换、回填与同步](https://github.com/farion1231/cc-switch/blob/main/src-tauri/src/services/provider/mod.rs)
- [Live 配置投影和快照恢复](https://github.com/farion1231/cc-switch/blob/main/src-tauri/src/services/provider/live.rs)
- [原子文件写入](https://github.com/farion1231/cc-switch/blob/main/src-tauri/src/config.rs)

### OpenRouter

- [Models](https://openrouter.ai/docs/guides/overview/models)
- [API Overview](https://openrouter.ai/docs/api/reference/overview)
- [Provider Routing](https://openrouter.ai/docs/guides/routing/provider-selection)
- [Model Fallbacks](https://openrouter.ai/docs/guides/routing/model-fallbacks)
- [Structured Outputs](https://openrouter.ai/docs/guides/features/structured-outputs)
- [Data Collection](https://openrouter.ai/docs/guides/privacy/data-collection)
- [Provider Logging](https://openrouter.ai/docs/guides/privacy/provider-logging)

### token-station

- [架构总览](../../contributing/架构总览.md)
- [桌面 App Agent 接入机制](../../contributing/桌面App-Agent接入机制.md)
- [入站适配器协议盘点](../../contributing/入站适配器-协议盘点.md)
- [路由机制](../../product/路由机制.md)
- [配置详解](../../product/配置详解.md)

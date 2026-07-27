# 免费模型供应商接入与统一目录设计

- 日期：2026-07-27
- 状态：方案 A 已确认并实现；前端构建、自动化测试与实际页面视觉检查通过
- 范围：`apps/desktop` 统一供应商目录、免费供应商独立配置流程，以及支撑免费/付费隔离所需的 CLI 配置与 Tauri 命令
- 交互参考：本地 `/Users/liuwenhao/Desktop/omniroute/` 的可搜索供应商卡片目录；仅参考信息组织和候选元数据，不复制其全部供应商、代码或运行时

## 1. 结论

Token Station 在同一个“添加供应商”目录中提供常规 API 与免费 API 两种模式：

```text
添加供应商
  → 常规 API / 免费 API
  → 共用搜索、筛选、供应商卡片和返回语义
  → 免费供应商配置
  → 验证 API Key 与一个真实免费模型
  → 创建独立免费供应商实例
  → 返回主页，由用户手动配置三档路由
```

目录交互统一，但免费实例仍不是普通供应商上的视觉筛选，而是独立配置实体：

- 独立上游 ID，例如 `nvidia_free`；
- 独立系统钥匙串凭据；
- 只包含目录中已核验的免费模型；
- 显式持久化 `access_tier: free`；
- 不与同平台的付费实例合并、覆盖或共享模型；
- 免费额度耗尽时不得自动切换到付费模型。

首版目录采用随应用发布的人工审核数据，不远程抓取第三方目录。免费政策发生变化时，通过更新目录和重新发布应用修正。

## 2. 背景与当前事实

### 2.1 当前添加流程

`apps/desktop/src/pages/AddProviderPage.tsx` 当前完成：

1. 从 `apps/desktop/src/catalog.ts` 选择普通供应商预设；
2. 填写或读取 Base URL、API Key；
3. 通过 `/models` 发现模型；
4. 将供应商加入主页与 Agent 共用目录。

统一目录补充“长期免费 / 试用额度”、地区、费用保护和核验日期等信息；普通与免费模式各自保留搜索和筛选状态。

### 2.2 当前配置不能区分免费和付费

`apps/cli/src/config.rs` 的 `UpstreamConfig` 使用 `#[serde(deny_unknown_fields)]`，现有字段只有：

- `provider`
- `base_url`
- `auth`
- `local`
- `models`

因此不能只在桌面端临时增加未知字段。若要稳定区分免费和付费，必须在配置类型中增加显式字段并补齐读写、快照与测试。

### 2.3 当前新增与能力声明不适合直接复用

`apps/desktop/src-tauri/src/lib.rs::add_provider` 当前：

- 拒绝同名供应商；
- 只接收模型字符串；
- 默认把所有模型声明为支持工具调用和 JSON Schema；
- 没有免费目录 allowlist；
- 没有真实生成请求前置验证。

免费模型的工具、JSON Schema、视觉和上下文能力并不一致，不能继续统一声明。免费流程需要独立命令和权威目录。

## 3. 目标

1. 在“添加供应商”标题右侧提供清晰的免费模型入口。
2. 提供中国用户优先、兼顾国际平台的免费供应商目录。
3. 支持按名称、别名、模型、说明和标签实时搜索。
4. 支持按免费类型和地区组合筛选。
5. 用一句话说明如何申请 API Key，并使用系统浏览器打开官方入口。
6. 在写入任何配置或钥匙串前完成真实模型验证。
7. 免费与付费实例能够在同一平台并存，互不覆盖。
8. 只把已核验的免费模型放入免费实例。
9. 为免费模型记录真实能力，避免 Agent 请求被错误路由。
10. 添加成功后不自动修改主页或 Agent 路由。

## 4. 非目标与红线

### 4.1 非目标

- 不实现在线远程供应商市场。
- 不自动抓取 OmniRoute 或其他第三方项目的目录。
- 不承诺供应商免费政策永久不变。
- 不自动把免费供应商放入上档、中档或下档。
- 不实现余额抓取、额度计数或跨供应商自动轮换。
- 不在首版支持 OAuth、网页 Cookie、免鉴权或自托管免费来源。
- 不把 NVIDIA 本地 NIM 或 NVIDIA AI Enterprise 试用混入托管 API Catalog 流程。

### 4.2 安全红线

- API Key 不进入 URL、日志、配置文件、前端持久化存储或模型缓存。
- 验证失败时不写配置和钥匙串。
- 离开配置页后清空前端内存中的 Key。
- 只有验证成功后才将 Key 写入系统钥匙串。
- 前端不能提交任意 Base URL、任意模型能力或任意“免费”标记绕过后端目录。
- 免费实例不能自动回退到同平台的付费模型。
- 对额度耗尽后可能继续计费的平台，必须要求用户先启用官方费用保护；无法形成可操作保护条件时不能标为“长期免费”。

## 5. 信息架构与页面流程

### 5.1 页面集合

在现有 `AppView` 中增加两个视图：

```text
free-provider-catalog
free-provider:<preset_id>
```

完整流程：

```text
add-provider
  └─ 浏览免费模型
       └─ free-provider-catalog
            └─ free-provider:<preset_id>
                 ├─ 返回目录：保留搜索与筛选状态
                 └─ 验证并添加成功：返回 home
```

导航历史继续复用 `viewHistoryRef`。免费配置成功后直接回主页，不返回普通添加页。

### 5.2 添加供应商页入口

标题区改为左右布局：

```text
← 返回
NEW UPSTREAM
添加供应商                               [浏览免费模型 →]
接入新的模型服务……
```

入口使用次级按钮，不放入供应商单选卡片网格。普通预设卡片表示“立即选择当前表单项”，免费入口表示“进入独立目录”，两种交互不能混用。

### 5.3 免费供应商目录

页面顶部：

```text
← 返回
FREE SIGNALS
免费模型供应商
选择可申请 API Key 的免费模型服务。免费政策以供应商实时规则为准。

[搜索供应商或免费模型……]
[全部] [长期免费] [试用额度]       [全部地区] [中国可用] [全球平台]
```

卡片在宽屏、中等窗口、窄屏分别使用 3、2、1 列。

每张卡片只展示：

- 图标、供应商名称；
- 一句话免费额度说明；
- `长期免费`或`试用额度`；
- `中国可用`、`全球平台`、`无需绑卡`、`需实名认证`等关键标签；
- 已核验免费模型数量；
- `已添加`状态。

搜索匹配：

- 名称与别名；
- 免费模型 ID 与展示名；
- 免费说明；
- 地区与限制标签。

免费类型与地区筛选和搜索条件相与。无结果时说明当前条件，并提供“清除筛选”。

### 5.4 免费供应商配置页

页面包含：

1. 供应商身份、免费类型和核验日期；
2. 一句话 API Key 申请说明；
3. `申请免费 API Key`外部浏览器按钮；
4. API Key 密码输入框；
5. 免费模型列表，默认全部选中；
6. 每个模型的工具、JSON、视觉与上下文能力；
7. 免费额度和费用保护提示；
8. `验证并添加`主按钮。

以 NVIDIA 为例：

```text
← 返回免费目录
NVIDIA API Catalog（免费）                  [长期免费] [全球平台]
使用 build.nvidia.com 生成 nvapi- Key，并在免费开发额度内调用托管模型。
最后核验：2026-07-27                        [申请免费 API Key ↗]

API Key
[••••••••••••••••••••••••••]

免费模型（3/3）
[✓] 模型 A       工具 · JSON · 128K
[✓] 模型 B       推理 · 工具 · 128K
[✓] 模型 C       视觉 · 128K

验证会发送一次极短测试请求，并消耗少量免费额度。
                                             [取消] [验证并添加]
```

API Key 申请说明固定为一句话，不在应用内维护长教程。

## 6. 视觉方向

目标用户是维护本地多 Agent 路由的 AI 工程师。页面的单一任务是：

> 快速找到一个可信的零成本入口，理解限制，并安全地接入 Token Station。

视觉继续沿用 Token Station 的“本地信号站”语言，不复制 OmniRoute 的 Dashboard：

| Token | 建议值 | 用途 |
|---|---|---|
| Canvas | 现有 `--canvas` | 页面底色 |
| Surface | 现有 `--surface` | 卡片与配置面板 |
| Surface 2 | 现有 `--surface-2` | 搜索、筛选和次级区域 |
| Ink | 现有 `--ink` | 标题与主要信息 |
| Muted | 现有 `--muted` | 限制和辅助说明 |
| Signal | 现有 `--signal` | 导航、焦点和主操作 |
| Free | `#45c98b` 附近 | 长期免费标签 |
| Trial | 现有 `--warning` | 试用额度标签 |

唯一的强视觉元素是“免费信号条”：目录标题旁用一条由若干短竖线构成的信号标记表达多个免费来源。其余区域保持当前桌面控制台的克制密度，避免绿色营销页、玻璃拟态和大面积渐变。

## 7. 权威目录模型

### 7.1 后端单一来源

新增后端权威目录，例如：

```text
apps/desktop/src-tauri/src/free_provider_catalog.rs
```

前端通过只读命令读取目录，不自行维护 Base URL、模型 allowlist 或能力：

```text
list_free_provider_presets() -> Vec<FreeProviderPresetView>
```

建议数据结构：

```rust
enum FreeOfferKind {
    Recurring,
    Trial,
}

enum ProviderRegion {
    China,
    Global,
}

enum OveragePolicy {
    HardStop,
    RateLimited,
    UserMustEnableGuard,
}

struct FreeProviderPreset {
    id: &'static str,
    upstream_name: &'static str,
    label: &'static str,
    base_url: &'static str,
    offer_kind: FreeOfferKind,
    region: ProviderRegion,
    tags: &'static [&'static str],
    free_note: &'static str,
    key_instruction: &'static str,
    application_url: &'static str,
    docs_url: &'static str,
    verified_at: &'static str,
    overage_policy: OveragePolicy,
    models: &'static [FreeModelPreset],
}

struct FreeModelPreset {
    id: &'static str,
    label: &'static str,
    tool: CapabilityState,
    vision: CapabilityState,
    json_schema: CapabilityState,
    context_window: u32,
}
```

前端可以进行本地搜索与筛选，但不能改变目录的身份、端点、免费模型集合和能力声明。

### 7.2 配置模型

在 `UpstreamConfig` 增加：

```rust
#[serde(default, skip_serializing_if = "AccessTier::is_paid")]
pub access_tier: AccessTier,

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessTier {
    Free,
    #[default]
    Paid,
}
```

旧配置没有该字段时按 `paid` 解释，保持向后兼容。免费供应商使用独立上游名：

```text
nvidia_nim       # 普通/付费实例
nvidia_free      # 免费实例
```

`ProviderView` 同步返回 `access_tier`，主页供应商列表、三档供应商下拉和模型管理区域显示`免费`标签，但不改变候选排序。

## 8. 首批目录与证据门槛

首版候选共 13 家：

| 供应商 | 目录分类 | 地区 | 首版说明 |
|---|---|---|---|
| SiliconFlow | 长期免费 | 中国 | 仅收录官方标记的免费模型 |
| ModelScope | 长期免费 | 中国 | 仅收录 API-Inference 免费调用范围 |
| 阿里云百炼 | 试用额度 | 中国 | 必须提示启用“仅免费额度”保护 |
| 腾讯混元 | 试用额度 | 中国 | 免费资源包用尽后不得由 TS 自动启用后付费 |
| Google Gemini API | 长期免费 | 全球平台 | 仅收录官方 Free Tier 模型 |
| Groq | 长期免费 | 全球平台 | 使用 Free tier 限速 |
| Mistral | 长期免费 | 全球平台 | 使用 Free mode |
| SambaNova | 长期免费 | 全球平台 | 使用无付款方式的 Free Tier |
| OpenRouter | 长期免费 | 全球平台 | 只收录 `:free` 或官方 free router |
| GitHub Models | 长期免费/预览 | 全球平台 | 使用具备 `models:read` 的 PAT |
| Cohere | 试用额度 | 全球平台 | 使用免费 Trial Key |
| Hugging Face | 长期免费 | 全球平台 | 使用每月免费推理额度 |
| NVIDIA API Catalog | 长期免费/开发用途 | 全球平台 | 只支持 build.nvidia.com 托管 API |

每个条目进入发布目录前必须同时满足：

1. 官方页面能申请 API Key 或等价静态 Bearer Token；
2. 官方文档确认存在免费层或明确试用额度；
3. 能通过 Token Station 支持的 Chat Completions 兼容路径调用；
4. 至少一个模型经过真实普通流式与工具能力核验；
5. 能说明额度耗尽后的费用行为；
6. 申请地址、文档地址和最后核验日期完整；
7. 服务条款不禁止用户以本地个人代理方式调用。

OmniRoute 的 `hasFree`、`freeNote` 和模型目录只作为候选线索，不能替代以上证据门槛。

### 8.1 首轮官方资料入口

- [SiliconFlow 免费模型与限流](https://docs.siliconflow.cn/cn/userguide/rate-limits/rate-limit-and-upgradation)
- [SiliconFlow API Key 与 OpenAI 接口](https://docs.siliconflow.cn/cn/userguide/quickstart)
- [ModelScope API-Inference](https://modelscope.cn/docs/model-service/API-Inference/intro)
- [阿里云百炼新用户免费额度](https://help.aliyun.com/en/model-studio/new-free-quota)
- [腾讯混元计费与免费资源包](https://cloud.tencent.com/document/product/1729/97731)
- [Gemini API 定价](https://ai.google.dev/gemini-api/docs/pricing)
- [Gemini OpenAI 兼容接口](https://ai.google.dev/gemini-api/docs/openai)
- [Groq Free tier 与计费](https://console.groq.com/docs/billing-faqs)
- [Groq OpenAI 兼容接口](https://console.groq.com/docs/openai)
- [Mistral 使用与限制](https://docs.mistral.ai/admin/billing-usage/usage-limits)
- [SambaNova Free Tier 限制](https://docs.sambanova.ai/docs/en/models/rate-limits)
- [OpenRouter 免费模型限制](https://openrouter.ai/docs/faq)
- [GitHub Models 免费 API](https://docs.github.com/en/github-models/use-github-models/prototyping-with-ai-models)
- [Cohere Trial Key](https://docs.cohere.com/docs/going-live)
- [Hugging Face 免费推理额度](https://huggingface.co/docs/inference-providers/pricing)
- [NVIDIA API Key](https://docs.nvidia.com/nemo/retriever/latest/extraction/api-keys/)

## 9. 验证与写入事务

新增专用命令：

```text
add_free_provider(preset_id, selected_models, api_key)
```

命令流程：

```text
解析后端 preset
  → 拒绝目录外模型
  → 检查模型至少一个
  → 检查独立 free upstream 是否已存在
  → 使用内存中的 Key 请求供应商鉴权/模型接口
  → 使用第一个选中模型发送极短真实生成请求
  → 验证响应协议
  → 构造 access_tier=free 的 UpstreamConfig
  → 写入草稿并运行现有 observe/validate
  → 写入系统钥匙串
  → 任一步失败则回滚草稿，不保存 Key
  → 返回 StateView
```

验证请求必须：

- 复用现有 EgressPolicy；
- 禁止跨域重定向携带凭据；
- 使用固定最小提示和极小输出上限；
- 不记录请求体、响应体或 Key；
- 错误只返回网络、鉴权、额度、模型或协议层分类；
- 明确告知用户会消耗少量免费额度。

免费实例已存在时不与付费实例合并。首版进入更新模式需要独立命令，保留免费身份，只允许更新免费模型集合和对应免费凭据。

## 10. 状态与失败处理

| 状态 | 页面行为 |
|---|---|
| 目录加载失败 | 显示错误与重试，不回退到普通目录 |
| 搜索无结果 | 展示当前搜索词与“清除筛选” |
| 未填写 Key | 禁用“验证并添加” |
| 未选择模型 | 禁用“验证并添加”并说明至少选择一个 |
| 验证中 | 锁定 Key、模型和导航操作，按钮显示阶段 |
| 鉴权失败 | 保留当前页内 Key，说明重新生成或检查地区 |
| 免费额度耗尽 | 不保存；提示更换免费供应商或等待额度恢复 |
| 模型不可用 | 不保存；提示目录可能过期，并记录非敏感诊断 |
| 添加成功 | 清空 Key，返回主页，显示“免费供应商已添加” |
| 页面离开 | 清空 Key；保留目录搜索与筛选状态 |

## 11. 测试与验收

### 11.1 配置与后端

- 旧配置缺少 `access_tier` 时仍可加载并按 `paid` 展示。
- `access_tier: free` 正确读写，未知值被拒绝。
- 普通与免费实例可同时存在并使用不同钥匙串条目。
- 前端提交目录外模型、伪造 URL 或伪造能力时被后端拒绝。
- 验证失败不改变配置、钥匙串和模型缓存。
- 钥匙串写入失败时回滚新增上游。
- 免费实例不会包含付费模型。
- 免费实例不会自动改变主页或 Agent 路由。

### 11.2 前端

- “添加供应商”页顶部以“常规 API / 免费 API”双标签切换，两类入口不再分成两个页面。
- 两种模式共用搜索位置、筛选位置、卡片结构、点击配置与返回目录语义。
- 搜索匹配供应商、模型、说明和标签。
- 免费类型与地区筛选可组合。
- 从普通或免费配置页返回时，保留原模式、搜索和筛选。
- 卡片在 3/2/1 列断点下无截断和横向滚动。
- API Key 默认打码，离开配置页后清空。
- 模型默认全选，可取消，但不能以零模型提交。
- 免费与付费实例在主页、下拉和供应商列表中有明确区分。
- 键盘焦点、屏幕阅读器名称和非颜色标签完整。

### 11.3 真实供应商证据

每家供应商至少记录：

- 申请 Key 成功；
- 一个普通流式请求成功；
- 一个声明支持工具的模型完成工具调用；
- 费用保护或额度耗尽行为；
- Key、请求和响应未进入日志、SQLite、配置和缓存；
- 最后核验日期。

核验失败的候选从发布目录移除，不允许仅凭第三方项目的 `hasFree` 标记上线。

## 12. 实施顺序

1. 评审本设计文档与三组视觉原型；
2. 增加 `AccessTier` 配置模型和向后兼容测试；
3. 建立后端免费目录及只读查询命令；
4. 建立真实验证与原子新增命令；
5. 增加目录页、配置页和导航；
6. 在主页与供应商选择器展示免费身份；
7. 补齐前端、Rust、密钥边界和真实供应商验收；
8. 更新产品文档和验证记录。

视觉稿确认前不修改生产代码。

# 用量统计 Dashboard 改造设计

- 日期：2026-07-23
- 状态：已完成本地实施与验证
- 范围：桌面端用量页、只读统计查询与聚合口径
- 参考实现：[cc-Switch UsageDashboard（commit a377d79）](https://github.com/farion1231/cc-switch/tree/a377d79303bc1e592d2783d559ca5bd6b8ba1417/src/components/usage)

## 1. 结论

现有页面把筛选、预算维护、定价维护和统计结果按表单顺序纵向堆叠，导致用户进入页面后先看到四个全宽原生下拉框和两组管理表单，真正的“用了多少、花了多少、是否稳定”被推到首屏以下。

本次采用“观测优先、维护收纳”的 Dashboard：

1. 顶部只保留 Agent、供应商、模型、时间范围和刷新控制，所有筛选作用于整页。
2. 首屏用一个总览面板回答总 Token、请求、成本、成功率、延迟和缓存效率。
3. 用 Token 构成轨道同时表达输入、输出和缓存命中，不再堆五张同权重卡片。
4. 增加按小时/按天趋势，成本与 Token 可切换查看。
5. 明细区用 Agent、供应商、模型、状态四个视角切换，不再要求用户理解“按什么聚合”的原生下拉框。
6. 预算状态保留在观测区；预算编辑与模型定价收进“管理与口径”折叠区。
7. 继续只读本地指标库，不记录 prompt、response 或任意请求内容。

## 2. cc-Switch 调研结果

调研基于 cc-Switch `a377d79303bc1e592d2783d559ca5bd6b8ba1417`，重点文件：

- [`UsageDashboard.tsx`](https://github.com/farion1231/cc-switch/blob/a377d79303bc1e592d2783d559ca5bd6b8ba1417/src/components/usage/UsageDashboard.tsx)
- [`UsageHero.tsx`](https://github.com/farion1231/cc-switch/blob/a377d79303bc1e592d2783d559ca5bd6b8ba1417/src/components/usage/UsageHero.tsx)
- [`UsageTrendChart.tsx`](https://github.com/farion1231/cc-switch/blob/a377d79303bc1e592d2783d559ca5bd6b8ba1417/src/components/usage/UsageTrendChart.tsx)
- [`types/usage.ts`](https://github.com/farion1231/cc-switch/blob/a377d79303bc1e592d2783d559ca5bd6b8ba1417/src/types/usage.ts)
- [`usage_stats.rs`](https://github.com/farion1231/cc-switch/blob/a377d79303bc1e592d2783d559ca5bd6b8ba1417/src-tauri/src/services/usage_stats.rs)

### 2.1 值得学习

| 机制 | cc-Switch 做法 | Token Station 采用方式 |
|---|---|---|
| 全局筛选 | 应用、Provider、模型、时间范围统一驱动总览、趋势和明细 | Agent、供应商、模型、时间统一作用于总览、趋势和分组表 |
| 信息层级 | Hero 先展示 Token 主指标，请求和成本为辅助指标 | 保留主次层级，并补充成功率与 p95 延迟 |
| Token 拆分 | 输入、输出、缓存创建、缓存命中分开展示 | 展示输入、输出、缓存读、缓存写、推理；明确子集语义 |
| 趋势 | 24 小时按小时，其余按天 | `24h` 按小时，`7d/30d/all` 按本地自然日 |
| 细分统计 | 请求日志、Provider、模型使用 Tab 切换 | 第一阶段实现 Agent、供应商、模型、状态统计 Tab |
| 刷新 | 关闭、5s、10s、30s、60s | 第一阶段提供关闭、30s、60s，默认关闭，避免无感轮询 |
| 维护功能 | 定价和重建放在 Dashboard 底部 Accordion | 预算编辑和模型定价统一收进底部折叠区 |
| 数据治理 | 本地 SQLite、详情保留期、日汇总、跨来源去重 | 本阶段继续读取现有本地 SQLite；不引入会话日志导入和跨来源合并 |

### 2.2 明确不照搬

1. cc-Switch 的 `realTotalTokens` 会把 fresh input、output、cache creation、cache read 相加，这是为其跨协议归一模型服务。
2. Token Station 的 `token_station_protocol::Usage` 已规定：
   - `cache_read_tokens` 和 `cache_write_tokens` 是 `input_tokens` 的分区/子集；
   - `reasoning_tokens` 是 `output_tokens` 的子集；
   - 总 Token 必须是 `input_tokens + output_tokens`。
3. 因此 Token Station 不把缓存和推理再次加入总 Token，否则会重复计数。
4. 不导入 cc-Switch 的 Session Log、Codex rollout 重建、Provider 用量脚本和外部账户额度查询；这些数据源不在本地网关 Receipt 的可信边界内。
5. 不照搬品牌图标筛选。Token Station 的 Agent Registry 是动态目录，筛选必须由真实 registry 数据驱动。

## 3. 当前问题

### 3.1 信息架构

- 四个全宽下拉框没有分组，且“总计/按 Agent/按模型”把展示方式伪装成数据过滤。
- 预算表单和定价表单位于统计结果之前，主任务被次要管理任务阻断。
- 汇总卡片没有明确主指标；请求、错误、延迟、Token、成本视觉权重相同。
- 输入和输出挤在同一行，缓存读写与推理 token 已落库却没有展示。
- 没有时间趋势，无法回答“成本什么时候增长”“今天是否出现错误尖峰”。
- 分组表只在选中 `by` 后出现，缺乏默认可探索路径。

### 3.2 视觉与交互

- 原生 `<select>` 在 macOS 下出现系统渐变和双箭头，和应用控件体系割裂。
- 页面没有明确的首屏锚点，长表单导致扫描路径不稳定。
- 大量相同圆角容器嵌套，缺少层级差异。
- 数字没有统一紧凑格式与完整值提示，大数难以快速比较。
- 错误、未定价和预算预警没有统一汇总。

### 3.3 数据能力

当前 `requests` 表已经持久化：

- `input_tokens`
- `output_tokens`
- `cache_read_tokens`
- `cache_write_tokens`
- `reasoning_tokens`
- `cost_micros`
- `latency_ms`
- `status`
- `agent_id`
- `upstream`
- `model`

但现有统计聚合只读取输入、输出、成本和延迟，缓存与推理信息被浪费。时间字段已存在，也足以在不修改 schema 的前提下实现趋势。

## 4. 页面任务与设计方向

### 4.1 页面单一任务

目标用户是维护本地多 Agent 网关的 AI 工程师。页面的单一任务是：

> 在 10 秒内判断选定时间与路由范围内的消耗规模、成本、稳定性和主要贡献者。

预算和定价是支撑这项任务的维护能力，不是页面入口的主任务。

### 4.2 视觉系统

沿用 Token Station 的“本地控制台 / 信号站”语义，不复制 cc-Switch 的玻璃卡片：

| Token | 值 | 用途 |
|---|---|---|
| Canvas | `#f4f6fa` / 现有 `--canvas` | 页面底色 |
| Surface | `#ffffff` / 现有 `--surface` | 主面板 |
| Ink | `#182133` / 现有 `--ink` | 标题与数字 |
| Signal | `#5368e8` / 现有 `--signal` | 输入、选中状态 |
| Output | `#8658d3` | 输出 token |
| Cache | `#1b8a76` | 缓存命中 |
| Warning | `#a96a12` / 现有 `--warning` | 未定价与预算接近 |

字体角色：

- 标题与正文：沿用系统 UI 字体，保证桌面应用一致性。
- 数据：`SF Mono / Menlo`，只用于主 Token、成本、趋势轴和表格数值。
- 标签：系统 UI 字体，使用 11–12px 和中等字重。

布局概念：用量页是一张“观测台账”，不是营销分析页。大数字只出现一次，其他信息通过轨道、刻度和紧凑表格组织。

### 4.3 标志性元素：Token 构成轨道

总览底部使用一条水平轨道：

```text
输入 68%                         输出 32%
████████████████████████████████▓▓▓▓▓▓▓▓▓▓▓▓▓
└ 缓存命中覆盖输入的 42% ────────────────────┘
```

- 主轨道按输入/输出占总 Token 的比例分段。
- 输入段内部叠加缓存命中纹理，表达“缓存是输入子集”，避免重复加总。
- 下方列出缓存写入和推理 token，并注明它们分别是输入/输出子集。
- 数据全为零时显示中性轨道，不制造百分比。

这是本页面唯一的强视觉表达，其余区域保持克制。

## 5. 页面结构

```text
┌ 用量统计 ───────────────────────────────────────────────┐
│ 本地 Receipt 聚合，不含请求内容                         │
│ [全部 Agent] [全部供应商] [全部模型]       [刷新] [近7天] │
├─────────────────────────────────────────────────────────┤
│ TOKEN 总量  66,728,074        请求 532   成本 48.5837    │
│                              成功率 98.7%  p95 1.24s      │
│ [输入/输出主轨道 + 缓存命中覆盖]                        │
├─────────────────────────────────────────────────────────┤
│ 使用趋势                         [Token] [成本]           │
│  ╭─╮   ╭────╮                                           │
│ ─╯ ╰───╯    ╰────                                       │
├─────────────────────────────────────────────────────────┤
│ [按 Agent] [按供应商] [按模型] [按状态]                 │
│ 名称         请求   成功率   Token   p95    成本          │
│ ...                                                     │
├─────────────────────────────────────────────────────────┤
│ 预算状态                                                 │
│ ▸ 预算与定价管理                                        │
└─────────────────────────────────────────────────────────┘
```

## 6. 交互规格

### 6.1 筛选

- Agent：来自 `listAgentRegistry()` 中 `admission=supported` 的动态列表。
- 供应商、模型：来自当前时间范围内的真实统计分组；筛选改变后允许级联收窄。
- 时间范围：全部、近 24 小时、近 7 天、近 30 天。
- 所有筛选作用于总览、趋势、预算状态说明和明细表。
- 切换 Agent 时清空可能已失效的供应商和模型筛选。
- 切换供应商时清空模型筛选。

### 6.2 刷新

- 提供手动刷新按钮。
- 自动刷新可选关闭、30 秒、60 秒，默认关闭。
- 刷新按钮在请求进行中显示旋转状态并禁用，避免并发重复读取。
- 页面卸载时清理计时器。

### 6.3 趋势

- `24h` 使用小时桶；`7d/30d/all` 使用本地自然日桶。
- Token 模式展示输入与输出堆叠趋势。
- 成本模式展示已定价成本；存在未定价请求时显示说明。
- 无数据时保留坐标区域并给出下一步，而不是隐藏整个模块。
- SVG 图表提供 `role="img"` 与摘要 `aria-label`；数据点可通过 `<title>` 查看完整值。

### 6.4 明细

- 默认按 Agent；可切换供应商、模型、状态码。
- 列：名称、请求、成功率、Token、p95、成本。
- 数值列右对齐并使用 tabular/monospace 数字。
- 未定价成本显示 `—`，同时显示该分组未定价请求数。
- 空分组仍显示表头和范围说明。

### 6.5 预算与定价

- 有预算时，在明细后展示紧凑预算状态行和进度。
- 预算接近、超出、即将到期、已到期、存在未定价请求时保留明确文字。
- “仅提醒，不影响路由”始终可见。
- 编辑预算和模型定价默认折叠，使用原生 `<details>` 保持键盘可达。
- 删除预算仍使用现有禁用条件；不改变 observe-only 语义。

## 7. 数据口径

### 7.1 聚合字段

`AggView` 增加：

- `cache_read_tokens`
- `cache_write_tokens`
- `reasoning_tokens`

派生值只在展示层计算：

```text
total_tokens = input_tokens + output_tokens
success_rate = (requests - errors) / requests
cache_hit_rate = cache_read_tokens / input_tokens
```

边界：

- `requests = 0` 时成功率展示 `—`。
- `input_tokens = 0` 时缓存命中率展示 `—`。
- 缓存读、缓存写均不得再次加入 `total_tokens`。
- reasoning 不得再次加入 `output_tokens`。
- `cost_micros = null` 表示范围内没有任何已定价行，不得显示为 0。

### 7.2 新分组

只读聚合增加：

- `hour`：按本地小时桶；
- `day`：按本地自然日桶。

时间组 key 使用桶起点 Unix 毫秒的十进制字符串，避免本地化文本参与排序。前端负责根据 locale 渲染。

### 7.3 新筛选

`StatsFilter` 增加精确匹配：

- `upstream`
- `model`

HTTP 与 IPC 参数保持同形：

```text
/admin/stats?since=7d&by=model&agent=codex&upstream=openai&model=gpt-5
get_stats(since, by, agentId, source, upstream, model)
```

筛选值只来自已聚合的配置元数据，不接受或返回请求内容。

### 7.4 性能边界

本阶段沿用现有“只读打开 SQLite、在 Rust 中聚合”的个人用量规模假设，不修改数据库 schema。

如果真实数据库达到以下任一条件，应单独立项做 SQL 聚合与日汇总：

- 明细超过 100 万行；
- 单次 `30d` 查询 p95 超过 200ms；
- 自动刷新导致连续 CPU 峰值超过一个刷新周期的 20%。

## 8. 组件与代码调整

### 8.1 前端

- `apps/desktop/src/pages/Stats.tsx`
  - 重构为 Dashboard 编排器；
  - 管理统一筛选、刷新、趋势模式和明细视角；
  - 保留预算读写逻辑并移入折叠区。
- 新增 `apps/desktop/src/components/UsageTrendChart.tsx`
  - 纯 SVG、无新依赖；
  - 输入/输出堆叠面积与成本折线。
- `apps/desktop/src/components/PricingEditor.tsx`
  - 不改变契约，由 Stats 折叠区承载。
- `apps/desktop/src/App.css`
  - 新增 `.usage-*` 命名空间；
  - 删除旧 `.stat-*` 视觉规则或保留兼容但不再引用；
  - 深浅主题均从现有 CSS token 派生。

### 8.2 后端与桥接

- `apps/cli/src/stats.rs`
  - 读取缓存与推理字段；
  - 增加 upstream/model 精确筛选；
  - 增加 hour/day 分组。
- `apps/cli/src/admin.rs`
  - 接受新筛选参数和新分组。
- `apps/desktop/src-tauri/src/lib.rs`
  - 扩展 `get_stats` 命令参数。
- `apps/desktop/src/api.ts`
  - 扩展 `AggView` 和 `getStats`。

## 9. 错误、加载与空状态

- 初次加载：总览与趋势显示同尺寸骨架，不跳动布局。
- 手动刷新失败：保留上一份成功数据，并在筛选栏下显示错误。
- 指标库不存在：显示“开启设置 · 本地指标并完成一次请求”，保留管理折叠区。
- 当前筛选无数据：显示“此范围暂无记录”，提供清除筛选按钮。
- 全部请求未定价：成本显示 `—`，说明需要在管理区配置模型价格。
- 部分请求未定价：显示已知成本，并标注“另有 N 个请求未定价”。
- 趋势只有一个桶：绘制点和基线，不伪造走势。

## 10. 可访问性与响应式

- 所有图标按钮具有中文 `aria-label`。
- 筛选控件均有可见标签或 `aria-label`。
- Tab 使用 `role="tablist"`、`role="tab"` 和 `aria-selected`。
- 图表不把颜色作为唯一编码；图例明确写“输入 / 输出 / 成本”。
- `prefers-reduced-motion` 下关闭轨道和图表入场动画。
- 默认桌面宽度使用两列总览；`< 980px` 收为单列；不引入横向页面滚动。
- 表格区域自身可横向滚动，首列保持可读。

## 11. 验收标准

1. 打开用量页时，首屏首先展示总 Token、请求、成本、成功率、p95 和 Token 构成，不再先展示预算/定价表单。
2. Agent、供应商、模型和时间筛选统一作用于总览、趋势和明细。
3. `24h` 趋势按小时，`7d/30d/all` 按天。
4. 输入、输出、缓存读、缓存写、推理 token 均可见，且总 Token 不重复计算缓存和推理。
5. 明细可在 Agent、供应商、模型、状态四个视角切换。
6. 预算预警继续只提醒、不影响路由；预算和定价编辑默认折叠。
7. 无数据、未定价、部分未定价、刷新失败均有明确引导。
8. 浅色与深色主题均使用同一结构和现有主题 token。
9. Vitest 覆盖筛选、刷新、视角切换、预算读写和空状态。
10. Rust 单测覆盖缓存/推理聚合、upstream/model 筛选、hour/day 分组。
11. 前端测试、生产构建、相关 Rust 测试和 `router-core` 红线检查通过。

## 12. 后续阶段

本次不阻塞实施、但值得后续单独设计：

- Receipt 请求明细分页与单条详情抽屉；
- 自定义起止时间；
- SQLite 日汇总与明细保留策略；
- 实际账单与本地估算成本对账；
- CSV/JSON 导出；
- 按 revision、fallback、协议与流式/非流式进一步分析。

## 13. 实施结果

本次已完成：

- Dashboard 信息架构与 Token 构成轨道；
- Agent、供应商、模型、时间范围统一筛选；
- 手动刷新及关闭/30 秒/60 秒自动刷新；
- Token/成本趋势切换；
- Agent、供应商、模型、状态四种明细视角；
- 缓存读、缓存写、推理 token 聚合；
- 小时/本地自然日时间桶；
- 预算状态概览和折叠式预算/定价管理；
- HTTP 与 IPC 同形筛选参数；
- 相对时间窗口转换错误修复：`24h/7d/30d` 现在会转换为绝对 cutoff，不再被误当作 Unix 时间戳。

验证结果：

- Vitest：17 个测试文件、137 项测试通过；
- 前端 TypeScript 与 Vite 生产构建通过；
- `token-station-cli` 统计测试：14 项通过；
- CLI 与 Tauri 桌面 Rust 编译检查通过；
- 宽屏、800px 窄窗口、浅色和深色视觉验收通过；
- `crates/router-core/**` 未修改。

# Gate A：运行态三态与 Runtime Supervisor 设计

> 日期：2026-07-21
>
> 状态：设计已确认；PR #40 已合入，实施等待 Contract B 骨架与时序同步
>
> 上游任务：[Agent 与网关——开发任务书](https://github.com/GlimpseEngine/token-station/blob/develop/docs/design/2026-07-21-Agent%E4%B8%8E%E7%BD%91%E5%85%B3-%E5%BC%80%E5%8F%91%E4%BB%BB%E5%8A%A1%E4%B9%A6.md)
>
> 范围：Gate A，仅包含 T1 与 T2

## 1. 目标

本设计解决两个问题：

1. 模型发现只更新目录缓存，不得让 Provider 配置草稿变脏。
2. 将配置草稿、磁盘已保存配置、代理正在运行的配置拆成三份事实，使“保存并应用”真正完成安全切换。

完成后必须满足：

- `draft_revision`、`saved_revision`、`running_revision` 分别代表草稿、磁盘和已 publish 实例。
- `running_revision` 只能在 `serve_lifecycle.rs::publish` 成功时写入 AppState。
- 运行配置 A 切到 B 后，下一条新请求命中 B；Apply 失败时继续使用 A。
- 旧实例的在飞请求最多获得 5 秒自然完成时间，超时后才取消。
- Runtime API、UI 与后续 Request Receipt 使用同一个 `running_revision`。
- serve task 异常退出后，1 秒内不得继续显示正常运行。

## 2. 非目标

Gate A 不实现：

- SSE 字节解析、`StreamOutcome` settle、错误分类和重试预算；
- 能力四态、多模态能力路由和 Provider 生命周期；
- Profile、Agent 挂载板和 Request Receipt；
- 实时配置文件监听；
- 可由用户调整的 drain 时间；
- 配置 diff 视图。

Gate A 只使用上游提供的 Contract A/B 类型，不另立同名协议类型。

## 3. 核心不变量

1. 一个 revision 永远只对应一份完整配置内容。
2. 草稿规范化指纹与 saved 指纹不同时表示有未保存更改；revision 关系只作为该事实的稳定标识，不替代内容判等。
3. 代理运行时，`saved_revision != running_revision` 表示磁盘配置尚未应用；代理停止时不推断运行配置。
4. draft 或 saved 都不能在运行状态不可确认时冒充 running。
5. 模型发现缓存与配置草稿使用独立存储路径和写入口。
6. 同一时刻最多一个 Apply；Apply 期间允许继续编辑和仅保存。
7. 异步操作必须经过 generation 校验，过期结果不得 publish。

## 4. 组件边界

### 4.1 ConfigState

`ConfigState` 是桌面控制面的配置事实源，持有：

- 可编辑的 `draft` 与 `draft_revision`；
- 最近成功写盘的 `saved` 快照与 `saved_revision`；
- revision ledger 路径与最近配置指纹。

后端命令修改草稿后先计算规范化指纹：与当前草稿相同则不推进 revision；与 saved 指纹相同则复用 `saved_revision`；否则分配新 revision。这样 A→B→A 会回到 saved revision，不会永久误报未保存。所有页面共同编辑一份完整草稿，不引入页面级 revision。

### 4.2 RuntimeSupervisor

`RuntimeSupervisor` 是代理生命周期的唯一所有者，持有：

- 当前已 publish 的 `RunningInstance`；
- serve task handle 与 listener 所有权；
- `instance_id` 与 `running_revision`；
- 停止接收新连接的控制信号；
- 在飞请求计数与强制取消 token；
- 当前 operation generation。

`RunningInstance` 不复制或推断草稿状态。其 revision 来自本次 Apply 捕获的不可变 saved 快照。
每次成功 publish 生成新的 UUID v4 `instance_id`；停止态没有实例 ID。

### 4.3 RuntimeProbe

`RuntimeProbe` 独立采集运行事实：

- serve task 是否存活；
- 实际 `listen` 地址是否可建立 TCP 连接；
- 是否至少一个已发现 Agent 仍正确指向 Token Station。

### 4.4 ModelCatalogCache

`model_catalog.rs::discover_with_cache` 只读写 `CacheEntry`：

```text
base_url / models / fetched_at_ms
```

发现成功、缓存回退和缓存写入 warning 可以从 `ConfigState` 提取只读的 discovery 输入，但不得修改 `ConfigState` 或分配 revision。只有用户明确选择模型并写入配置，才属于草稿修改。

## 5. Revision 持久化

revision 元数据写入配置同目录的 `token-station.state.json`，不修改 `token-station.json`。sidecar 至少保存：

```text
last_allocated_revision
committed_revision
committed_fingerprint
pending_revision?
pending_fingerprint?
```

保存采用轻量 pending journal：

1. 校验完整草稿并计算规范化配置指纹。
2. 原子写 sidecar 的 pending revision 与 fingerprint。
3. 原子替换 `token-station.json`。
4. 将 pending 提升为 committed，并再次原子写 sidecar。

步骤 2 失败时不写配置。步骤 3 成功后即视为保存成功并更新内存中的 saved 快照；若步骤 4 失败，返回元数据持久化 warning，重启时按 pending 指纹完成恢复。

启动恢复规则：

- 磁盘指纹等于 pending：完成 pending 提交；
- 磁盘指纹等于 committed：丢弃无效 pending；
- 两者都不相等：视为外部修改，分配并提交新 revision。

revision 可以跳号，但不能复用为不同配置。App 启动后令 `draft = saved`、`draft_revision = saved_revision`，尚未 publish 时 `running_revision = null`。

## 6. 保存与应用流程

### 6.1 编辑与发现

- 有效配置修改生成新的 `draft_revision`。
- 修改后的规范化指纹等于 saved 时，`draft_revision` 回到 `saved_revision`，不为相同内容创造第二个身份。
- Apply revision N 期间允许继续编辑，后续修改生成 N+1。
- 模型发现不分配 revision，也不触发“有未保存更改”。

### 6.2 仅保存

仅保存执行完整校验与 journal 写盘：

```text
saved = draft snapshot
saved_revision = draft_revision
running_revision unchanged
```

### 6.3 停止态保存并应用

停止态点击“保存并应用”会启动代理：

```text
capture revision N
→ save N
→ prepare_server(N)
→ preflight(N)
→ bind
→ publish
→ running_revision := N
```

Gate A 的 preflight 是无计费、无真实推理请求的本地检查：candidate 必须成功构建 Gateway、加载所需插件和路由候选，并通过 listen 地址与运行参数校验；端口占用留到正式 bind 阶段处理。

### 6.4 运行态保存并应用

运行态严格按 listener handoff 处理：

```text
capture revision N
→ save N
→ prepare new Gateway without occupying the port
→ preflight candidate
→ stop old listener from accepting new connections and release it
→ bind the candidate to the same address
→ publish candidate and set running_revision := N
→ let old in-flight requests finish for at most 5 seconds
→ cancel requests still running after the grace period
→ release the old runtime
```

新请求只允许落到新实例，或在极短 bind 空窗内收到 connection refused；不得继续落到旧配置。

任务书原 Contract B 将 `old.drain.cancel()` 写在 5 秒宽限期之前，但该 token 又是请求取消 token 的父节点，两者会导致旧请求立即取消。经确认，本设计以“先等待 5 秒，超时后再 cancel”为准；实现前应由接口负责人同步 Contract B 骨架或文字定义。

### 6.5 Apply 并发

- 每次 Apply 捕获不可变配置快照和 revision。
- 同一时刻只允许一个 Apply，重复 Apply 返回 `apply_in_progress`。
- Apply 期间允许编辑和仅保存。
- 若 N 应用期间保存了 N+1，N publish 后自然呈现“已保存尚未应用”。
- Stop 或取消使当前 generation 失效；晚到的 prepare 结果只能 discard。

## 7. 切换失败与恢复

| 失败阶段 | saved revision | running revision | 处理 |
|---|---:|---:|---|
| 配置校验或写盘 | 不变 | 不变 | 返回保存错误，不启动 candidate |
| prepare / preflight | 已推进到 N | 保持旧值 | 旧实例继续服务，显示“已保存尚未应用” |
| 新 listener bind | 已推进到 N | 保持旧值 | 保留旧 Gateway，立即重新绑定旧实例 |
| 旧 listener 恢复也失败 | 已推进到 N | 不冒充可用 | 返回切换错误，显示“运行态未知” |
| 新实例 publish 后旧实例 drain 异常 | 已推进到 N | N | 不回滚新实例；记录 warning 并清理旧 runtime |

旧 Gateway 和旧 runtime 必须保留到新实例 publish 成功。只有 publish 成功才能推进 AppState 中的 `running_revision`。

## 8. RequestContext 与 drain

server 层按 Contract B 创建并注入 `RequestContext`：

- 客户端断开只取消对应请求；
- Apply 或 Stop 先停止旧 listener 接收新连接；
- 旧请求在 5 秒 grace 内自然执行；
- grace 到期后触发父取消 token；
- gateway 将取消结果交给 Contract A，最终记为 `ClientCancelled/499`；
- ClientCancelled 不计为上游失败，也不摘除上游健康状态。

Contract B 骨架必须提供 server 可调用的 constructor/factory、父取消 token 注入方式，以及客户端断开时触发单请求取消的所有权接口或 guard。缺少这些入口时不得由 Gate A 自行补一个平行 `RequestContext`。

Gate A 不实现或复制 `StreamOutcome`、settle 和 gateway 对 `RequestContext` 的消费逻辑，只完成 server/serve lifecycle 侧的创建、注入和 drain 编排。

## 9. Runtime API

对外运行事实不得压缩为一个 `running` 布尔：

```text
app_runtime: stopped | running
listener_reachable: bool
agent_connected: bool
running_revision: number | null
instance_id: string | null
```

字段来源：

- `app_runtime`：已 publish serve task 是否仍存活；
- `listener_reachable`：短超时 TCP 连接真实探测 `listen`；
- `agent_connected`：重新读取并解析至少一个已发现 Agent 的实际配置，确认它仍正确指向当前 Token Station 实例；ownership 记录只能帮助定位受管文件，不能单独证明仍已连接；
- `running_revision`、`instance_id`：当前 `RunningInstance`。

内部可以保留 `preparing/switching/draining/error` 等 transition phase，用于按钮禁用和进度提示，但不能替代上述事实字段。

Supervisor 持有 task handle，并在任务退出时立即清除运行实例和推送状态事件。listener 每 500ms 以不超过 200ms 的连接超时探测一次，确保异常后 1 秒内 UI 不再显示正常运行。

## 10. 前端状态

顶部状态：

- task 不存在：代理已停止；
- task 存在且 listener 可达：代理运行中；
- task 存在但 listener 不可达：运行态未知；
- Agent 连接状态独立展示。

保存区状态：

- 草稿规范化指纹与 saved 指纹不同：有未保存更改；
- 代理运行且 `draft_revision == saved_revision != running_revision`：已保存尚未应用；
- 代理运行且 `draft_revision == saved_revision == running_revision`：运行中 revision N；
- 代理停止且 `draft_revision == saved_revision`：无改动。

进入后三种 revision 状态前，必须先确认草稿与 saved 指纹相同；revision 只表达保存与 publish 的配置身份，不独立承担内容判脏。

运行事实探测失败时不得回退展示 draft 或 saved 配置。

## 11. 测试策略

### 11.1 T1

- 连续刷新 N 次，`draft_revision` 不变；
- live、cache fallback、cache write warning 三条路径均不污染草稿；
- 刷新后保存按钮不显示“有未保存更改”；
- 用户明确写入模型后才推进 draft revision。

### 11.2 Revision 与恢复

- 编辑、保存、publish 分别只推进对应 revision；
- A→B→A 编辑后，草稿复用 A 的 `saved_revision`，保存区恢复为无未保存更改；
- 重复值不生成 revision；
- 重启后 revision 延续；
- 外部修改配置后启动时生成新 revision；
- journal 在配置写入前、配置写入后和完成提交后三个崩溃点均能恢复；
- Apply N 期间编辑或保存 N+1，publish 结果仍为 N。

### 11.3 Runtime Supervisor

- 停止态“保存并应用”能够启动并 publish；
- 运行配置 A 切换到 B 后，下一条真实请求命中 B；
- A 的在飞请求可在 5 秒内自然完成；
- 超时旧请求被取消并记为 499；
- preflight 失败继续使用 A；
- bind 失败恢复 A，双重 bind 失败显示“运行态未知”；
- 过期 generation 无法 publish；
- serve task 被 kill 后，1 秒内 API 和 UI 不再显示正常运行。

### 11.4 API 与 UI

- Runtime API 分别返回五个规定字段；
- `agent_connected` 按“至少一个 Agent 正确接入”计算；
- 四种保存文案按 revision 关系展示；
- listener 不可达时不展示任何伪造的运行配置；
- 不新增 diff 视图。

## 12. 实施前置条件

1. PR `codex/agent-integration-pr` 已由负责人合入 `develop`。
2. 接口负责人提供 Contract A/B 类型骨架，其中 Contract B 包含 server 创建与绑定取消信号所需的公开入口。
3. Contract B 明确采用“5 秒 grace 后再触发请求取消”的语义，并修正任务书旧段落中“先 cancel 再等待”的冲突描述。
4. Contract B 未同步前，不开始 T2/T5/T6 实现；可推进不依赖 Contract 的 T4。

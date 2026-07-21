# 代理启动非阻塞与生命周期状态设计

日期：2026-07-20

状态：已完成本地实施，等待用户验收

## 1. 背景与根因

当前桌面端点击“启动代理”后，前端直接等待 `serve_start` 返回。后端命令在持有
`AppInner` 全局 Mutex 的情况下依次执行：

- 配置物化与校验；
- 文件日志和 SQLite 指标库初始化；
- `Gateway::new`；
- Wasmtime Engine 创建以及 Agent、Provider WASM 插件加载；
- 虚拟 Key 的系统凭据访问；
- Tokio Runtime 创建；
- TCP 监听地址绑定。

这些工作包含同步磁盘 I/O、系统凭据访问和 WASM 编译/实例化。它们既占用当前
Tauri 命令执行路径，又在整个过程持有应用状态锁，因此启动耗时期间窗口交互和
其他状态读取可能被拖慢。前端目前只有一个通用 `busy` 布尔值，不能区分“正在
启动”“运行中”“启动失败”和“正在取消”。

本设计只解决启动过程阻塞界面和生命周期竞态问题。Agent 扫描缓存由
`2026-07-20-agent-scan-cache-design.md` 独立处理。

## 2. 目标

1. 点击“启动代理”后立即进入可见的“正在启动”状态，窗口和标签切换保持可交互。
2. 日志、SQLite、Gateway/WASM、Keychain、Runtime 和端口绑定等重型工作不在窗口
   事件线程执行，也不在执行期间持有 `AppInner` Mutex。
3. 后端作为代理生命周期的唯一事实来源，明确表达停止、启动、取消、运行和失败。
4. 同一时刻最多存在一个启动准备任务；重复点击不得创建重复 Gateway 或重复抢占端口。
5. 启动中可以请求取消；不能强制中断的同步准备工作完成后必须丢弃结果，禁止发布服务。
6. 所有失败都回到可重试状态，不留下半运行服务、残留监听器或对前端错误发布的虚拟 Key。
7. 保持现有 `Gateway`、服务数据面和配置语义不变。
8. 不修改 `crates/router-core/**`，不改变路由算法、规则匹配、评分、选池、排序、
   `RouterConfig` 或 `Decision` 核心契约。

## 3. 非目标

- 不在本阶段缩短 WASM 冷编译本身的总耗时。
- 不增加 WASM 预编译缓存、跨进程 Engine 缓存或常驻预热进程。
- 不改变 Agent/Provider 插件格式、发现规则、权限沙箱或加载顺序。
- 不重写 `Gateway::new`、`server::serve` 或 CLI 数据面。
- 不改变监听地址、鉴权、虚拟 Key、日志或指标的业务语义。
- 不实现开机自动启动、崩溃自动拉起或后台守护进程。
- 不为 Agent 扫描与代理启动引入统一任务调度器；只有新鲜性能证据证明资源竞争
  仍不可接受时，才单独设计优先级或预热机制。

## 4. 方案比较与选择

### 4.1 方案 A：只增加前端 Loading

点击后显示“正在启动”，继续调用现有同步 `serve_start`。

优点是改动最少；缺点是重型工作和全局锁范围完全不变，只改变文案，无法保证窗口
可交互，也无法解决重复启动和启动/停止竞态。因此不采用。

### 4.2 方案 B：后台准备任务 + 后端生命周期状态机

轻量 `serve_start` 只完成校验、状态切换和后台任务派发，立即向前端返回
`starting`。后台任务在阻塞线程池准备服务，完成后用短锁原子提交结果，并通过
Tauri 事件通知前端。

该方案能直接消除界面阻塞，同时不修改 Gateway 和路由内核，失败与取消语义也可
完整测试。本阶段采用此方案。

### 4.3 方案 C：WASM 预热或编译缓存

在 App 启动时预建 Engine，或持久化 Wasmtime 编译产物。

该方案可能缩短冷启动总时长，但会引入版本失效、缓存完整性、插件更新、资源占用
和安全审计问题。当前证据只能证明启动链路存在阻塞，不能证明必须引入缓存，因此
本阶段不采用。

## 5. 核心原则

### 5.1 控制面与准备工作分离

- 控制面只负责校验请求、推进状态、分配 generation 和提交最终结果。
- 准备工作负责创建独立的 `PreparedServer`，期间不访问或持有 `AppInner` Mutex。
- 只有 generation 仍有效且状态仍允许发布时，准备结果才能变成 `RunningServer`。

### 5.2 后端状态权威

前端可以为即时反馈先展示命令返回的 `starting`，但不得自行推断服务已经运行。
只有后端提交 `running` 后，才能展示虚拟 Key、建立 Admin Endpoint 或允许依赖代理
的数据面操作。

### 5.3 取消采用“禁止发布”而非强杀线程

Wasmtime 编译、Keychain 和部分同步 I/O 不能安全地强制中断。启动中点击取消后：

1. 后端将状态切换为 `stopping`；
2. 当前 generation 被标记为不得发布；
3. 准备线程自然结束后立即释放 listener、runtime 和其他资源；
4. 状态切换为 `stopped`；
5. `stopping` 完成前拒绝新的启动请求，保证最多一个准备任务在途。

## 6. 生命周期数据模型

后端用显式枚举替代 `Option<RunningServer>` 对全部状态的隐式表达：

```text
ServerLifecycle
  Stopped
  Starting  { generation, listen }
  Stopping  { generation, listen }
  Running   { generation, server }
  Failed    { generation, listen, public_error }
```

`generation` 单调递增，只用于后端判定迟到结果，不暴露为授权凭据。

桌面 IPC 的 `ServeView` 增加：

```text
phase: stopped | starting | stopping | running | error
running: boolean
listen: string
virtual_key: string | null
error: string | null
```

兼容约束：

- 暂时保留 `running`，其值严格等于 `phase == running`，避免现有数据页误连尚未完成的服务。
- `virtual_key` 只在 `running` 时返回。
- `error` 只包含可展示的脱敏错误，不包含 API Key、配置原文、快照内容或未截断插件输出。
- `get_state` 在任意阶段都必须快速返回当前快照。

## 7. 组件边界

### 7.1 启动准备单元

增加一个边界清晰的启动准备单元，可放在
`apps/desktop/src-tauri/src/serve_lifecycle.rs`：

```text
prepare_server(config: ClientConfig) -> Result<PreparedServer, StartError>
```

它只负责：

- 打开 FileLog 和可选 SQLiteStore；
- 调用现有 `Gateway::new`；
- 读取或创建虚拟 Key；
- 创建 Tokio Runtime；
- 绑定 TCP Listener；
- 返回尚未发布到全局状态的完整准备结果。

它不负责修改 App 状态、不发 Tauri 事件、不改变路由配置，也不吞掉具体启动阶段的
错误。测试中允许用受控 fake preparer 替代真实重型初始化，以稳定复现阻塞、失败和
竞态。

### 7.2 生命周期协调单元

生命周期协调逻辑负责：

- 分配 generation；
- 保证单飞；
- 将准备任务派发到 `tauri::async_runtime::spawn_blocking`；
- 校验完成结果是否仍可发布；
- 原子提交 `RunningServer`；
- 清理取消或过期的 `PreparedServer`；
- 发出统一的 `serve-state-changed` 事件。

所有状态加锁区只允许进行枚举判断、配置快照、generation 更新和结果交换，不得在
锁内执行文件、数据库、Keychain、WASM、Runtime shutdown 或 socket 操作。

### 7.3 前端状态协调

`App.tsx` 不再用通用 `run()` 包装代理启停，而使用独立的生命周期处理：

- App 挂载时先注册 `serve-state-changed` 监听，再调用 `get_state`，避免漏掉完成事件；
- 点击启动后使用命令立即返回的 `starting` 快照更新界面；
- 后续以事件中的后端快照更新为 `running`、`error` 或 `stopped`；
- 组件卸载时注销事件监听；
- `setAdminEndpoint` 只在 `phase == running` 时启用本地 HTTP 端点；
- 迟到的启动完成事件不能覆盖后端已经返回的取消状态。

## 8. 启动数据流

### 8.1 正常启动

1. 前端调用 `serve_start`。
2. 后端短暂加锁：校验配置、拒绝冲突状态、复制 `ClientConfig`、生成 generation，
   将状态设为 `Starting`。
3. 后端派发后台准备任务并立即返回 `ServeView(phase=starting)`。
4. 前端显示“正在启动”，但标签导航仍可操作。
5. 后台线程执行 `prepare_server`，全程不持有 App Mutex。
6. 准备成功后，协调单元短暂加锁检查 generation 和状态。
7. 若仍为同一 `Starting`，启动 `server::serve` 并原子提交 `Running`。
8. 后端发出 `serve-state-changed`；前端显示监听地址和虚拟 Key。

### 8.2 重复启动

- `Running`：幂等返回当前运行快照。
- `Starting`：返回当前启动快照，不再派发任务。
- `Stopping`：返回明确的 `startup_cleanup_in_progress` 错误，要求等待清理结束。
- `Failed` 或 `Stopped`：允许发起新的 generation。

### 8.3 启动中取消

1. 前端在 `starting` 状态展示“取消启动”。
2. `serve_stop` 将同一 generation 切换为 `Stopping` 并立即返回。
3. 准备任务完成后发现状态已不允许发布，执行资源清理。
4. 清理完成后切换为 `Stopped` 并发事件。
5. 无论准备结果成功还是失败，都不能重新进入 `Running`，也不显示误导性的启动失败。

### 8.4 正常停止

1. `serve_stop` 在短锁内从 `Running` 取出 `RunningServer`，保留 generation 并将状态
   切换为 `Stopping`。
2. 命令立即返回 `Stopping`，并在锁外的后台清理任务中关闭 runtime、等待 listener
   释放。
3. 清理结束后再次校验 generation，切换为 `Stopped` 并广播最终快照。
4. `Stopping` 完成前拒绝重新启动，避免旧 listener 尚未释放时重新绑定同一端口。

停止命令在 `Stopped` 下保持幂等；在 `Failed` 下用于清除错误并回到 `Stopped`。

### 8.5 启动失败

- 配置物化失败发生在任务派发前，命令直接返回现有校验错误，状态保持 `Stopped` 或
  原有 `Failed`。
- 后台准备失败时，只有 generation 仍有效才能提交 `Failed`。
- 错误必须带稳定阶段标识，例如 `log_open`、`metrics_open`、`gateway_init`、
  `virtual_key`、`runtime_init`、`listen_bind`，并附带脱敏恢复建议。
- 用户可以直接从 `Failed` 再次启动；新 generation 会清除旧错误。

## 9. 界面行为

| phase | 顶栏状态 | 主按钮 | 其他页面 |
|---|---|---|---|
| `stopped` | 已停止 | 启动代理 | 正常可用 |
| `starting` | 正在启动 | 取消启动 | 标签可切换，窗口保持响应 |
| `stopping` | 正在停止 | 禁用并显示正在停止 | 标签可切换，窗口保持响应 |
| `running` | 运行中 · 地址 | 停止 | 正常可用 |
| `error` | 启动失败 | 重试启动 | 展示脱敏错误，其他页面可用 |

启动过程中可以暂时禁用会改变本次启动配置的表单提交，但不能用全屏遮罩阻断导航、
Agents 展开、只读浏览或错误查看。

## 10. 并发、资源与错误边界

- 同一进程最多一个 `PreparedServer` 构建任务在途。
- 所有迟到结果都通过 generation 和当前 lifecycle 双重校验。
- `PreparedServer` 未发布或被取消时必须显式释放 listener 并关闭其 runtime；整个清理
  过程不得阻塞窗口事件线程，也不得在 App Mutex 内等待线程退出。
- 准备阶段可能按现有语义创建日志、SQLite 文件或可复用的 Keychain 条目；取消只保证
  不发布服务、不暴露虚拟 Key，不删除这些正常且幂等的持久化产物。
- 端口绑定只发生一次。绑定失败不得写入 `Running`。
- 虚拟 Key 只有完整启动成功后才进入前端快照；失败日志不得输出 Key。
- Tauri 事件只是状态通知，`get_state` 始终是恢复和重载后的权威兜底。
- Agent 后台扫描可以与启动准备并行，但二者不得共享授权缓存或相互持锁。若实际测量
  显示并发造成明显启动时间回退，再单独设计调度策略，不在本阶段预先扩展。
- 窗口关闭或进程退出时允许操作系统终止尚未发布的准备任务；不得创建独立常驻子进程。

## 11. 测试与验收

### 11.1 Rust 单元与命令测试

- 使用阻塞 fake preparer 时，`serve_start` 返回 `starting`，不等待 fake 解除阻塞。
- fake preparer 阻塞期间，`get_state` 能返回，证明准备过程未持有 App Mutex。
- 连续两次启动只调用 preparer 一次。
- `Starting → Running` 只在相同 generation 下成立。
- 启动中停止进入 `Stopping`；迟到成功结果被清理，最终为 `Stopped`。
- `Stopping` 期间的新启动被拒绝，不产生第二个准备任务。
- 日志、SQLite、Gateway、Keychain、Runtime 和 bind 各阶段失败均进入可重试 `Failed`。
- 旧 generation 的失败或成功都不能覆盖新状态。
- 正常停止在锁外关闭 runtime，listener 释放后才进入 `Stopped`，并保持重复停止幂等。
- `crates/router-core/**` 红线检查保持零差异。

### 11.2 前端测试

- 点击“启动代理”后立即显示“正在启动”，不等待完成事件。
- fake 后台准备任务保持未完成时，可以切换主页、Agents 和设置页。
- `serve-state-changed(running)` 到达后才展示虚拟 Key 并启用 Admin Endpoint。
- `serve-state-changed(error)` 展示错误和“重试启动”。
- 启动中点击“取消启动”进入“正在取消”，迟到完成不得显示运行中。
- 组件卸载时正确注销事件监听，重复挂载不产生多重监听。
- 现有保存配置、正常启停和数据页端点测试继续通过。

### 11.3 人工验收

1. 使用测试钩子将启动准备人为阻塞至少 3 秒；阻塞期间连续切换标签、展开 Agent、
   移动窗口，界面均能响应。
2. 冷启动真实代理，确认“正在启动”立即出现，完成后只产生一个监听服务。
3. 启动过程中取消，确认最终保持已停止，端口未被占用，虚拟 Key 不显示。
4. 人为占用监听端口后启动，确认进入“启动失败”，释放端口后可直接重试成功。
5. 使用损坏测试插件触发 Gateway 初始化失败，确认错误脱敏且没有半启动服务。
6. 快速重复点击启动、取消和重试，确认不会产生双实例或状态倒退。

## 12. 完成标准

- 重型启动准备全部离开窗口事件线程和 App Mutex 临界区。
- `serve_start` 在准备任务尚未完成时即可返回 `starting`。
- 生命周期的五种状态、重复启动、取消、失败重试和停止均有确定行为。
- 页面在人工阻塞启动准备时仍可切换和操作只读内容。
- 启动失败或取消后不存在运行中的 listener、错误发布的 runtime 或泄露的虚拟 Key。
- 现有 Gateway、插件加载和数据面行为保持不变。
- 自动化测试、桌面构建和红线脚本通过，且 `crates/router-core/**` 零修改。

## 13. 后续性能决策

本阶段实施后记录各启动阶段耗时，只记录阶段名和毫秒数，不记录配置值、文件内容、
路径中的用户信息或任何凭据。若新鲜数据证明窗口已不阻塞但冷启动时长仍不可接受，
再为 Wasmtime Engine 复用、组件缓存或安全预热另写 Spec；不得把这些优化顺带加入
本阶段实现。

## 14. 本地实施结果

- `serve_start` 已改为轻量派发：先返回 `starting`，重型准备进入 Tauri 阻塞线程池。
- 后端已实现 `stopped / starting / stopping / running / error` 五态、generation 单飞、
  启动取消、锁外停止和 `serve-state-changed` 通知。
- 前端已实现即时启动状态、取消、失败重试、事件恢复和仅在 `running` 时启用 Admin
  Endpoint。
- 本机 Debug 冷启动测量中，`gateway_init` 约 21 秒，其余已记录阶段接近 0 毫秒；
  该耗时不再阻塞窗口，WASM 冷编译优化仍按本设计留待独立证据和独立 Spec。
- Rust 测试、前端测试、生产构建、Clippy、格式和 router-core 红线检查已通过。

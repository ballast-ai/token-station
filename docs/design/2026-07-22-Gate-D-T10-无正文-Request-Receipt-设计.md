# Gate D / T10：无正文 Request Receipt 设计

> 日期：2026-07-22
> 范围：`requests / decisions / attempts / conversion_reports` 四表与首页最近 5 次回执。

## 1. 硬不变量

1. 回执库不存 prompt、response、原始 header、明文 Key，也不提供可装入自由正文的列。
2. 一个 `request_id` 是唯一记账单元。入口使用操作系统随机源生成 128 bit ID，不再使用“毫秒 + 进程内计数器”；同一请求的 decision、attempt 和 conversion 只能挂在该 ID 下，重放写入幂等。
3. `attempts` 只记真正获得 Provider permit 并进入 southbound 路径的尝试；本地 admission 拒绝不伪装成上游请求。
4. 成本必须是 `actual / estimated / unknown` 三态。当前价格表计算只能产生 `estimated`；无价格或计算为 0 时保持 `unknown + NULL`，不声称本地模型免费。`actual` 留给未来账单对账。
5. Desktop 启动的每条回执携带 `running_revision`，必须与 Runtime API/UI 当时公布的 revision 一致；CLI 独立 serve 没有该账本时为 `NULL`。
6. 旧库先备份再前向迁移。历史请求可继续显示扁平摘要，但不伪造当时并未记录的逐次 attempt/conversion。

## 2. 四表边界

```text
requests (1 / request_id)
  ├─ decisions (0..1 / request_id)
  ├─ attempts (0..N / request_id + ordinal)
  └─ conversion_reports (0..N / request_id + ordinal)
```

- `requests`：请求终态、协议、Agent、运行 revision、延迟、token、成本三态。为兼容既有统计查询，保留旧版扁平路由列，它们继续表示最终实际服务者；新代码不得把这些列解释为原始路由决策。
- `decisions`：原始选中的 pool / Provider / model、决策来源、fallback 数和无正文 features。该记录在路由完成时固化，后续 fallback 不覆盖；实际服务者由 attempts 终态及 `requests` 的兼容路由列给出。
- `attempts`：序号、Provider/model、延迟、HTTP 终态、`ErrorCode`、`StreamOutcome`、是否允许换 Provider。
- `conversion_reports`：只记 `inbound_normalize / provider_request / provider_response / outbound_render / stream_translate` 等闭集阶段、源/目标协议、成败和 `ErrorCode`；不记转换前后正文。

`RequestRecord` 继续作为 `Recorder` 的唯一写入参数，并扩展为聚合回执：新增 `agent_id`、`running_revision`、`cost_kind`、`decision`、`attempt_records`、`conversion_reports`。既有 `routing` 字段保持“最终实际服务者”语义，避免改变统计口径。文件日志与 SQLite 共用该无正文模型，不另建可容纳任意 JSON 的旁路字段。

### 2.1 受控枚举与空值

- `cost_kind` 仅允许 `actual / estimated / unknown`。`unknown` 的 `cost_micros` 和 `price_version` 必须为 `NULL`；当前本地价格表只能写 `estimated`；本期不存在写入 `actual` 的生产路径。
- `conversion_reports.stage` 仅允许 `inbound_normalize / provider_request / provider_response / outbound_render / stream_translate`。
- `attempts.fallback_allowed` 表示当前错误分类是否允许透明换 Provider，不表示当时一定存在下一个候选。
- 未发生的 decision、attempt、conversion 保持缺失或空集合；迁移旧行时不补造历史事件。

## 3. 写入时序

1. 请求入口产生随机 `request_id`，绑定 Agent 和 `running_revision`。Desktop 在 publish 新 Runtime 时把已成功发布的 revision 注入该 server 的 `AppState`；CLI 独立 serve 注入 `None`。
2. normalize 成功或失败均追加 conversion 元数据；路由成功后同时初始化兼容 `routing` 和不可变 `decision`。fallback 只更新兼容 `routing`，不覆盖 `decision`。
3. 每次获得 Provider permit、开始 southbound 后启动 attempt 计时；尝试结束时一次性追加 Provider/model、延迟、HTTP 终态、错误码、流式终态和 fallback 分类。admission、本地预算拒绝和排队等待不计 attempt。
4. 转换阶段在阶段结束时写一条元数据；流式转换只在整个流终结时写一条，不按 chunk 落库。最终 `settle` 仍是成功唯一出口。
5. Recorder 在请求结束后用一个 SQLite transaction 写四表：先 `INSERT OR IGNORE requests`，仅在确实插入父记录时继续写 children；如果 `request_id` 已存在，整个重放为 no-op。

### 3.1 迁移策略

- schema 从 v3 前向迁移到 v4：给 `requests` 增加 Agent、revision、成本状态列，并创建 `decisions / attempts / conversion_reports`。
- 迁移前沿用现有备份约定生成一次数据库备份；迁移在事务中完成，失败不提升 `user_version`。
- v3 历史行的 `cost_kind`：有非零成本时标记 `estimated`，无成本或 0 成本标记 `unknown` 并读为 `NULL`；不创建历史 decision/attempt/conversion。
- 读取层同时支持 v4 详情和迁移后的旧行扁平摘要，不能因 children 为空而隐藏历史请求。

## 4. 读取与前端

- 新增只读 `recent_receipts(limit)`；调用方即使传入更大值也硬截断为 5，按 `started_at_ms DESC, request_id DESC` 稳定读取。
- `/admin/receipts` 与 Tauri IPC 返回同一结构；指标库未建时返回空数组，不制造错误。
- 首页仅展示最近 5 条一行摘要：时间、Agent/协议、路由、终态、延迟、token、成本三态。
- 点开单条后才显示 decision → attempts → conversions 时间线；不轮询、不画图、不做实时流量大盘。

读取模型为固定结构体，不透传数据库行或任意 JSON。管理 API 继续复用现有本机管理面鉴权；前端只在首页装载时读取一次。

## 5. 验收证据

1. SQLite schema 存在且仅由 `requests / decisions / attempts / conversion_reports` 承载回执，列名扫描不含正文/header/key 容器。
2. 两次 fallback 生成 1 request + 1 decision + 2 attempts，顺序与终态正确；转换报告不携带任何请求/响应正文。
3. 数据库和文件日志原始字节均找不到 prompt/tool/header canary。
4. 未定价/本地模型显示 `unknown`，不存储 0；价格表计算显示 `estimated`；无账单数据时不会出现 `actual`。
5. Desktop 真实请求回执的 `running_revision` 与 Runtime API/UI 一致。
6. 首页只显示 5 条，展开后可看 decision/attempt/conversion 时间线和空态。
7. v3 数据库迁移后历史请求仍可读取，但 attempts/conversions 为空；重复记录同一 request_id 不产生孤儿或重复 children。

## 6. 实现边界与取舍

采用“扩展 `RequestRecord` 聚合模型”的方案：保留现有 `Recorder::record(&RequestRecord)` 接口，网关在内存中收集无正文 children，SQLite 用单事务拆写四表。该方案改动集中，并让文件日志与数据库使用同一隐私模型。

不采用以下方案：

- 新建一套 `RequestReceipt` 并整体替换 Recorder：领域命名更纯，但会迫使所有 recorder、测试和调用点同步迁移，本期没有额外收益。
- 从既有 attempts 计数或日志反推逐次尝试：无法还原 Provider、终态和顺序，会违反“不伪造历史事件”。

本期明确排除 Agent Skill 用量、实时轮询、图表、导出、账单对账和任意正文调试字段。

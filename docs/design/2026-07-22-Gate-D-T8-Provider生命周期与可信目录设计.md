# Gate D / T8：Provider 生命周期与可信模型目录设计

> 日期：2026-07-22
> 范围：Provider Add / Edit / Test / Catalog / Usage / Delete 闭环；不包含 Agent Skill 用量。

## 1. 不变量

1. `/models` 只产生目录事实，不直接修改配置草稿；刷新 N 次仍不改变 `draft_revision`。
2. 目录中的模型不会因一次刷新缺失而消失：新出现标 `active`，曾出现但本次缺失标 `removed`，live 获取失败时已知模型标 `stale`。
3. 每个目录模型携带 `source / last_seen_ms / catalog_state` 和 tool、vision、JSON Schema 四态；没有证据就是 `unknown`。
4. 配置仍在引用的模型不能被静默删除。目录刷新只给 diff；用户保存模型集合时由后端重新校验主页与五个 Agent 的引用。
5. 删除 Provider 必须先返回引用预览；存在引用时 fail-closed，不自动清空路由。确认删除后，原 Provider JSON 进入本地 tombstone sidecar，可恢复，不做不可逆硬删。
6. Provider 详情页是生命周期入口：基本信息、分层测试、目录 diff、能力、用量摘要、删除影响在同一展开区域完成。
7. 目录证据与 Provider 身份绑定：新增、URL/Key 变更、删除和恢复都先使旧目录失效；同名同 URL 也不得继承另一账号的 live 证据。

## 2. 目录账本

缓存升级为 version 2：

```text
ProviderCatalog {
  base_url,
  revision,
  fetched_at_ms,
  models: [
    { model, capability, source, last_seen_ms, catalog_state }
  ]
}
```

- live 刷新成功：revision + 1；命中的模型为 `active/live` 并更新 `last_seen_ms`；旧模型未命中则保留为 `removed`。
- live 失败但有缓存：返回缓存，非 removed 模型在视图中标 `stale/cache`，不覆盖最后一次可信 live 账本。
- version 1 缓存读取时原地迁移为 version 2 语义：原模型视为 `active/cache`，能力全 `unknown`，`last_seen_ms=fetched_at_ms`。
- Provider 详情视图把配置中手工存在、但目录从未见过的模型补为 `active/configured`；这只是配置来源，不伪造 live 验证。

刷新结果同时返回 `added[] / removed[]`。前端明确展示 diff；`removed` 条目继续可见，且已配置/被引用的模型仍保留在模型选择与路由中。

## 3. 删除两阶段

1. `preview_provider_removal(name)` 收集主页三档和所有 Agent 自定义路由引用，返回稳定路径列表。
2. UI 展示影响面；有引用时只给“先调整路由”指引，不允许确认。
3. 无引用时 `remove_provider(name)` 再次校验，随后先原子写入 `provider-tombstones.json`，再从草稿移除并更新 revision。
4. `restore_provider(name)` 从 tombstone 恢复；若同名 Provider 已存在则拒绝，避免覆盖用户新配置。
5. tombstone 存在时同名新增必须拒绝，用户需先恢复再编辑；`archive` 自身也拒绝覆盖旧恢复点。

## 4. 分层测试与用量

详情页测试采用诚实状态：DNS/网络、HTTP、鉴权、模型、生成使用已有 `probe_layered` 的真实 southbound 请求；基础生成通过后，继续分别发出并校验流式终态、指定 Tool Call 和 JSON Schema 结构化输出。只有对应真实请求成功才能标 `pass`；基础生成未通过时更深三层标 `skipped`，不以模型名猜测。

Provider 用量按现有无正文 metrics 的 `upstream` 维度聚合，只显示请求数、错误数、延迟、token 和成本三态；不读取 prompt/response。

## 5. 验收

1. 两次 live 目录中第二次缺失的模型仍在账本，状态为 `removed`，diff 明确列出。
2. live 失败使用缓存时状态为 `stale`，不会把失败当空目录写入。
3. 刷新目录不改变草稿 revision；保存移除被引用模型失败并指出引用。
4. 删除前能预览全部主页/Agent 引用；有引用删除失败，无引用删除生成 tombstone，随后可恢复。
5. Provider 详情页能看到最终 URL、能力、目录状态/diff、测试状态、用量和删除影响。
6. 删除后同名同 URL 重加不得出现旧目录；旧 tombstone 不得被第二次删除覆盖。
7. 通过 Provider 编辑和模型保存命令产生新 revision 后，下一条真实代理请求必须命中新 Provider 运行态。

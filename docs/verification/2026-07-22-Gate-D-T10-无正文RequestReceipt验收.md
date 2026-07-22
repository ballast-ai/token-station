# Gate D / T10 无正文 Request Receipt 验收

> 日期：2026-07-22
> 结论：本地验证完成，可以进入 Gate D 下一项。

## 1. 范围

- SQLite schema v4：`requests / decisions / attempts / conversion_reports`。
- 网关回执：随机 request ID、原始 decision、真实 southbound attempts、转换闭集、成本三态。
- Desktop 运行 revision 注入、`/admin/receipts`、Tauri IPC 与首页最近 5 条。
- 排除 Agent Skill 用量、实时轮询、图表、导出和账单对账。

## 2. 关键不变量证据

1. `requests` 保留旧扁平 routing 作为最终实际服务者；`decisions` 固化 fallback 前的原始决策。真实 fallback 用例验证二者分别为 `mock_fallback` 与 `mock_primary`。
2. 只有取得 Provider permit、进入 southbound 的调用才追加 attempt。本地 admission 跳过不增加 attempt；每条 attempt 保留序号、目标、延迟、原始上游 HTTP/错误、`StreamOutcome` 和 fallback 分类。上游 `2xx + 协议错`与被拒绝的 `3xx` 都保留真实状态码，无 HTTP response 的传输失败保持 `NULL`。
3. conversion stage 是五态闭集；流式路径在整体终结时写一次结果，不按 chunk 落库。
4. `request_id` 正常路径使用操作系统随机源生成 128 bit ID；同一 ID 的父记录未插入时，三个子表整体 no-op。
5. v3 库迁移前生成备份；迁移事务新增三子表和 Agent/revision/cost_kind，旧行可读但不伪造 children。
6. 本地价格表只能产生 `estimated`；0、无价格或无有效价格版本统一规范化为 `unknown + NULL`；生产路径不写 `actual`。
7. Desktop 在 Runtime publish 成功时把 revision 注入对应 server。首次运行、热切换以及 apply 失败后继续使用旧 Runtime 的真实请求均与 UI/runtime revision 一致。
8. 读取层、HTTP、IPC 和前端均硬限制最多 5 条；首页仅 mount 读取一次，以原生 `details` 展开 decision → attempts → conversions。

## 3. 隐私红线

- 四表列名扫描不存在 prompt、请求/响应正文、raw header、API key、secret 或任意 JSON/detail 容器。
- 端到端 canary 同时放入请求正文、响应正文、tool 描述、header 和 Provider Key；`metrics.sqlite` 与 `requests.log` 原始字节均无命中。
- 未知 Agent URL 命名空间不会写入 `agent_id`；只有宿主校验过的固定 Agent ID 可持久化。

## 4. 验证命令

```text
cargo test -p token-station-cli request_receipt_tests --lib
cargo test -p token-station-cli store::tests --lib
cargo test --workspace
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
cargo fmt --all -- --check
cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
cd apps/desktop && npm test -- --run
cd apps/desktop && npm run build
git diff --check
```

结果：回执/存储定向测试 10/10；根工作区全量测试通过（CLI 119 通过、1 环境忽略；proxy 49 通过、1 子进程 fixture 忽略，其余 crate/集成/文档测试全通过）；Desktop 148 通过、1 环境忽略，另 3 个 YAML 回归通过；前端 90/90 与构建通过；两套格式、严格 Clippy 和 `git diff --check` 全通过。

## 5. 后续边界

- `actual` 等待未来账单对账来源，不由本地价格表冒充。
- v1-v3 历史请求只展示扁平摘要；不反推或补造 decision、attempt、conversion。
- 下一项按路线图进入 T7 Phase 0/1；本次未提前实现 T7/T9。

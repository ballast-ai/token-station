# Agent 接入 Task 6–7 验证记录（2026-07-20）

## 结论

- Task 6 的配置计划、加密快照、ownership、原子事务、断开与恢复功能退出条件已满足。
- Task 7 的 7 个结构化 Tauri IPC、服务端计划仓、窗口会话确认令牌和前端 API 契约已实现并通过定向与全量测试。
- `crates/router-core/**` 未修改；冻结目录摘要仍为 `637c90a0988afa23e059ecf991853101abbd60ade57365645c58eed068a84a1a`。
- 桌面 Rust 聚合行覆盖率实测 `80.91%`，尚未达到计划要求的 `90%`；该硬门禁保留到 Task 12，不能视为通过或降低阈值。

## Task 6 安全事务证据

- 计划 IPC 投影只含字段级脱敏 diff，内部 patch 与完整投影不可序列化、不可 Debug。
- `before_hash` 绑定目标路径、存在状态、权限、owner 和精确字节，缺失文件与空文件 revision 不同。
- 快照使用 OS keychain 中的 256-bit master key、AES-256-GCM、每份随机 nonce 和固定 AAD；索引不含密钥或明文。
- Unix 快照/索引为 `0600`、目录为 `0700`；每 Agent/目标保留 5 份未固定快照，pinned 不清理。
- 写入顺序为同目录临时文件、flush、fsync、权限/owner、二次 revision 校验、原子替换、目录 fsync。
- 未确认、revision 变化、keychain/快照失败均零写入；temp create、temp fsync、replace 失败保持原目标。
- 写后读取、解析、自检、ownership 提交失败会从本次加密快照恢复；主失败与恢复失败分别返回。
- 断开和恢复只投影 owned paths，用户后来新增的非归属字段保持；受管字段被用户改动时失败关闭并要求重新预览。
- 原配置不存在时，写后失败会恢复为“不存在”；成功新建权限为 `0600`。
- 当前兼容状态变为 `DETECTED_BLOCKED` 后禁止新连接，但仍允许已绑定的断开/快照恢复。

## Task 7 IPC 边界证据

- renderer 只能提交 `agent_id`、最近扫描结果中的精确 `installation_path` 查找键、`operation_id`、`snapshot_id` 和确认 token。
- command 不接收目标配置路径、patch、配置内容、可执行路径、命令或 argv。
- 多实例只有在服务端对扫描缓存做唯一精确匹配后才解除冲突标记；任意路径、遍历字符串和过期扫描键均拒绝。
- 首次接入的缺失配置目标由 Registry + 受限进程环境在服务端推导；plan 阶段只保存在内存，不创建目标、快照或 ownership 文件。
- 计划仓最多保留 64 个短期计划；正确 token 一次性原子消费，错误 token 不消费。
- HMAC token 绑定 operation ID、脱敏计划摘要、Tauri window label、到期时间和随机 challenge；跨窗口、跨计划、跨 connect/restore 入口均拒绝。
- 连接计划额外绑定代理 origin、虚拟 Key 摘要和三个 Adapter readiness；apply 前运行态变化则要求重新预览。
- apply 前重新执行只读扫描并比较 canonical installation、版本、配置指纹、可运行状态、Connector 和兼容目录 sequence。
- 快照列表仅返回必要元数据，不暴露 envelope hash、before hash、owner、权限、密文或原始字节。
- 未安装 Agent 仍由 Registry 返回卡片，当前内置列表稳定包含 5 个 Agent。

## 已执行验证

| 门禁 | 结果 |
|---|---|
| `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml transaction` | 11 passed |
| `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml commands` | 6 passed |
| 桌面 Rust 全量测试 | 84 passed, 1 ignored（真实 keychain runner） |
| 桌面 Rust Clippy `-D warnings` | passed |
| `npm --prefix apps/desktop run test -- --run api` | 8 passed |
| 前端全量测试 | 12 passed |
| `npm --prefix apps/desktop run build` | passed |
| workspace / desktop `cargo deny` | advisories、bans、licenses、sources passed |
| workspace / desktop `cargo audit` | 无漏洞；17 个项目既有且已允许的维护性告警 |
| `scripts/test-router-core-redline.sh` | 17 scenarios passed |
| router-core 冻结摘要 | exact match |
| desktop Rust `--fail-under-lines 90` | **failed: 80.91%**，Task 12 待收口 |

真实 OS keychain round-trip 按计划保持 ignored，只能在隔离受控 runner 执行，普通单测不创建或删除开发者真实 keychain 条目。

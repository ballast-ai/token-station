# Agent 兼容目录发布与回滚

本文是桌面端 Agent 兼容目录的发布运行手册。兼容目录只决定某个已发现版本能否调用应用内已有 Connector；它不是更新器、插件市场或远程配置执行器。

## 1. 当前生产状态

源码中的 `PRODUCTION_CATALOG_URL` 与 `PRODUCTION_CATALOG_PUBLIC_KEY` 当前均为空。正式构建因此不发起目录请求，只使用随应用发布、经过测试的内置目录。

只有以下三项书面确认后才允许修改生产常量：

1. 固定 HTTPS URL（禁止重定向、query、fragment 和 userinfo）；
2. Ed25519 公钥及其指纹；
3. 发布负责人、复核人、私钥保管人和事故联系人。

缺任一项都保持空值，不能用临时 URL、测试公钥或运行时参数代替。

## 2. 信任与格式

- 对 HTTP body 的**原始 bytes**做 Ed25519 签名；服务器不得重新格式化、压缩或转码。
- 签名放在唯一的 `x-token-station-compatibility-signature` 响应头中，使用 128 位小写十六进制。
- body 不超过 512 KiB，必须是 UTF-8 JSON，并通过 `deny_unknown_fields` schema 校验。
- 目录只能包含 Agent ID、版本范围、baseline、配置指纹、阻断原因和本机构建已有的 Connector ID。
- 禁止包含命令、脚本、下载 URL、配置路径、patch、路由规则、密钥或可执行代码。
- 生产私钥离线保存，不进入仓库、CI secret、构建机、目录服务器或应用包；CI 只持测试密钥。

## 3. sequence 与时效规则

- `sequence` 必须为正整数，并在每次内容变化时严格递增。
- 同 sequence、不同 bytes 视为冲突并拒绝；低于已接受 sequence 视为回滚攻击并拒绝。
- 远程目录必须有 `expires_at_ms`，有效期不超过 366 天；发布时间、过期时间或最低 App 版本无效均拒绝。
- “撤回错误目录”不能重新发布旧 sequence，必须基于上一已接受内容生成更高 sequence。
- 缓存绑定生产 URL 和公钥指纹，验签后以私有权限原子替换。

## 4. 发布前演练

每次改兼容范围都先用测试密钥和 mock `CatalogTransport` 运行：

```bash
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml \
  compatibility_valid_signed_catalog_is_cached_idempotently_and_rollback_is_rejected
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml \
  compatibility_signature_tamper_expiry_and_network_failure_fall_back_without_cache_write
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml \
  compatibility_offline_cache_preserves_blocks_but_revokes_remote_allows
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml \
  compatibility_release_drill_blocks_then_withdraws_at_a_higher_sequence
```

演练必须证明：

1. 原始 bytes 签名可通过，目录被缓存；
2. 相同 bytes/sequence 幂等；
3. 篡改、错误签名、过期、超大响应和 schema 错误不写缓存；
4. 目录引用未知 Agent/Connector 时拒绝；
5. 网络失败时远程新增 allow 被撤销，已缓存 block 仍保留；
6. sequence 回退及同 sequence 异内容被拒绝；
7. 紧急阻断生效；撤回阻断必须使用更高 sequence，旧阻断不能再回滚覆盖。

## 5. 正常发布

1. 从当前最高 sequence 的已接受原始文件复制新目录，不从旧草稿开始。
2. 只增加已经完成 Connector 准入和真实 E2E 的版本；新 Agent 版本默认保持 unknown。
3. 运行 Task 12 全量门禁、五 Agent 受控验收和 router-core 红线检查。
4. 两人复核 Agent ID、版本范围、Connector ID、时效、minimum App version 与 sequence。
5. 离线签名最终原始 bytes，记录 SHA-256、公钥指纹、签名、sequence、签署人与时间。
6. 上传不可变对象，再原子更新固定 URL；读取线上 bytes 重新验签。
7. 先在内部只读扫描观察，再允许内部接入，随后小范围灰度。

## 6. 紧急阻断与撤回

发现配置破坏、数据丢失或协议不兼容时：

1. 在目标 Agent entry 增加最窄的 `blocked` SemVer 范围和不含 URL/敏感信息的原因；
2. sequence 加一、缩短合理有效期、签名并发布；
3. 验证目标版本为 `DETECTED_BLOCKED` 且没有 `PreviewConnect`；
4. 保留已有 ownership 的断开和恢复通道；不得删除快照或静默改回配置；
5. 观察目录验签、阻断命中、写入失败和恢复失败指标。

若阻断错误，复制当前最高 sequence 的目录，删除错误规则，以**更高 sequence**重新签名发布。禁止重新暴露旧低 sequence 文件作为“回滚”。

## 7. 应用/目录回退

触发条件：任一用户配置丢失、恢复失败、目录验签异常、未知版本误放行或 Connector 大面积失败。

```text
停止扩量
→ 保留证据和已接受目录 bytes
→ 发布更高 sequence 的收紧目录（必要时只保留 block）
→ 回退到上一已验证应用版本
→ 指导用户从加密快照恢复
→ 复核 ownership 与目标文件 revision
```

删除远程对象、下发低 sequence 或更换同 URL 公钥都不是合法回退。生产远程配置异常时，可发布将 URL/公钥重新置空的应用版本，回到内置目录；已有快照和 ownership 继续用于安全退出。

## 8. 灰度顺序与停止条件

```text
内部只读扫描
→ Claude Code / Codex / OpenCode 内部接入与断开
→ OpenClaw / Hermes 受控测试
→ 小范围灰度
→ 观察配置失败率与恢复率
→ 扩大范围
```

每阶段记录应用版本、目录 sequence、Agent 版本、操作数、写入失败数、自动恢复数与 repair-required 数。任何 repair-required 或未解释的配置差异立即停止扩量。

## 9. 发布记录模板

```text
catalog_version:
sequence:
body_sha256:
public_key_fingerprint:
issued_at / expires_at:
变更 Agent 与版本范围:
关联 Connector / E2E 证据:
签署人 / 复核人:
灰度范围:
回退基线:
```

# 第二批协作任务：T6 ConnectorProjection 验收

> 日期：2026-07-22
> 范围：#6 VS Code 扩展式增量投影与可逆恢复
> 结论：本地实现与自动化验收通过

## 1. 交付结论

- `ConfigChangePlan.projection` 以逐文件方式公开 format、目标路径、前后 revision、owned paths、forward/reverse 脱敏变化和 credential slot 来源。
- 精确 patch value 仅存在于没有 `Serialize`/`Debug` 的服务端 `PatchOperation`；IPC 中敏感字段只显示 `local_virtual_key` 或 `encrypted_snapshot`。
- `apply_patch_with_reverse` 在应用每个正向操作前记录原语义值，按逆序生成反向 patch；计划签发前会实际应用反向 patch并核对所有 owned paths。
- connect、disconnect、snapshot restore 与 Claude Desktop companion 文件共用同一投影和事务链。
- 前端展示目标文件与脱敏字段 diff，只有点击“确认并应用”后才提交；已连接 Agent 的“恢复 Agent 原始配置”走相同预览链。

## 2. 安全边界证据

- 计划序列化测试断言不包含虚拟 key、源配置 marker 或 patch `value`，但包含类型化 credential source。
- revision 在快照和写盘前复验；owned value 被外部修改时 disconnect 拒绝写盘。
- 恢复从加密快照读取受管基线，仅投影 owned paths；用户之后修改的非受管字段保持不变。
- JSON5/YAML/TOML/dotenv 的未知字段、注释、顺序或 decoration 由各自 lossless codec 保留。
- Claude Desktop profile 与 `_meta.json` 仍按一个多文件事务提交；任一复验失败逆序恢复已替换文件。

## 3. 关键测试

- `reverse_patch_restores_owned_values_without_reverting_unowned_edits`
- `plan_is_redacted_and_binds_instance_revision_catalog_and_expiry`
- `transaction_disconnect_restores_only_owned_paths_and_preserves_later_user_fields`
- `transaction_snapshot_restore_updates_ownership_without_replacing_unowned_fields`
- `openclaw_transaction_connect_restore_disconnect_preserves_json5_comments`
- `hermes_transaction_connect_restore_disconnect_preserves_yaml_comments`
- `claude_desktop_profile_and_meta_commit_together_and_recover_together`
- `previews the redacted Connector projection before applying it`
- `previews and confirms one-click restoration to the encrypted baseline`

## 4. Fresh verification

```text
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
结果：lib 158 passed, 1 ignored；YAML regression 3 passed；doc tests 通过

cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
结果：通过，0 warning

npm test -- --run
结果：11 files passed，108 tests passed

npx tsc --noEmit
结果：通过

cargo fmt --all -- --check
结果：通过

git diff --check
结果：通过
```

## 5. 通过条件对照

| 条件 | 结果 |
|---|---|
| 仅修改目标字段并保留其余内容 | 通过 |
| 有字段级正向/反向 diff | 通过 |
| apply 前必须预览并确认 | 通过 |
| 可一键恢复到接管前且保留非受管新改动 | 通过 |
| 凭据不进入 IPC patch value | 通过 |
| 并发修改与 ownership 漂移 fail-closed | 通过 |
| 单文件和多文件事务均可回滚 | 通过 |

# 可逆 ConnectorProjection 设计

日期：2026-07-22
状态：已实现并通过自动化验收
对应任务：第二批协作任务 #6

## 1. 背景

现有 Agent 接管链已经具备结构化 patch、加密快照、ownership HMAC、revision 校验和字段级恢复，但存在两个产品/契约缺口：

1. `ConfigChangePlan` 只有正向脱敏变化，没有一等的、多文件可逆投影契约；
2. 前端获取计划后立即提交，用户无法在写盘前查看将修改的字段。

本阶段复用既有事务系统，不新增平行的备份或写盘通道。

## 2. 目标

- 定义 IPC-safe `ConnectorProjection`，逐文件描述格式、路径、revision、owned paths、正向 diff、反向 diff 和 credential slot 来源。
- 在服务端生成包含精确值的反向 `PatchOperation`，验证其能恢复接管前受管字段；精确值不得实现 `Serialize` 或 `Debug`。
- 实际撤销继续以加密快照和 ownership 为事实源，保留用户接管后的非受管修改。
- 前端必须先展示脱敏 diff，经用户明确确认后才能调用 apply；已接管状态提供恢复原始配置入口。

## 3. 执行步骤

1. 为配置 codec 增加 `apply_patch_with_reverse`，按 forward 操作的逆序生成反向 patch。
2. 在计划期把反向 patch 应用于 projected document，并逐个 owned path 与 baseline 比对；无法恢复则拒绝签发计划。
3. 为主文件及 companion 文件生成统一 `ConnectorFileProjection`。
4. 敏感字段只在公开契约中暴露 `local_virtual_key` 或 `encrypted_snapshot` 来源，不暴露值。
5. connect、disconnect、snapshot restore 均生成正/反向 diff；多文件计划保持相同事务提交/回滚语义。
6. 前端增加配置投影预览对话框，展示目标文件和 `human_diff`，提供取消与“确认并应用”。
7. 以 JSON/JSON5/TOML/YAML/dotenv、Claude Desktop 双文件事务和完整前端测试回归。

## 4. 退出条件

- 反向 patch 无法在任一受支持格式恢复 owned paths；
- 精确凭据或完整配置进入可序列化 IPC、日志或前端状态；
- disconnect/restore 会覆盖接管后新增的非受管字段；
- 多文件计划无法证明失败后的整体恢复；
- 用户要求暂停或调整范围。

## 5. 通过条件

- 接管只修改 Connector 声明的 owned paths，未知字段和格式装饰按 codec 契约保留；
- 每个投影文件都包含正向与反向脱敏 diff，服务端验证反向 patch；
- 凭据只暴露 slot 来源，序列化计划不包含密钥或 patch value；
- apply 前可见 diff 且必须明确确认；
- 恢复只回写受管字段，接管后的非受管修改仍保留；
- revision 变化、owned value 漂移和格式歧义均 fail-closed；
- Rust、TypeScript、Vitest、clippy 和格式门禁通过。

## 6. 交付产物

- `ConnectorProjection`、`ConnectorFileProjection`、`CredentialBinding` 公共类型；
- 跨格式反向 patch 生成和计划期验证；
- connect/disconnect/restore 的多文件投影视图；
- 配置 diff 预览与确认 UI、一键恢复入口；
- codec、计划、事务、前端回归测试；
- `docs/verification/2026-07-22-第二批协作任务-T6ConnectorProjection验收.md`。

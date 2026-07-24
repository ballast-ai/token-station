# Codex 鉴权与 TOML 表恢复修复设计

日期：2026-07-24
状态：实施中
范围：Codex Connector 的 GUI 鉴权与 Agent 原始配置恢复

## 1. 现场问题

在 macOS Codex App 中接入后，文本与图片请求均显示：

```text
Missing environment variable: `TOKENSTATION_KEY`.
```

同时点击“恢复 Agent 原始配置”失败：

```text
配置路径 '/model_providers/tokenstation' 只允许 TOML 标量
```

前者来自 Connector 只写 `env_key = "TOKENSTATION_KEY"`。GUI 启动的 Codex 不会继承终端的临时环境变量，因此无法读取本地虚拟 Key。后者来自恢复计划用一条 `Add /model_providers/tokenstation` 回放完整 provider 对象，但 TOML patch codec 仅能写标量。

## 2. 目标与边界

- Codex App 与 CLI 都可获得 Token Station 的本地虚拟 Key，文本和图片请求走同一 Responses 入站路径。
- 连接、断开和从快照恢复能够处理完整 `model_providers.tokenstation` TOML 表。
- 虚拟 Key 继续作为敏感字段：不得进入 IPC、错误、diff 或日志；目标配置仍经既有私有权限和快照事务保护。
- 不修改非 Token Station provider、无关配置字段或快照恢复策略。

## 3. 设计

官方 Codex 配置参考允许 provider 使用 `experimental_bearer_token`，并说明 `env_key` 依赖进程环境。Token Station 的 GUI Connector 改为：

1. 声明需要本地虚拟 Key；
2. 将 Key 写到 `model_providers.tokenstation.experimental_bearer_token`；
3. 将该路径标为 sensitive，使计划和 UI 仅显示脱敏占位；
4. 删除对 `TOKENSTATION_KEY` 的运行时依赖和误导性的成功提示。

TOML codec 的 `Add` / `Replace` 扩展为可把 JSON object 递归投影成 `toml_edit::Table`。这只服务于服务端反向 patch 验证；实际恢复仍通过 `project_owned_paths` 从加密快照投影，保留非受管字段与 decoration。

## 4. 验收

- Codex 连接计划包含敏感的 `experimental_bearer_token`，不再写 `env_key`。
- 无虚拟 Key 时 Codex 接入拒绝，不产生配置写入。
- 完整 provider 表的 TOML 反向 patch 可恢复且不触发“只允许 TOML 标量”。
- 既有连接、恢复与安全脱敏测试通过；新 App 经“恢复原始配置 → 再接入”验证。

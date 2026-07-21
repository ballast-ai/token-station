# OpenClaw Connector 验收记录

日期：2026-07-20
结论：`openclaw-v1` 对 `2026.6.11` 通过本地准入；其他版本保持未知保护。

## 1. 官方取证

| 项目 | 证据 |
|---|---|
| 官方仓库 | `openclaw/openclaw` |
| 目标 tag | `v2026.6.11` |
| tag commit | `e085fa1a3ffd32d0ea6917e1e6fb4ecbffbb77d2` |
| 本机只读版本 | `OpenClaw 2026.6.11 (e085fa1)` |
| 版本命令 | `openclaw --version` |
| 默认配置 | `~/.openclaw/openclaw.json` |
| 环境覆盖 | `OPENCLAW_CONFIG_PATH`、`OPENCLAW_STATE_DIR` |
| 配置格式 | JSON5，允许注释、尾逗号和未引号键 |
| Provider 路径 | `models.providers.<id>` |
| 协议 | `api: "openai-completions"` |

官方页面：

- <https://github.com/openclaw/openclaw/releases/tag/v2026.6.11>
- <https://docs.openclaw.ai/gateway/configuration>
- <https://docs.openclaw.ai/gateway/config-tools>

执行日最新稳定版为 `2026.7.1`，主分支 package 版本为 `2026.7.2`。二者没有被本次
内置目录扩大为 verified，升级后按 unknown 保护。

## 2. JSON5 codec 决策

严格 JSON 重写会丢失 OpenClaw 官方允许的注释，因此不可准入。实现采用固定版本
`json-five 0.3.1` 的 round-trip AST，并增加以下防护：

- 原注释和空白随 AST 上下文保留；
- 只修改 declared owned paths；
- 用独立语义反序列化复验结果；
- 递归拒绝重复 JSON5 键；
- owned path 祖先含 `$include` 时 fail closed；
- 解析错误不回显配置行或密钥；
- 写入仍统一经过加密快照、revision、原子替换、写后解析和 ownership。

该依赖仍处于早期版本，因此固定精确版本，不通过远程目录改变 codec；升级依赖必须重新跑
注释、重复键、fuzz/非法输入和官方 schema 验收。

## 3. 自动化结果

- `openclaw_connector_preserves_json5_comments_and_restores_only_owned_paths`：通过；
- `openclaw_transaction_connect_restore_disconnect_preserves_json5_comments`：通过；
- `config_json5_projection_preserves_comments_unknown_fields_and_rejects_duplicates`：通过；
- Registry supported/discovery-only 能力边界：通过；
- `2026.6.11` verified、`2026.7.1` unknown：通过；
- OpenClaw 2026.6.11 隔离 `config validate --json`：`valid: true`、warnings 为空；
- 校验前后 fixture SHA-256 一致；
- 隔离状态只落入一次性 `/tmp/token-station-openclaw-validation.*` 并已清理；
- Chat Completions 文本、流式、function tool、401、上游错误和日志脱敏复用
  `apps/cli/tests/proxy.rs` 协议回归。

未读取、修改或启动用户 `~/.openclaw` 实例；未安装、升级 OpenClaw。

## 4. 范围外

OpenClaw Gateway、远程 channel、MCP、浏览器和用户 skills 不在 Connector 准入范围。
本任务未修改 `crates/router-core/**`、`crates/protocol/**`、
`apps/cli/src/gateway.rs` 或 `apps/cli/src/server.rs`。

# Hermes Agent Connector 验收记录

日期：2026-07-20
结论：`hermes-v1` 对 package `0.18.0`（官方发布 `v2026.7.1`）通过本地准入；
`0.18.2` 等其他版本保持 unknown 保护。

## 1. 官方取证

| 项目 | 证据 |
|---|---|
| 官方项目 | `NousResearch/hermes-agent` |
| 验收发布 | `v2026.7.1`，tag commit `7c1a029553d87c43ecff8a3821336bc95872213b` |
| package 版本 | `0.18.0` |
| 本机只读输出 | `Hermes Agent v0.18.0 (2026.7.1)` |
| 可执行文件/版本命令 | `hermes` / `hermes version` |
| 默认配置 | `~/.hermes/config.yaml` |
| 环境覆盖 | `HERMES_HOME/config.yaml` |
| Provider | `model.provider: custom` |
| 协议 | `model.api_mode: chat_completions` |
| 只读校验 | `hermes config check` |

官方页面：

- <https://github.com/NousResearch/hermes-agent/releases/tag/v2026.7.1>
- <https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/configuration.md>
- <https://github.com/NousResearch/hermes-agent/blob/main/website/docs/developer-guide/provider-runtime.md>

执行日官方最新发布 `v2026.7.7.2` 的 package 版本为 `0.18.2`。本次没有把它推断为兼容，
避免“同一月份发布”被误当成配置契约相同。

## 2. YAML codec 选型证据

普通 YAML serializer 会丢失用户注释和排版，不能用于 Hermes live config。实现固定使用：

- [`yaml-edit 0.2.3`](https://docs.rs/yaml-edit/0.2.3/yaml_edit/)：lossless CST 和路径编辑；
- [`serde_norway 0.9.42`](https://docs.rs/serde_norway/0.9.42/serde_norway/)：独立语义解析。

实测 `yaml-edit` 会丢弃首个键之前的 leading comment，因此 Codec 显式保存并重新拼接该
前导块；自动化测试覆盖顶部注释、行内注释、未知字段和重复写入幂等。语义层使用自定义
递归 Visitor 拒绝重复键和 merge key，并在每次 patch 后重新解析。多个 document、非法
YAML、非字符串 mapping key、非对象根或非标量 patch 全部 fail closed；错误不回显源行。

## 3. 自动化与隔离 E2E

- `config_yaml_projection_is_lossless_and_rejects_ambiguous_input`：通过；
- `hermes_connector_preserves_yaml_comments_and_restores_only_owned_paths`：通过；
- `hermes_transaction_connect_restore_disconnect_preserves_yaml_comments`：通过；
- `0.18.0` verified、`0.18.2` unknown：通过；
- Hermes 版本输出优先规范化 package SemVer `0.18.0`：通过；
- 非法/重复 YAML 的发现指纹和 Connector 写入均阻断：通过；
- 快照落盘文件不含 fixture 明文密钥，IPC plan Debug 不含虚拟 Key：通过。

官方 CLI 在独立 `/tmp/token-station-hermes-validate.*` 中执行 `hermes config check`：

- config version 33：通过；
- 校验退出码：0；
- 校验前后 `config.yaml` SHA-256 均为
  `f136ac6d7f6c5b6721e9468e4c369f9eb6a52231ab5968ce349beec9116a1de5`；
- 未读取、写入或启动真实 `~/.hermes`。

Hermes 真实客户端使用临时 `HERMES_HOME` 连接仅监听回环地址的确定性 Chat Completions
测试端点：

| 场景 | 结果 |
|---|---|
| SSE 流式文本 | 输出 `HERMES_TEXT_OK` |
| `read_file` 工具调用与结果回传 | 输出 `HERMES_TOOL_OK` |
| 上游 503 | 输出包含 `503` 与 `HERMES_UPSTREAM_ERROR`，不误报成功标记 |

发现一个上游语义：Hermes `--oneshot` 在上述 503 场景仍返回进程退出码 0。因此自动化以
错误对象/标记是否传播为断言，不能只依赖进程码。Token Station 的共用 `agent-openai`
数据面另由 `apps/cli/tests/proxy.rs` 覆盖文本、SSE、function tool、鉴权和上游错误。

## 4. 红线与范围

### cc-Switch 交叉检查

参考 [cc-Switch v3.17.0 Hermes 实现](https://github.com/farion1231/cc-switch/blob/v3.17.0/src-tauri/src/hermes_config.rs)：

- 它把 Hermes 视为累加式 Provider 配置，保护未知/未来字段，并处理 `HERMES_HOME` 与平台默认路径；
- 它以 top-level section 替换方式写 YAML，并对历史重复顶层键采用 keep-last 自愈；
- Token Station 学习“保留未知字段”和“路径优先级”思路，但不复制其写入策略：重复键在安全事务中属于歧义，必须阻断；Token Station 只投影五个 owned scalars，并增加预览确认、加密快照、revision CAS、写后语义复验和字段 ownership。

cc-Switch 的 `custom_providers` 管理目标是多 Provider UI；Token Station 的目标是把当前 Hermes
模型入口安全接到本机统一代理，因此采用 NousResearch 官方同样支持的顶层 `model` custom
endpoint 契约。竞品源码没有替代官方 schema 或 E2E 证据。

- 未安装、升级、迁移或修复 Hermes；
- 未修改用户真实 `~/.hermes/config.yaml` 或 `.env`；
- 未修改 `crates/router-core/**`；
- 未为 Hermes 增加路由算法、选池、评分或排序特判；
- gateway、远程平台、浏览器、skills、memory 和 Hermes 自身更新器不在 Connector 范围。

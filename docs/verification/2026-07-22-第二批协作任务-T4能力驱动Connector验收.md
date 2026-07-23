# 第二批协作任务：T4 能力驱动 Connector 验收

> 日期：2026-07-22
> 范围：#4 capability-driven Connector、Claude Desktop、Gemini CLI
> 结论：本地实现与自动化验收通过；macOS/Windows 真实 Claude Desktop 冒烟留待最终总验收

## 1. 交付范围

- `build.rs` 扫描 Connector 模块并生成 `BUILTIN_CONNECTORS`；命令层通过 Registry 查找 Connector，不再维护手写分发 `match`。
- 每个 Connector 自声明 `connector_id`、`agent_id`、平台、配置格式/路径、受管字段、入站适配器、URL 形态、虚拟 key 和重启要求。
- Desktop Agent 默认集合、Registry 校验、前端顺序与导航标记均从描述和能力数据派生；删除前端 Agent ID 顺序/标记常量。
- CLI scoped namespace 由已加载插件 manifest 的 `agent_tools` 与显式 `agent_routes` 联合准入：未来 Agent 无需核心枚举，未知命名空间仍返回 404。
- 新增 Gemini CLI Connector、lossless dotenv codec 和 `google-gemini-generate-content` WASM 入站适配器。
- 新增 Claude Desktop 3P Connector，以及 profile 与 `_meta.json` 同计划提交、快照、ownership、恢复、断开和失败回滚的多文件事务。

## 2. 契约与安全边界

### Gemini CLI

- 唯一受管文件为 `~/.gemini/.env`。
- 仅拥有 `GOOGLE_GEMINI_BASE_URL` 和 `GEMINI_API_KEY`；注释、顺序、原始换行和未知键保持不变。
- 写入的是 Token Station 本地虚拟 key，不写上游真实凭据。
- Gemini 请求从 `/v1beta/models/<model>:generateContent|streamGenerateContent` 解析模型和流式模式，经真实代理返回 Gemini `candidates`/SSE 形态。

### Claude Desktop

- macOS 仅写 `~/Library/Application Support/Claude-3p/configLibrary/`；Windows 仅写 `%LOCALAPPDATA%/Claude-3p/configLibrary/`。
- 固定 profile ID 为 `7f60d1f4-8d8c-4f5c-9f4c-2c2530c4f9f2`；写入官方 gateway 字段，并同步维护 `_meta.json` 的 `appliedId`/`entries`。
- profile 与 `_meta.json` 分别绑定 revision、加密快照和 ownership；任一写后复验失败时逆序恢复两份文件。
- Linux 返回 typed `unsupported_platform`，且配置、快照、ownership 均零写入；绝不写 `~/.claude/settings.json` 冒充 Claude Desktop。

采用契约来自：

- [Gemini CLI configuration](https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md)
- [Gemini CLI content generator](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/core/contentGenerator.ts)
- [Claude Desktop third-party configuration](https://claude.com/docs/third-party/claude-desktop/configuration)
- [Claude Desktop in-app configuration](https://claude.com/docs/third-party/claude-desktop/in-app-configuration)
- [Claude Desktop data storage](https://claude.com/docs/third-party/claude-desktop/data-storage)

## 3. 关键行为证据

- Connector conformance 验证生成表 ID 唯一、描述与真实 capability 一致，且每种格式都能生成并解析计划。
- `dotenv_round_trip_changes_only_owned_keys_and_restores_them` 验证 Gemini 仅修改受管键并精确恢复；重复键 fail-closed 且错误不回显值。
- `claude_desktop_profile_and_meta_commit_together_and_recover_together` 覆盖 connect、未知字段保留、双文件 ownership、snapshot restore、disconnect 和强制写后失败回滚。
- `openclaw_transaction_connect_restore_disconnect_preserves_json5_comments` 与 `hermes_transaction_connect_restore_disconnect_preserves_yaml_comments` 验证既有 JSON5/YAML Connector 的未知字段和注释保持。
- `registry_driven_future_agent_route_ids_need_no_cli_enum_change` 验证未来 lower-kebab Agent ID 无需 CLI 枚举。
- `scoped_models_auth_and_unknown_namespaces_fail_closed` 验证未声明、未配置的合法名称不会继承默认路由。
- `all_four_inbound_protocols_coexist_in_declared_match_order` 通过真实代理同时覆盖 OpenAI Chat Completions、Responses、Anthropic Messages 和 Gemini GenerateContent。

## 4. Fresh verification

在当前工作树执行：

```text
Desktop Rust tests
结果：lib 157 passed, 1 ignored；集成测试与 doc tests 通过

cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
结果：通过，0 warning

npm test -- --run
结果：11 files passed，107 tests passed

npx tsc --noEmit
结果：通过

cargo test -p token-station-cli
结果：lib 122 passed, 1 ignored；main 1 passed；plugin devchain 1 passed；plugin install 4 passed；proxy 50 passed, 1 ignored；upgrade 4 passed

cargo test -p token-station-conformance --test official_plugins
结果：5 passed

cargo test -p token-station-plugin-runtime --test official_plugins
结果：16 passed；包含 Gemini/Anthropic/OpenAI/Responses 的真实 WASM conformance

cargo clippy --workspace --all-targets -- -D warnings
结果：通过，0 warning

cargo clippy --manifest-path plugins/official/agent-gemini/Cargo.toml --target wasm32-wasip2 -- -D warnings
结果：通过，0 warning

cargo fmt --all -- --check
结果：通过
```

## 5. 通过条件对照

| 条件 | 结果 |
|---|---|
| 新增 Agent 不修改上层核心枚举、命令分支或前端 ID 分支 | 通过：模块自动注册，控制面由 capability/descriptor 派生 |
| Claude Desktop / Gemini CLI 通过 Connector 接入 | 通过 |
| Claude Desktop 仅使用 3P profile，Linux 明确不支持 | 通过 |
| dotenv / JSON5 / YAML 保留未知字段并可恢复 | 通过 |
| Claude Desktop 多文件事务可回滚、可恢复 | 通过 |
| 未声明 Agent namespace fail-closed | 通过 |
| 通用自动化门禁无失败、无 warning | 通过 |

## 6. 最终总验收保留项

自动化验收无阻塞缺口。最终总验收需在真实 macOS/Windows Claude Desktop 与真实 Gemini CLI 上各执行一次 connect → 请求 → restore/disconnect 冒烟，并保留界面与磁盘证据；该项不阻塞进入 #6。

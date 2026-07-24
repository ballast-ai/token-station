# Codex 鉴权与 TOML 表恢复修复验收

日期：2026-07-24
状态：部分通过；配置接入与恢复代码已覆盖，真实 Codex 客户端链路仍有阻塞项

## 已验证

1. Codex 接入计划把本地虚拟 Key 写入
   `model_providers.tokenstation.experimental_bearer_token`；新建 provider 不再只依赖
   GUI 客户端无法获得的 `TOKENSTATION_KEY` 环境变量。
2. 该 Key 被标记为敏感字段，计划展示与日志不会回显其值。
3. TOML 反向补丁可以将仅含字符串、布尔值、整数和嵌套对象的 JSON
   对象恢复为 TOML 表。
4. 首次接入时新建的 `model_providers.tokenstation` 直接父表会被记录为可撤销路径。

## 自动化证据

```text
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib -- --test-threads=1
rustfmt --edition 2021 --check <本次四个 Rust 文件>
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
git diff --check
```

合入前以 PR 当次 CI 和隔离 worktree 的命令输出为准，不把旧工作区中的历史测试结果当作本次验收证据。
全量 `cargo fmt --check` 仍被 `origin/develop` 基线中已有的 `commands.rs`
格式差异阻塞，本 PR 没有顺带格式化该无关文件。

## 2026-07-24 运行验证补充

桌面 App 更新后，使用手工构造的 OpenAI Responses 请求验证
`/agents/codex/v1/responses`：

1. 最小文本请求：HTTP 200，1858 ms。
2. 公开 PNG 图片 URL 请求：HTTP 200，2473 ms，路由特征 `has_images=true`。

这只能证明网关基本文本/图片链路可用，不能替代真实 Codex 客户端验收。

## 真实 Codex 0.146.0 阻塞项

1. macOS 系统代理启用且指向本地代理端口时，Codex 未按系统
   `ExceptionsList` 绕过 `127.0.0.1`，请求在到达 Token Station 前返回 502；
   显式设置 `NO_PROXY=127.0.0.1,localhost` 后才能进入网关。
2. 进入网关后，`agent-openai-responses` 仍拒绝 Codex 请求中的
   `parallel_tool_calls=false`，返回 `unsupported_capability`，因此真实消息和图片均不能完成。
3. Codex 启动时会访问带 Agent 命名空间的
   `/agents/codex/v1/models?client_version=...`；当前链路返回 502，模型目录端点需要补齐或明确降级。

## PR 深度审查阻塞项

以下问题在 Draft PR 合入前必须由负责人处理：

1. `experimental_bearer_token` 会写入现有 Codex 配置，但事务层保留原文件权限。
   如果原文件为 `0644` 或 `0640`，同机其他账号或同组账号可能读取本地虚拟 Key。
   写入凭据时应收紧为 `0600`，并明确断开时是否恢复原权限；Windows 需要等价 ACL。
2. 当前 JSON→TOML 递归转换仍拒绝有效 TOML 数组和浮点数；TOML datetime
   经过 JSON 语义层还会丢失类型身份。完整 provider 表包含这些值时，恢复计划仍可能失败
   或恢复成错误类型。正确修复应保留 `toml_edit::Item`，或引入覆盖全部 TOML 类型的带标签表示。
3. 反向补丁目前只删除本次创建的最深父路径。若
   `model_providers` 和 `model_providers.tokenstation` 都不存在，恢复后仍可能留下空的
   `model_providers`；同时生成的父级 `Remove` 可能超出 Connector 声明的叶子 owned path。
   需要在 planner 的 ownership 边界内计算、校验所有结构清理操作。
4. 若接入前 `~/.codex/config.toml` 根本不存在，断开计划仍把投影结果作为“存在的文件”
   交给事务层写回，不能恢复到“文件不存在”的基线。需要把目标存在性纳入恢复计划，
   并补“首次接入 → 断开 → 再接入”的回归测试。
5. 历史快照现在可能包含旧的 `experimental_bearer_token`。虚拟 Key 轮换后恢复旧快照，
   会复活已失效 Key，同时留下活跃 ownership，导致 Codex 失去鉴权且普通重连被拒。
   敏感路径应在恢复时绑定当前运行时凭据，或把快照绑定到有效 ownership 代际。
6. 从旧版 Connector 升级时，既有 provider 里的 `env_key = "TOKENSTATION_KEY"`
   不会被新 patch 删除。新旧鉴权字段同时存在时可能继续读取缺失的环境变量或旧值。
   重连 patch 应显式移除 `env_key`，并在 `validate_projected` 中断言它不存在。

因此本变更只应以 Draft PR 提交：配置恢复与 GUI 鉴权问题具备回归测试，
但不能宣称“Codex 已可正常接入”。负责人需要解决以上运行时兼容问题后，再完成：

1. 在 Token Station 中“恢复 Agent 原始配置”，确认恢复成功且非受管字段不变；
2. 重新“一键接入”并重启 Codex；
3. 用真实 Codex 分别发送文本和图片，确认不再出现 502。

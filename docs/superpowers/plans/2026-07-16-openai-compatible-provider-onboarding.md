# OpenAI-compatible 模型供应商通用接入实施计划

## 目标与边界

用 Qwen、Kimi、MiniMax、GLM 四个中国区标准 API 作为代表性测试矩阵，验证未来满足
OpenAI Chat Completions 契约的模型供应商可以只通过配置接入 token-station。四家不是
产品白名单，不新增厂商专属业务分支或四个 provider 插件。

实施默认复用 `agent-anthropic`、现有 router 和 `provider-openai-compatible`。不得修改
`crates/router-core`、`crates/protocol`、`crates/release` 或现有 DeepSeek 配方。只有
真实 E2E 产生可复现的通用方言缺陷时，才允许先补 provider fixture，再做厂商中立修复。

真实 API Key 不进入聊天、仓库、配置、日志或命令行参数；用户在本机通过标准输入将
Key 写入 macOS 钥匙串，测试完成后使用 `key remove` 删除。

## Task 1：四份单供应商配置

### 交付物

新增：

- `apps/cli/claude-code-qwen-config.json`
- `apps/cli/claude-code-kimi-config.json`
- `apps/cli/claude-code-minimax-config.json`
- `apps/cli/claude-code-glm-config.json`

每份配置只包含一个 upstream、一个最新稳定模型和一个 default pool；`rules` 与
`hint_routes` 为空。凭证来源统一使用 `{"slot":"provider_api_key","keyring":true}`。

模型和端点按执行日官方文档复核，首轮目标为：

- Qwen：`qwen3.7-max`；
- Kimi：`kimi-k2.7-code`；
- MiniMax：`MiniMax-M3`；
- GLM：`glm-5.2`。

能力声明保持保守：只声明官方文档明确支持且本轮需要的工具调用、上下文窗口和标准
采样参数；未验证的 vision / json_schema 不作为本轮路由前提。

### 验证与提交

```bash
./target/release/token-station-cli --config <config> upstream list
./target/release/token-station-cli --config <config> rule list
git diff --check
```

四份配置解析通过后创建一个本地配置提交，不 push。

## Task 2：通用接入指南

### 交付物

新增 `docs/guides/Claude-Code-OpenAI-Compatible-供应商接入指南.md`，内容包括：

- 通用 OpenAI-compatible 供应商判断清单；
- 四份测试配置的启动方式；
- 使用 `key set` 从标准输入写入钥匙串；
- 使用 `key remove` 删除测试凭证；
- Claude Code 侧继续使用 Anthropic 兼容模型标识；
- 普通对话、工具闭环、错误回传和 metrics 检查命令；
- thinking / reasoning 专有字段的支持边界；
- 新供应商是配置接入、通用 provider 修复还是独立 provider 插件的判定流程。

文档不得包含真实 Key、Key 前后缀、终端历史或用户级全局配置修改。

### 验证与提交

检查所有命令参数与 CLI 当前实现一致，扫描敏感信息和占位符；指南与验收骨架可合并为
一个本地文档提交，不 push。

## Task 3：自动化与无密钥验证

依次运行：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build --release -p token-station-cli
./target/release/token-station-cli plugin test plugins/official/agent-anthropic
./target/release/token-station-cli plugin test plugins/official/provider-openai-compatible
```

四份配置分别运行 `upstream list` 和 `rule list`。此阶段不调用真实上游，不消耗额度。

## Task 4：本机钥匙串录入

配置和 CLI 验证完成后，由用户在本机终端执行交互式录入。每次使用 shell 私有变量，
通过标准输入传给 CLI，并立即 `unset`。不得要求用户把 Key 发到聊天。

示例流程：

```zsh
read -s "KEY?Qwen API Key: "; print
print -rn -- "$KEY" | ./target/release/token-station-cli key set qwen provider_api_key
unset KEY
```

其他三家替换 upstream 名。录入后只验证钥匙串条目可被服务解析，不回显明文。

## Task 5：真实 upstream probe

每家先执行最小真实探活：

```bash
./target/release/token-station-cli --config <config> upstream test <upstream> --model <model>
```

记录状态、延迟和实际模型，不记录响应正文或 Key。若失败，按以下顺序判定：

1. 标准 API Key 与套餐 Key 端点是否匹配；
2. 模型是否在账户和地域可用；
3. Base URL 与 `/chat/completions` 拼接是否正确；
4. Bearer 鉴权、请求参数或错误格式是否存在方言差异。

未通过 probe 的供应商不进入 Claude Code E2E，也不得标为兼容。

## Task 6：每家 Claude Code 真实 E2E

对四份配置逐一启动独立 token-station 数据目录，避免指标和凭证证据串线。Claude Code
使用 `--setting-sources project` 隔离用户全局 settings，并继续使用不会自动附加
unsupported adaptive thinking 的 Anthropic 兼容模型标识。

每家完成：

1. 普通流式文本，输出固定可验证标记；
2. `Read` 工具读取仓库内固定小文件；
3. 工具结果第二轮回传并输出可验证答案；
4. 本地错误 virtual key 返回 Anthropic 401，且 metrics 不增长；
5. 受控的上游鉴权错误映射，不让 Claude Code 无限重试；
6. metrics 中 upstream/model、stream、tool_count 和状态与被测配置一致；
7. 日志不含 prompt、response、API Key 或 virtual key。

任一家因专有 reasoning/thinking 字段无法完成工具闭环时，记录“未兼容”及最小证据，
提交 IR 或 provider 方言变更请求；未经评审不修改 IR，不伪造通过。

## Task 7：验收记录与凭证清理

新增 `docs/verification/2026-07-16-Claude-Code-OpenAI-Compatible-Providers-E2E.md`，按供应商
分别记录：配置、模型、命令、成功/失败、metrics 证据、边界和未验证项。

验收后由用户执行：

```bash
./target/release/token-station-cli key remove qwen provider_api_key
./target/release/token-station-cli key remove kimi provider_api_key
./target/release/token-station-cli key remove minimax provider_api_key
./target/release/token-station-cli key remove glm provider_api_key
```

删除后再次确认四个钥匙串条目不可解析。运行数据目录保留到验收文档完成，随后仅删除
明确属于本轮测试的临时目录，不触碰用户其他 token-station 数据。

## Task 8：最终红线审计

检查：

- `crates/router-core`、`crates/protocol`、`crates/release` 无 diff；
- Gateway 和 `agent-anthropic` 无厂商名分支；
- 四家仅出现在配置、指南和验收证据；
- 没有真实凭证、virtual key、运行日志或构建产物进入 Git；
- workspace 自动化和两套 plugin conformance 通过；
- 本地提交粒度清晰，工作区干净，未 push。

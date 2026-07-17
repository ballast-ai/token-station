# Codex 经 token-station 接入 DeepSeek

Codex 使用 OpenAI Responses API。token-station 在 `POST /v1/responses` 接收请求，
由 `agent-openai-responses` 转为 Canonical IR，再由
`provider-openai-compatible` 调用 DeepSeek Chat Completions。

```text
Codex -> /v1/responses -> agent-openai-responses -> Canonical IR
      -> router -> provider-openai-compatible -> DeepSeek
```

样例配置：
[apps/cli/codex-deepseek-config.json](../../apps/cli/codex-deepseek-config.json)。
它只监听 `127.0.0.1:8791`，运行数据写入
`token-station-m4/codex/data`，不会接触现有的 8787 实例。

## 1. 构建和安装插件

先在仓库根目录构建 CLI 和本 Agent 所需的两个插件：

```bash
cargo build --release -p token-station-cli

./target/release/token-station-cli plugin build \
  plugins/official/agent-openai-responses
./target/release/token-station-cli plugin test \
  plugins/official/agent-openai-responses
./target/release/token-station-cli plugin build \
  plugins/official/provider-openai-compatible
./target/release/token-station-cli plugin test \
  plugins/official/provider-openai-compatible

./target/release/token-station-cli \
  --config apps/cli/codex-deepseek-config.json \
  plugin install plugins/official/agent-openai-responses
./target/release/token-station-cli \
  --config apps/cli/codex-deepseek-config.json \
  plugin install plugins/official/provider-openai-compatible
```

三个 M4 配置共用 `token-station-m4/plugins`。插件只需安装一次；若同名包已存在，
先用 `plugin info` 核对，不要手工覆盖 WASM 和验收回执。

## 2. 启动隔离代理

只在 token-station 服务终端注入上游凭证，不要把值写进 JSON 或日志：

```bash
export DEEPSEEK_API_KEY='你的 DeepSeek API Key'

./target/release/token-station-cli \
  --config apps/cli/codex-deepseek-config.json upstream list
./target/release/token-station-cli \
  --config apps/cli/codex-deepseek-config.json rule list
./target/release/token-station-cli \
  --config apps/cli/codex-deepseek-config.json serve
```

本地 virtual key 位于 `token-station-m4/codex/data/virtual-key`，只负责 Codex 到
本地代理的鉴权，不是 DeepSeek Key。

## 3. 用临时 CODEX_HOME 运行

以下配置只写入 `/tmp`，不会读取或修改用户的 `~/.codex`：

```bash
export TS_VIRTUAL_KEY="$(tr -d '\r\n' < token-station-m4/codex/data/virtual-key)"
export CODEX_HOME=/tmp/token-station-m4-codex-home
mkdir -p "$CODEX_HOME"

cat > "$CODEX_HOME/config.toml" <<'TOML'
model = "auto"
model_provider = "token-station"
web_search = "disabled"

[features]
apps = false
browser_use = false
computer_use = false
goals = false
image_generation = false
in_app_browser = false
multi_agent = false
plugins = false
workspace_dependencies = false

[model_providers.token-station]
name = "Token Station"
base_url = "http://127.0.0.1:8791/v1"
env_key = "TS_VIRTUAL_KEY"
wire_api = "responses"
requires_openai_auth = false
request_max_retries = 0
stream_max_retries = 0
TOML
```

这些开关只作用于本轮临时 Codex 进程。当前适配器只承载本地 function tool，
因此关闭 Codex 的 hosted tool、namespace tool 和个人插件注入；否则适配器会对
无法无损映射的 `namespace` / `web_search` 工具明确返回能力错误。

如果要严格遵守“仓库文件只能由补丁修改”的开发约束，可手工创建这份临时文件；
它不是项目交付物，也不应提交。

普通流式验收：

```bash
codex exec --ephemeral --skip-git-repo-check --sandbox read-only \
  -C /tmp/token-station-m4-codex-work \
  '只回答固定标记：CODEX_M4_OK'
```

工具闭环验收可在临时工作目录放置 `marker.txt`，再要求 Codex 必须调用本地读取工具
读取文件并原样回答。验收证据应同时包含最终标记，以及 metrics 中至少一轮
`tool_count > 0`。

## 4. 错误链和审计

本地鉴权错误不应进入 router 或上游：

```bash
TS_VIRTUAL_KEY='intentionally-wrong' \
codex exec --ephemeral --skip-git-repo-check --sandbox read-only \
  -C /tmp/token-station-m4-codex-work '回答 AUTH_TEST'
```

命令应失败并收到 Responses 形状的 401。受控上游错误应停止 8791 服务，再以
`DEEPSEEK_API_KEY='intentionally-wrong'` 重启同一隔离配置后执行一次请求；不要对
正在使用的 8787 实例做此测试。

停止服务并刷盘后检查：

```bash
./target/release/token-station-cli \
  --config apps/cli/codex-deepseek-config.json stats --since all
rg -n 'CODEX_M4_OK|intentionally-wrong|sk-' \
  token-station-m4/codex/data/requests.log || true
```

日志不应包含 prompt、response、上游 Key 或 virtual key。测试结束可删除本轮创建的
`/tmp/token-station-m4-codex-*`；是否保留 metrics 由验收取证需求决定。

## 5. 当前边界

- 已覆盖文本输入、消息输入、图片 URL、function tools、function call/output、
  Responses SSE、usage 和 Responses 错误。
- reasoning item、computer/hosted tool、file-id 图片及 Responses 完整事件全集尚未
  支持；遇到这些输入会明确报能力错误，不会静默丢字段。
- Responses 结构化输出暂不支持；`text.format.type` 为 `json_schema` 或
  `json_object` 时返回 `unsupported_capability`，不会进入 router 或上游。
- 本配方只证明 Codex 的文本与本地 function tool 主链，不等于“完整兼容所有
  Responses 能力”。

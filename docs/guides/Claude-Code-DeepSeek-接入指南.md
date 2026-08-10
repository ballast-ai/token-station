# Claude Code 经 token-station 接入 DeepSeek

这份配方把两侧协议分开配置：Claude Code 仍发送 Anthropic Messages，
`agent-anthropic` 将其归一为 Canonical IR；路由后由现有
`provider-openai-compatible` 调用 DeepSeek。DeepSeek 只出现在配置中，Rust
网关、路由和入站适配器都没有 DeepSeek 专属分支。

```text
Claude Code
  -> POST /v1/messages
  -> agent-anthropic
  -> Canonical IR -> router
  -> provider-openai-compatible
  -> POST https://api.deepseek.com/chat/completions
```

样例配置：
[apps/cli/claude-code-deepseek-config.json](../../apps/cli/claude-code-deepseek-config.json)。
配置文件只引用 `DEEPSEEK_API_KEY` 环境变量，不包含 Key 值。

## 1. 构建并安装两个协议插件

在仓库根目录执行：

```bash
cargo build --release -p token-station-cli

./target/release/token-station-cli plugin build \
  plugins/official/agent-anthropic
./target/release/token-station-cli plugin build \
  plugins/official/provider-openai-compatible

./target/release/token-station-cli \
  --config apps/cli/claude-code-deepseek-config.json \
  plugin install plugins/official/agent-anthropic
./target/release/token-station-cli \
  --config apps/cli/claude-code-deepseek-config.json \
  plugin install plugins/official/provider-openai-compatible
```

`plugin install` 会执行与开发阶段相同的一致性套件，并把通过验收的包复制到
`token-station-e2e/plugins/`。该目录、运行数据和本地 virtual key 已被
`.gitignore` 排除。

如果包已安装，命令会拒绝覆盖。开发时更新插件应先用 `plugin remove <name>`
删除旧包，再重新安装；不要手工覆盖已生成验收回执的 WASM。

## 2. 配置 DeepSeek Key

只在启动 token-station 的终端中设置环境变量：

```bash
export DEEPSEEK_API_KEY='你的 DeepSeek API Key'
```

不要把 Key 写进 JSON、命令参数或仓库文件。也可以把样例配置的 `auth` 改成
`{"slot":"provider_api_key","store":true}`，再通过标准输入写入数据目录下
受私有权限保护的明文 `secrets.json`：

```bash
printf '%s' "$DEEPSEEK_API_KEY" | \
  ./target/release/token-station-cli key set deepseek provider_api_key
```

## 3. 检查配置并启动代理

以下只读命令会解析配置并展示上游与路由：

```bash
./target/release/token-station-cli \
  --config apps/cli/claude-code-deepseek-config.json upstream list
./target/release/token-station-cli \
  --config apps/cli/claude-code-deepseek-config.json rule list
```

启动代理：

```bash
./target/release/token-station-cli \
  --config apps/cli/claude-code-deepseek-config.json serve
```

首次启动会在终端显示一次本地 virtual key，随后只提示文件位置：
`token-station-e2e/data/virtual-key`。该 key 只用于 Claude Code 到本地代理的
鉴权，不是 DeepSeek Key。

## 4. 启动 Claude Code

另开一个终端，在仓库根目录读取本地 virtual key：

```bash
export TS_VIRTUAL_KEY="$(tr -d '\r\n' < token-station-e2e/data/virtual-key)"
```

然后仅为这次 Claude Code 进程注入配置：

```bash
ANTHROPIC_BASE_URL='http://127.0.0.1:8787' \
ANTHROPIC_AUTH_TOKEN="$TS_VIRTUAL_KEY" \
ANTHROPIC_MODEL='claude-3-5-haiku-20241022' \
MAX_THINKING_TOKENS=0 \
CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING=1 \
CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1 \
CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
claude --model claude-3-5-haiku-20241022 \
  --safe-mode \
  --setting-sources project
```

`ANTHROPIC_AUTH_TOKEN` 会作为 Bearer token 发给本地代理。不要把
`DEEPSEEK_API_KEY` 传给 Claude Code；它只应存在于 token-station 服务进程。

这里的 `claude-3-5-haiku-20241022` 是 Claude Code 侧的协议兼容标识，不是实际
上游。token-station 的路由仍会选择配置中的
`deepseek/deepseek-v4-flash`。当前 Claude Code 会把不认识的网关模型名当作新
Claude 模型并自动附加 `thinking: {"type":"adaptive"}`；而 Canonical IR 尚不能
无损承载该字段，所以不能直接把 Claude Code 的 `--model` 设为
`deepseek-v4-flash`。

`--setting-sources project` 用于排除用户级 `~/.claude/settings.json` 中可能已有的
`env.ANTHROPIC_BASE_URL` / `env.ANTHROPIC_AUTH_TOKEN`。这些 settings 环境项可能
覆盖当前 shell 的同名变量。若项目级 settings 也配置了这些变量，应先移除冲突，
或在一个没有此类配置的目录完成验收。`--safe-mode` 让首次连通测试不加载个人
插件、hooks 和 MCP；确认连通后可按需去掉。

## 5. 当前兼容边界

- 已实现普通文本、system、图片块、工具定义、`tool_use` / `tool_result`、
  Anthropic SSE、usage 和 Anthropic 错误格式。
- Canonical IR 当前不能无损表达 `thinking` / `redacted_thinking`。适配器会明确
  返回能力错误，不会静默丢弃。因此本配方关闭实验 beta，并使用不会触发
  adaptive thinking 的 Claude Code 兼容模型标识。
- `/v1/messages/count_tokens` 尚未实现。Claude Code 会退回本地 token 估算；
  该端点在 Claude Code 网关协议中本来就是可选项。
- 当前 DeepSeek OpenAI 格式公开模型为 `deepseek-v4-flash` 和
  `deepseek-v4-pro`。样例使用 flash；切换 pro 只需更新配置中的 upstream model
  和 router pool，Claude Code 侧继续使用上述兼容标识，不需要改 Rust 代码。
- Claude Code 自己的 `modelUsage` 会按兼容标识展示上下文和价格，不能作为实际
  DeepSeek 计费依据；真实 upstream/model/token 用量以 token-station 的
  `requests.log`、metrics 和 DeepSeek 账单为准。
- 图片能进入 Canonical IR，但样例没有为 DeepSeek 声明 `vision: true`，因此
  路由不会把需要视觉能力的请求发给该上游。

## 6. 扩展到其他模型厂商

保持 `plugins.agent = "agent-anthropic"`，即可继续接收 Claude Code 的 Anthropic
Messages。出站侧按厂商协议配置：

- OpenAI-compatible 厂商：新增 `upstreams` 条目并继续复用
  `provider-openai-compatible`。
- 非 OpenAI-compatible 厂商：新增独立 `provider-*` 插件，并在
  `plugins.providers` 中把 provider 方言映射到该插件。
- 在 `router.pools` 中组合不同厂商和模型，规则只引用逻辑池与能力，不在
  `agent-anthropic` 中判断厂商名。

这保证“调用者协议”“路由决策”“模型厂商协议”三层可独立扩展。

## 7. 官方协议依据

- [DeepSeek 首次 API 调用](https://api-docs.deepseek.com/)：OpenAI 格式 base URL、
  当前模型名和鉴权方式。
- [DeepSeek Chat Completions](https://api-docs.deepseek.com/api/create-chat-completion)：
  `/chat/completions`、SSE 与工具调用字段。
- [DeepSeek 模型与能力](https://api-docs.deepseek.com/quick_start/pricing)：上下文长度
  和工具/JSON 能力。
- [Claude Code LLM Gateway 协议](https://code.claude.com/docs/en/llm-gateway-protocol)：
  `/v1/messages`、认证头、可选 token counting 与 beta/thinking 行为。

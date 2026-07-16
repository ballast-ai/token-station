# OpenClaw 经 token-station 接入 DeepSeek

OpenClaw 的自定义模型 provider 使用 `openai-completions`。token-station 在
`POST /v1/chat/completions` 接收请求，经 `agent-openai` 和 Canonical IR 路由到
DeepSeek。样例配置：
[apps/cli/openclaw-deepseek-config.json](../../apps/cli/openclaw-deepseek-config.json)。

该实例监听 `127.0.0.1:8793`，数据写入
`token-station-m4/openclaw/data`，不会读取现有 OpenClaw 状态。

## 1. 插件和隔离服务

本配方与 OpenCode 共用 `agent-openai` 和
`provider-openai-compatible`。按 OpenCode 指南安装一次即可。随后在独立终端运行：

```bash
export DEEPSEEK_API_KEY='你的 DeepSeek API Key'
./target/release/token-station-cli \
  --config apps/cli/openclaw-deepseek-config.json upstream list
./target/release/token-station-cli \
  --config apps/cli/openclaw-deepseek-config.json rule list
./target/release/token-station-cli \
  --config apps/cli/openclaw-deepseek-config.json serve
```

## 2. 临时 OpenClaw 状态和配置

同时覆盖 HOME、配置路径和状态目录，避免 OpenClaw 的旧状态迁移访问用户
`~/.openclaw`：

```bash
export TS_VIRTUAL_KEY="$(tr -d '\r\n' < token-station-m4/openclaw/data/virtual-key)"
export HOME=/tmp/token-station-m4-openclaw-home
export OPENCLAW_STATE_DIR=/tmp/token-station-m4-openclaw-state
export OPENCLAW_CONFIG_PATH=/tmp/token-station-m4-openclaw-config.json5
mkdir -p "$HOME" "$OPENCLAW_STATE_DIR" /tmp/token-station-m4-openclaw-work
```

在 `$OPENCLAW_CONFIG_PATH` 写入以下 JSON5；`${TS_VIRTUAL_KEY}` 由 OpenClaw 在
运行时从环境变量解析：

```json5
{
  models: {
    mode: "replace",
    providers: {
      "token-station": {
        baseUrl: "http://127.0.0.1:8793/v1",
        apiKey: "${TS_VIRTUAL_KEY}",
        api: "openai-completions",
        models: [{
          id: "auto",
          name: "Token Station routed model",
          reasoning: false,
          input: ["text"],
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
          contextWindow: 1000000,
          maxTokens: 8192
        }]
      }
    }
  },
  agents: {
    defaults: {
      model: { primary: "token-station/auto" },
      workspace: "/tmp/token-station-m4-openclaw-work"
    }
  }
}
```

普通流式验收：

```bash
openclaw agent --local --agent main --model token-station/auto \
  --message '只回答固定标记：OPENCLAW_M4_OK' --json
```

工具闭环验收时，在临时 workspace 准备 `marker.txt`，要求 Agent 必须用读取工具
读取并原样回答。不要给它修改仓库或用户目录的任务。

## 3. 错误链、审计与清理

本地鉴权错误：

```bash
TS_VIRTUAL_KEY='intentionally-wrong' \
openclaw agent --local --agent main --model token-station/auto \
  --message '回答 AUTH_TEST' --json
```

该请求应得到 OpenAI 形状的 401，且不进入上游。受控上游错误只重启 8793 隔离
实例并注入错误 `DEEPSEEK_API_KEY`，发出一次请求后确认没有无限重试。

```bash
./target/release/token-station-cli \
  --config apps/cli/openclaw-deepseek-config.json stats --since all
rg -n 'OPENCLAW_M4_OK|intentionally-wrong|sk-' \
  token-station-m4/openclaw/data/requests.log || true
```

日志和 metrics 不应含 prompt、response 或凭证。完成后停止本轮服务并删除：

```bash
rm -rf /tmp/token-station-m4-openclaw-state \
  /tmp/token-station-m4-openclaw-home \
  /tmp/token-station-m4-openclaw-work \
  /tmp/token-station-m4-openclaw-config.json5
```

这些路径全部属于本配方；不要删除用户的 `~/.openclaw`。

## 4. 当前边界

- 本配方覆盖 OpenAI Chat Completions 文本流和本地 function tool 主链。
- OpenClaw gateway、远程 channel、MCP、浏览器和用户级 skills 不在验收范围。
- `reasoning: false` 是模型声明，不代表 Responses reasoning 已实现；该 Agent 走的
  是 `agent-openai`，不是 `agent-openai-responses`。

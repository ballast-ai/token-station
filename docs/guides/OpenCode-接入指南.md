# OpenCode 经 token-station 接入 DeepSeek

OpenCode 使用 OpenAI-compatible Chat Completions。token-station 在
`POST /v1/chat/completions` 接收请求，由 `agent-openai` 归一化，再通过
`provider-openai-compatible` 调用 DeepSeek。

样例配置：
[apps/cli/opencode-deepseek-config.json](../../apps/cli/opencode-deepseek-config.json)。
它监听 `127.0.0.1:8792`，数据写入 `token-station-m4/opencode/data`。

## 1. 构建、安装并启动

如果已按 Codex 指南安装 provider，只需补装 `agent-openai`：

```bash
cargo build --release -p token-station-cli
./target/release/token-station-cli plugin build plugins/official/agent-openai
./target/release/token-station-cli plugin test plugins/official/agent-openai
./target/release/token-station-cli \
  --config apps/cli/opencode-deepseek-config.json \
  plugin install plugins/official/agent-openai
```

若尚未安装 `provider-openai-compatible`，也对它执行相同的
`plugin build`、`plugin test` 和 `plugin install`。三个 M4 配置共用插件目录，
同名包不要重复覆盖。

在独立服务终端启动：

```bash
export DEEPSEEK_API_KEY='你的 DeepSeek API Key'
./target/release/token-station-cli \
  --config apps/cli/opencode-deepseek-config.json upstream list
./target/release/token-station-cli \
  --config apps/cli/opencode-deepseek-config.json rule list
./target/release/token-station-cli \
  --config apps/cli/opencode-deepseek-config.json serve
```

## 2. 临时安装和进程级配置

不要全局安装或写 `~/.config/opencode`。固定验收版本并安装到 `/tmp`：

```bash
npm install --prefix /tmp/token-station-m4-opencode opencode-ai@1.18.2
export OPENCODE_BIN=/tmp/token-station-m4-opencode/node_modules/.bin/opencode
export TS_VIRTUAL_KEY="$(tr -d '\r\n' < token-station-m4/opencode/data/virtual-key)"
```

用最高优先级的进程级配置注入自定义 provider：

```bash
export OPENCODE_CONFIG_CONTENT='{
  "$schema": "https://opencode.ai/config.json",
  "model": "token-station/auto",
  "provider": {
    "token-station": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "Token Station",
      "options": {
        "baseURL": "http://127.0.0.1:8792/v1",
        "apiKey": "{env:TS_VIRTUAL_KEY}"
      },
      "models": {
        "auto": {
          "name": "Token Station routed model",
          "tool_call": true
        }
      }
    }
  },
  "permission": {
    "*": "ask",
    "bash": "allow",
    "edit": "deny"
  }
}'
export OPENCODE_DISABLE_AUTOUPDATE=1
export OPENCODE_DISABLE_DEFAULT_PLUGINS=1
export OPENCODE_DISABLE_LSP_DOWNLOAD=1
```

普通流式验收：

```bash
mkdir -p /tmp/token-station-m4-opencode-work
cd /tmp/token-station-m4-opencode-work
"$OPENCODE_BIN" run --auto '只回答固定标记：OPENCODE_M4_OK'
```

工具闭环验收时，在该目录准备只读 `marker.txt`，要求 OpenCode 必须调用 shell
读取并原样回答。`edit` 明确为 deny，`--auto` 只自动批准 ask，不会覆盖 deny。

## 3. 错误链、审计与清理

将单次命令的 `TS_VIRTUAL_KEY` 改为 `intentionally-wrong`，应收到 OpenAI 形状的
本地 401，且 metrics 不新增上游请求。上游错误只在 8792 隔离服务上测试：停止
服务，用受控错误的 `DEEPSEEK_API_KEY` 重启并只发一次请求，确认 Agent 不无限重试。

```bash
TS_VIRTUAL_KEY='intentionally-wrong' \
"$OPENCODE_BIN" run --auto '回答 AUTH_TEST'

./target/release/token-station-cli \
  --config apps/cli/opencode-deepseek-config.json stats --since all
rg -n 'OPENCODE_M4_OK|intentionally-wrong|sk-' \
  token-station-m4/opencode/data/requests.log || true
```

日志不应包含 prompt、response 或任何凭证。完成后退出临时 shell，并删除
`/tmp/token-station-m4-opencode` 和 `/tmp/token-station-m4-opencode-work`。

## 4. 当前边界

- 已覆盖 Chat Completions 文本、流式增量、function tool、tool result、usage 和错误。
- OpenCode 的插件、MCP、LSP 和用户配置不在本配方验收范围，测试时均隔离或关闭。
- provider 配置使用 OpenCode 官方 `@ai-sdk/openai-compatible` 接口；不修改
  token-station 的路由或 provider 适配器。

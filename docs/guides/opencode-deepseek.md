# Connect OpenCode to DeepSeek through Token Station

OpenCode uses OpenAI-compatible Chat Completions. Token Station receives
requests at `POST /v1/chat/completions`. `agent-openai` normalizes them, and
`provider-openai-compatible` calls DeepSeek.

Sample configuration:
[apps/cli/opencode-deepseek-config.json](../../apps/cli/opencode-deepseek-config.json).
It listens on `127.0.0.1:8792` and writes data to
`token-station-m4/opencode/data`.

## 1. Build, install, and start

If you installed the provider with the Codex guide, install only
`agent-openai`:

```bash
cargo build --release -p token-station-cli
./target/release/token-station-cli plugin build plugins/official/agent-openai
./target/release/token-station-cli plugin test plugins/official/agent-openai
./target/release/token-station-cli \
  --config apps/cli/opencode-deepseek-config.json \
  plugin install plugins/official/agent-openai
```

If `provider-openai-compatible` is not installed, run the same `plugin build`,
`plugin test`, and `plugin install` commands for it. The three M4
configurations share one plugin directory. Do not overwrite a package with the
same name.

Start the service in a separate terminal:

```bash
export DEEPSEEK_API_KEY='your DeepSeek API Key'
./target/release/token-station-cli \
  --config apps/cli/opencode-deepseek-config.json upstream list
./target/release/token-station-cli \
  --config apps/cli/opencode-deepseek-config.json rule list
./target/release/token-station-cli \
  --config apps/cli/opencode-deepseek-config.json serve
```

## 2. Temporary installation and process configuration

Do not install globally or write to `~/.config/opencode`. Pin the accepted
version and install it under `/tmp`:

```bash
npm install --prefix /tmp/token-station-m4-opencode opencode-ai@1.18.2
export OPENCODE_BIN=/tmp/token-station-m4-opencode/node_modules/.bin/opencode
export TS_VIRTUAL_KEY="$(tr -d '\r\n' < token-station-m4/opencode/data/virtual-key)"
```

Use the highest-priority process configuration to add the custom provider:

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

Run the standard streaming acceptance test:

```bash
mkdir -p /tmp/token-station-m4-opencode-work
cd /tmp/token-station-m4-opencode-work
"$OPENCODE_BIN" run --auto 'Reply only with this exact marker: OPENCODE_M4_OK'
```

For the tool-loop test, put a read-only `marker.txt` in this directory.
Require OpenCode to read it with the shell and return its exact content. The
`edit` permission is `deny`. `--auto` approves only `ask`; it does not
override `deny`.

## 3. Error path, audit, and cleanup

Set `TS_VIRTUAL_KEY` to `intentionally-wrong` for one command. The command
must receive an OpenAI-shaped local 401. Metrics must not add an upstream
request. Test upstream errors only on the isolated service at port 8792. Stop
the service. Restart it with a controlled invalid `DEEPSEEK_API_KEY`. Send
one request and confirm that the Agent does not retry without a limit.

```bash
TS_VIRTUAL_KEY='intentionally-wrong' \
"$OPENCODE_BIN" run --auto 'Reply with AUTH_TEST'

./target/release/token-station-cli \
  --config apps/cli/opencode-deepseek-config.json stats --since all
rg -n 'OPENCODE_M4_OK|intentionally-wrong|sk-' \
  token-station-m4/opencode/data/requests.log || true
```

Logs must not contain prompts, responses, or credentials. Exit the temporary
shell when the test is complete. Delete
`/tmp/token-station-m4-opencode` and
`/tmp/token-station-m4-opencode-work`.

## 4. Current boundaries

- Coverage includes Chat Completions text, streaming deltas, function tools,
  tool results, usage, and errors.
- Structured output is not supported. If `response_format.type` is `json_schema`
  or `json_object`, the adapter returns a capability error before the router
  or upstream receives the request.
- OpenCode plugins, MCP, LSP, and user configuration are outside this procedure.
  Tests isolate or disable them.
- The provider configuration uses the official OpenCode
  `@ai-sdk/openai-compatible` interface. It does not change Token Station
  routing or the provider adapter.

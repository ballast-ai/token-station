# Connect OpenClaw to DeepSeek through Token Station

The OpenClaw custom model provider uses `openai-completions`. Token Station
receives requests at `POST /v1/chat/completions`. It routes them to DeepSeek
through `agent-openai` and the Canonical IR. See the sample configuration:
[apps/cli/openclaw-deepseek-config.json](../../apps/cli/openclaw-deepseek-config.json).

This instance listens on `127.0.0.1:8793`. It writes data to
`token-station-m4/openclaw/data`. It does not read existing OpenClaw state.

## 1. Plugins and isolated service

This procedure shares `agent-openai` and
`provider-openai-compatible` with OpenCode. Install them once as shown in the
OpenCode guide. Then use a separate terminal:

```bash
export DEEPSEEK_API_KEY='your DeepSeek API Key'
./target/release/token-station-cli \
  --config apps/cli/openclaw-deepseek-config.json upstream list
./target/release/token-station-cli \
  --config apps/cli/openclaw-deepseek-config.json rule list
./target/release/token-station-cli \
  --config apps/cli/openclaw-deepseek-config.json serve
```

## 2. Temporary OpenClaw state and configuration

Override HOME, the configuration path, and the state directory. This prevents
migration of old OpenClaw state from accessing `~/.openclaw`.

```bash
export TS_VIRTUAL_KEY="$(tr -d '\r\n' < token-station-m4/openclaw/data/virtual-key)"
export HOME=/tmp/token-station-m4-openclaw-home
export OPENCLAW_STATE_DIR=/tmp/token-station-m4-openclaw-state
export OPENCLAW_CONFIG_PATH=/tmp/token-station-m4-openclaw-config.json5
mkdir -p "$HOME" "$OPENCLAW_STATE_DIR" /tmp/token-station-m4-openclaw-work
```

Write this JSON5 to `$OPENCLAW_CONFIG_PATH`. OpenClaw resolves
`${TS_VIRTUAL_KEY}` from the environment at runtime.

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

Run the standard streaming acceptance test:

```bash
openclaw agent --local --agent main --model token-station/auto \
  --message 'Reply only with this exact marker: OPENCLAW_M4_OK' --json
```

For the tool-loop test, put a read-only `marker.txt` in the temporary
workspace. Require the Agent to read it with the read tool and return its exact
content. Do not give the Agent a task that modifies the repository or a user
directory.

## 3. Error path, audit, and cleanup

Test a local authentication error:

```bash
TS_VIRTUAL_KEY='intentionally-wrong' \
openclaw agent --local --agent main --model token-station/auto \
  --message 'Reply with AUTH_TEST' --json
```

The request must receive an OpenAI-shaped 401 and must not reach the upstream.
To test a controlled upstream error, restart only the isolated instance on
port 8793 with an invalid `DEEPSEEK_API_KEY`. Send one request. Confirm that
retries are bounded.

```bash
./target/release/token-station-cli \
  --config apps/cli/openclaw-deepseek-config.json stats --since all
rg -n 'OPENCLAW_M4_OK|intentionally-wrong|sk-' \
  token-station-m4/openclaw/data/requests.log || true
```

Logs and metrics must not contain prompts, responses, or credentials. Stop the
service when the test is complete. Then delete only these paths:

```bash
rm -rf /tmp/token-station-m4-openclaw-state \
  /tmp/token-station-m4-openclaw-home \
  /tmp/token-station-m4-openclaw-work \
  /tmp/token-station-m4-openclaw-config.json5
```

All listed paths belong to this procedure. Do not delete the user's
`~/.openclaw`.

## 4. Current boundaries

- This procedure covers the OpenAI Chat Completions text stream and the local function-tool path.
- OpenClaw gateway, remote channels, MCP, browser access, and user-level skills are outside acceptance.
- `reasoning: false` is a model declaration. It does not mean that Responses
  reasoning is implemented. This Agent uses `agent-openai`, not
  `agent-openai-responses`.

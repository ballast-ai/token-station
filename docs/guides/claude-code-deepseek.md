# Connect Claude Code to DeepSeek through Token Station

This procedure configures the two protocols separately. Claude Code continues to send Anthropic Messages. `agent-anthropic` normalizes them into Canonical IR. After routing, the `provider-openai-compatible` South provider component calls DeepSeek. DeepSeek appears only in configuration. The Rust gateway, router, and inbound adapter have no DeepSeek-specific branches.

```text
Claude Code
  -> POST /v1/messages
  -> agent-anthropic
  -> Canonical IR -> router
  -> provider-openai-compatible
  -> POST https://api.deepseek.com/chat/completions
```

Use the sample configuration at [apps/cli/claude-code-deepseek-config.json](../../apps/cli/claude-code-deepseek-config.json). The configuration only references the `DEEPSEEK_API_KEY` environment variable. It does not contain the key value.

## 1. Stage the two protocol packages

Run these commands from the repository root:

```bash
cargo build --release -p token-station-cli
scripts/prepare-desktop-test-plugins.sh   # builds every official package into plugins-dist/

mkdir -p token-station-e2e/plugins
cp -R plugins-dist/agent-anthropic plugins-dist/provider-openai-compatible-v2 \
  token-station-e2e/plugins/
```

`plugins-dist/provider-openai-compatible-v2/` is the South provider component (`manifest.json` + `component.wasm`, built from `token-station-south`). The sample configuration vouches for it through `plugins.providers`, which is what lets a component without a local conformance receipt serve traffic; its conformance ran where it was built. Official release binaries embed every official package, so this step is only needed for a source build. `.gitignore` excludes the plugin directory, runtime data, and the local virtual key.

## 2. Configure the DeepSeek key

Set the environment variable only in the terminal that starts Token Station:

```bash
export DEEPSEEK_API_KEY='your DeepSeek API Key'
```

Do not put the key in JSON, command arguments, or repository files. You can also change `auth` in the sample configuration to `{"slot":"provider_api_key","store":true}`. Then write the key through standard input to the private plaintext `secrets.json` file in the data directory:

```bash
printf '%s' "$DEEPSEEK_API_KEY" | \
  ./target/release/token-station-cli key set deepseek provider_api_key
```

## 3. Check the configuration and start the proxy

These read-only commands parse the configuration and show the upstream and route:

```bash
./target/release/token-station-cli \
  --config apps/cli/claude-code-deepseek-config.json upstream list
./target/release/token-station-cli \
  --config apps/cli/claude-code-deepseek-config.json rule list
```

Start the proxy:

```bash
./target/release/token-station-cli \
  --config apps/cli/claude-code-deepseek-config.json serve
```

At first start, the terminal shows the local virtual key one time. Later starts show only its location: `token-station-e2e/data/virtual-key`. This key authenticates Claude Code to the local proxy. It is not the DeepSeek key.

## 4. Start Claude Code

Open another terminal. Read the local virtual key from the repository root:

```bash
export TS_VIRTUAL_KEY="$(tr -d '\r\n' < token-station-e2e/data/virtual-key)"
```

Inject this configuration only into the current Claude Code process:

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

`ANTHROPIC_AUTH_TOKEN` is sent to the local proxy as a Bearer token. Do not give `DEEPSEEK_API_KEY` to Claude Code. It must exist only in the Token Station service process.

`claude-3-5-haiku-20241022` is a protocol-compatible identifier for Claude Code. It is not the actual upstream. Token Station still selects `deepseek/deepseek-v4-flash` from the configuration. Claude Code treats an unknown gateway model name as a new Claude model and adds `thinking: {"type":"adaptive"}`. Canonical IR cannot preserve this field. Therefore, do not set Claude Code `--model` to `deepseek-v4-flash`.

`--setting-sources project` excludes values from the user-level `~/.claude/settings.json`. Existing `env.ANTHROPIC_BASE_URL` or `env.ANTHROPIC_AUTH_TOKEN` values in that file can override the same shell variables. Remove conflicts from project settings, or run the test in a directory without these settings. `--safe-mode` excludes personal plugins, hooks, and MCP during the first connection test. Remove it later if required.

## 5. Current compatibility limits

- Text, system messages, image blocks, tool definitions, `tool_use`, `tool_result`, Anthropic SSE, usage, and Anthropic errors are implemented.
- Canonical IR cannot preserve `thinking` or `redacted_thinking`. The adapter returns a capability error and does not silently drop these fields. This procedure disables experimental beta behavior and uses a Claude Code compatibility identifier that does not trigger adaptive thinking.
- `/v1/messages/count_tokens` is not implemented. Claude Code falls back to a local token estimate. This endpoint is optional in the Claude Code gateway protocol.
- The current public DeepSeek models in OpenAI format are `deepseek-v4-flash` and `deepseek-v4-pro`. The sample uses flash. To use pro, update the upstream model and router pool. Continue to use the compatibility identifier in Claude Code. No Rust change is required.
- Claude Code `modelUsage` shows context and pricing for the compatibility identifier. Do not use it as DeepSeek billing data. Use the Token Station `requests.log`, metrics, and DeepSeek bill for actual upstream, model, and token usage.
- Images enter Canonical IR, but the sample does not declare `vision: true` for DeepSeek. The router does not send vision requests to this upstream.

## 6. Add other model providers

Keep `plugins.agent = "agent-anthropic"` to continue receiving Anthropic Messages from Claude Code. Configure outbound traffic for the provider protocol:

- For an OpenAI-compatible provider, add an `upstreams` entry and continue to use `provider-openai-compatible`.
- For a provider that is not OpenAI-compatible, stage its South provider component (the Anthropic wire ships as `provider-anthropic-v2`) and map the provider dialect to it in `plugins.providers`.
- Combine providers and models in `router.pools`. Rules reference logical pools and capabilities. Do not test provider names in `agent-anthropic`.

This design lets the caller protocol, routing decision, and provider protocol change independently.

## 7. Official protocol references

- [DeepSeek First API Call](https://api-docs.deepseek.com/): OpenAI-format base URL, current model names, and authentication.
- [DeepSeek Chat Completions](https://api-docs.deepseek.com/api/create-chat-completion): `/chat/completions`, SSE, and tool-call fields.
- [DeepSeek Models and Capabilities](https://api-docs.deepseek.com/quick_start/pricing): context length, tools, and JSON capabilities.
- [Claude Code LLM Gateway Protocol](https://code.claude.com/docs/en/llm-gateway-protocol): `/v1/messages`, authentication headers, optional token counting, and beta or thinking behavior.

# Connect Claude Code to OpenAI-Compatible Model Providers

This guide explains how to use Claude Code with any provider that follows the OpenAI Chat Completions contract through Token Station. Qwen, Kimi, MiniMax, and GLM are initial general-compatibility test samples. They are not a product allowlist and do not require four provider-specific plugins.

```text
Claude Code
  -> Anthropic Messages
  -> agent-anthropic
  -> Canonical IR -> router
  -> provider-openai-compatible
  -> configured upstream / model
```

## 1. Determine whether direct configuration is sufficient

A provider must meet these minimum requirements:

- Accept `POST <base_url>/chat/completions`.
- Use a Bearer API key.
- Use OpenAI Chat Completions-compatible requests and responses.
- Use SSE `data:` frames and `[DONE]` for streaming responses.
- Use `tools`, `tool_calls`, and `role=tool` for tool calls.
- Report errors through HTTP status codes and optional `error.message` values.

If the provider meets these requirements, add only the upstream configuration, model capabilities, and credential. Do not modify Rust. Add a separate `provider-*` plugin only when the request, authentication, streaming, or tool protocol differs. If Canonical IR cannot express required semantics, submit a change request before you modify `crates/protocol`.

## 2. Initial and alternative test configurations

| Test sample | Configuration | Current test model |
|---|---|---|
| Qwen | [`claude-code-qwen-config.json`](../../apps/cli/claude-code-qwen-config.json) | `qwen3.7-max` |
| Kimi | [`claude-code-kimi-config.json`](../../apps/cli/claude-code-kimi-config.json) | `kimi-k2.7-code` |
| MiniMax | [`claude-code-minimax-config.json`](../../apps/cli/claude-code-minimax-config.json) | `MiniMax-M3` |
| GLM | [`claude-code-glm-config.json`](../../apps/cli/claude-code-glm-config.json) | `glm-5.2` |
| Kimi alternative | [`claude-code-kimi-moonshot-v1-config.json`](../../apps/cli/claude-code-kimi-moonshot-v1-config.json) | `moonshot-v1-128k` |
| MiniMax alternative | [`claude-code-minimax-m2.5-config.json`](../../apps/cli/claude-code-minimax-m2.5-config.json) | `MiniMax-M2.5` |

All six configurations contain one upstream and one default pool. `rules` and `hint_routes` are empty. They test provider protocol compatibility, not multi-model routing policy.

Alternative configurations use separate data directories to preserve metrics for the original and alternative models. Acceptance tests found these results:

- Kimi `moonshot-v1-128k` completes text and tool round trips. `kimi-k2.7-code` requires `reasoning_content`, so the second tool round fails with the current IR.
- MiniMax `MiniMax-M3` and `MiniMax-M2.5` both complete tool round trips. Through the OpenAI-compatible API, both put `<think>` in normal text. Changing models does not separate reasoning content.

Provider model aliases can change. Before an acceptance test, check the official model catalog again. Record the test date, requested model, and actual upstream model.

## 3. Build and install the common plugins

Run these commands from the repository root:

```bash
cargo build --release -p token-station-cli

./target/release/token-station-cli plugin build \
  plugins/official/agent-anthropic
./target/release/token-station-cli plugin build \
  plugins/official/provider-openai-compatible

./target/release/token-station-cli \
  --config apps/cli/claude-code-qwen-config.json \
  plugin install plugins/official/agent-anthropic
./target/release/token-station-cli \
  --config apps/cli/claude-code-qwen-config.json \
  plugin install plugins/official/provider-openai-compatible
```

The four configurations share `token-station-e2e/plugins/`. Install the plugins once. If an old package exists in the development directory, remove it with `plugin remove` before reinstallation. Do not overwrite WASM files or acceptance receipts manually.

## 4. Store API keys safely

Do not send API keys in chat. Do not put them in JSON, command arguments, or shell scripts. This zsh function hides terminal input and writes the key through standard input to the macOS keychain:

```zsh
store_provider_key() {
  local upstream="$1"
  local label="$2"
  local key

  read -s "key?$label API Key: "
  print
  print -r -- "$key" | \
    ./target/release/token-station-cli key set "$upstream" provider_api_key
  unset key
}

store_provider_key qwen Qwen
store_provider_key kimi Kimi
store_provider_key minimax MiniMax
store_provider_key glm GLM
unset -f store_provider_key
```

The CLI prints only the stored upstream and slot. It does not print the key. The configurations use different upstream names. Although each slot is named `provider_api_key`, keychain entries remain isolated by `upstream/slot`.

## 5. Check configurations and run real probes

First, check configuration and routing:

```bash
for vendor in qwen kimi minimax glm; do
  config="apps/cli/claude-code-${vendor}-config.json"
  ./target/release/token-station-cli --config "$config" upstream list
  ./target/release/token-station-cli --config "$config" rule list
done
```

Then send one real minimal completion to each provider:

```bash
./target/release/token-station-cli \
  --config apps/cli/claude-code-qwen-config.json \
  upstream test qwen --model qwen3.7-max

./target/release/token-station-cli \
  --config apps/cli/claude-code-kimi-config.json \
  upstream test kimi --model kimi-k2.7-code

./target/release/token-station-cli \
  --config apps/cli/claude-code-minimax-config.json \
  upstream test minimax --model MiniMax-M3

./target/release/token-station-cli \
  --config apps/cli/claude-code-glm-config.json \
  upstream test glm --model glm-5.2

./target/release/token-station-cli \
  --config apps/cli/claude-code-kimi-moonshot-v1-config.json \
  upstream test kimi --model moonshot-v1-128k

./target/release/token-station-cli \
  --config apps/cli/claude-code-minimax-m2.5-config.json \
  upstream test minimax --model MiniMax-M2.5
```

`upstream test` calls a real API and can incur a small charge. Standard metered keys and plan keys, such as Coding Plan or Token Plan, can use different endpoints. For a 401 or 404 response, first check the key type and region. Do not immediately classify the provider as incompatible.

## 6. Start one provider proxy

Test each provider separately to prevent port and metrics overlap. For example, start Qwen:

```bash
./target/release/token-station-cli \
  --config apps/cli/claude-code-qwen-config.json serve
```

For another provider, change only the configuration name. Each configuration uses a separate data directory:

```text
token-station-e2e/qwen/data
token-station-e2e/kimi/data
token-station-e2e/minimax/data
token-station-e2e/glm/data
token-station-e2e/kimi-moonshot-v1/data
token-station-e2e/minimax-m2.5/data
```

The first start creates a local virtual key. It authenticates Claude Code to the local proxy. It is not an upstream API key. Set its file mode to `0600`.

## 7. Start Claude Code

For Qwen, open another terminal and read the local virtual key for that configuration:

```bash
export TS_VIRTUAL_KEY="$(tr -d '\r\n' < token-station-e2e/qwen/data/virtual-key)"
```

Inject configuration only into the current Claude Code process:

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

`claude-3-5-haiku-20241022` is a protocol-compatible identifier for Claude Code. The current single-provider configuration selects the actual provider and model. Do not give the upstream API key to Claude Code.

`--setting-sources project` isolates gateway environment variables from user-level Claude settings that can override the current shell. `--safe-mode` excludes personal plugins, hooks, and MCP during the first connection test.

## 8. Acceptance tests for each provider

Complete at least these tests for each provider:

1. Run a normal streaming conversation and get fixed, verifiable text.
2. Require Claude Code to use `Read` on a fixed small repository file.
3. After the tool result returns, get the correct final answer.
4. Use an invalid local virtual key. Verify an Anthropic 401 and no upstream request.
5. Verify that upstream/model, stream, tool_count, and status in metrics match the current configuration.
6. Verify that logs contain no prompt, response, upstream key, or local virtual key.

Provider-specific fields such as `reasoning_content`, `reasoning_details`, and `<think>` are not part of the current lossless IR contract. If a provider requires them for a tool round trip, mark that provider as incompatible and submit a change request. Do not silently drop them and claim full support.

## 9. Remove keys after testing

```bash
./target/release/token-station-cli key remove qwen provider_api_key
./target/release/token-station-cli key remove kimi provider_api_key
./target/release/token-station-cli key remove minimax provider_api_key
./target/release/token-station-cli key remove glm provider_api_key
```

After removal, run `upstream test` again. It must return a keychain missing-credential error without an upstream request. Do not write the key to an environment variable or temporary file to verify removal.

## 10. Rules for a new provider

If a later provider meets the requirements in §1, copy one configuration and replace:

- The upstream name.
- The Base URL.
- The key source.
- The model ID and capabilities.

After configuration validation, probe, Claude Code conversation, and tool round-trip tests pass, connect the provider without a Rust change. Enhance the common provider or add a separate provider plugin only when real evidence shows protocol incompatibility.

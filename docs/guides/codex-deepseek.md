# Connect Codex to DeepSeek through Token Station

Codex uses the OpenAI Responses API. Token Station receives requests at `POST /v1/responses`. `agent-openai-responses` converts them to Canonical IR. Then `provider-openai-compatible` calls DeepSeek Chat Completions.

```text
Codex -> /v1/responses -> agent-openai-responses -> Canonical IR
      -> router -> provider-openai-compatible -> DeepSeek
```

Use the sample configuration at [apps/cli/codex-deepseek-config.json](../../apps/cli/codex-deepseek-config.json). It listens only on `127.0.0.1:8791`. It writes runtime data to `token-station-m4/codex/data` and does not access an existing 8787 instance.

## 1. Build and install plugins

From the repository root, build the CLI and the two plugins required by this Agent:

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

The three M4 configurations share `token-station-m4/plugins`. Install the plugins once. If a package with the same name exists, check it with `plugin info`. Do not overwrite the WASM file or acceptance receipt manually.

## 2. Start the isolated proxy

Inject the upstream credential only into the Token Station service terminal. Do not put the value in JSON or logs:

```bash
export DEEPSEEK_API_KEY='your DeepSeek API Key'

./target/release/token-station-cli \
  --config apps/cli/codex-deepseek-config.json upstream list
./target/release/token-station-cli \
  --config apps/cli/codex-deepseek-config.json rule list
./target/release/token-station-cli \
  --config apps/cli/codex-deepseek-config.json serve
```

The local virtual key is at `token-station-m4/codex/data/virtual-key`. It authenticates Codex to the local proxy. It is not the DeepSeek key.

## 3. Run with a temporary CODEX_HOME

The following configuration writes only to `/tmp`. It does not read or modify `~/.codex`:

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

These switches apply only to this temporary Codex process. The current adapter carries only local function tools. Therefore, disable Codex hosted tools, namespace tools, and personal plugin injection. Otherwise, the adapter returns a capability error for `namespace` or `web_search` tools that it cannot map without loss.

If development rules require patches for all repository files, create this temporary file manually. It is not a project artifact and must not be committed.

Run a normal streaming acceptance test:

```bash
codex exec --ephemeral --skip-git-repo-check --sandbox read-only \
  -C /tmp/token-station-m4-codex-work \
  'Reply only with this fixed marker: CODEX_M4_OK'
```

For a tool round-trip test, put `marker.txt` in the temporary working directory. Require Codex to read it with the local read tool and return its exact content. Evidence must include the final marker and at least one metrics record with `tool_count > 0`.

## 4. Error path and audit

A local authentication error must not reach the router or upstream:

```bash
TS_VIRTUAL_KEY='intentionally-wrong' \
codex exec --ephemeral --skip-git-repo-check --sandbox read-only \
  -C /tmp/token-station-m4-codex-work 'Reply AUTH_TEST'
```

The command must fail with a Responses-shaped 401. To test a controlled upstream error, stop the 8791 service. Restart the same isolated configuration with `DEEPSEEK_API_KEY='intentionally-wrong'`, then send one request. Do not run this test against an active 8787 instance.

After you stop the service and flush data, check the results:

```bash
./target/release/token-station-cli \
  --config apps/cli/codex-deepseek-config.json stats --since all
rg -n 'CODEX_M4_OK|intentionally-wrong|sk-' \
  token-station-m4/codex/data/requests.log || true
```

Logs must not contain prompts, responses, upstream keys, or the virtual key. After the test, you can remove `/tmp/token-station-m4-codex-*`. Keep metrics only if acceptance evidence requires them.

## 5. Current limits

- Text input, message input, image URLs, function tools, function calls and outputs, Responses SSE, usage, and Responses errors are covered.
- Reasoning items, computer or hosted tools, file-ID images, and the complete Responses event set are not supported. The adapter returns a capability error and does not silently drop fields.
- Structured Responses output is not supported. When `text.format.type` is `json_schema` or `json_object`, the adapter returns `unsupported_capability` before routing or an upstream request.
- This procedure proves the main Codex text and local function-tool path. It does not prove full compatibility with all Responses capabilities.

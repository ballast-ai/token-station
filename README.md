# token-station

[中文](README.zh-CN.md)

A loopback LLM proxy with local routing. Point your IDE, agent, or any
OpenAI-compatible client at `127.0.0.1`, and token-station routes each request
to the right upstream — your own API keys, your local models — by rules you
wrote, on your machine, with nothing about your prompts ever leaving it.

```
IDE / agent ──▶ 127.0.0.1:8787 ──▶ rules → hints → heuristic → default
                                        │
                     ┌──────────────────┼──────────────────┐
                     ▼                  ▼                  ▼
               api.openai.com     api.groq.com      localhost:11434
               (your key)         (your key)        (your Ollama)
```

## Why this exists

Model routing usually means a cloud gateway reading your traffic. token-station
takes the opposite bet: **routing is a local decision**. The router, the rules,
the metrics, and your keys all live on your machine. The proxy's outbound
connections are exactly the upstreams you configured — plus one anonymous
version check, only when you run `upgrade`, never in the background.

- **Content cannot reach disk.** The request log and metrics store are built
  from a record type whose fields are numbers, closed enums, or names from
  your own config. There is no column a prompt could go into — and the tests
  prove it by grepping the raw database bytes for a canary.
- **Keys stay in the OS keychain**, resolved per request, injected into one
  header, never logged. A key pasted into a URL is refused at config load.
- **Plugins are sandboxed WASM.** Provider adapters translate protocols; they
  get no network and no secrets. An exfiltration gate checks every outbound
  request against the endpoint you configured before any credential resolves.
- **Auth is on by default.** A local virtual key exists before the port does;
  loopback is a boundary against the network, not against other processes.

## Quick start

```bash
git clone https://github.com/ballast-ai/token-station
cd token-station
cargo build --release -p token-station-cli

cp apps/cli/example-config.json token-station.json
./target/release/token-station-cli upstream add openai_personal \
  --provider openai-compatible --base-url https://api.openai.com/v1 \
  --model "gpt-5.5,tool,vision,json-schema,ctx=400000" \
  --auth keyring --pool sota
./target/release/token-station-cli key set openai_personal provider_api_key
./target/release/token-station-cli serve
```

Then point your client at `http://127.0.0.1:8787/v1` with the virtual key that
`serve` printed. Ask for model `auto` and the router decides; ask for a
concrete model and you get it.

Manage it from the same binary: `upstream list/add/remove/test`,
`rule list`, `config set/edit`, `stats` (volume, errors, latency, tokens —
read from the local metrics store).

## Verifying official binaries

Official releases are reproducible and signed: an Ed25519-signed manifest
proves the publisher, and rebuilding at the release tag proves the source —
`scripts/verify-release.sh` does both comparisons for you. The signing key
lives offline; the public key is in this source tree. Details in
[docs/release/可复现构建与发布验证.md](docs/release/可复现构建与发布验证.md).

## Status

C1 (minimal usable client) is complete: streaming proxy, four-layer routing,
upstream health ejection, OS keychain custody, metrics, CLI management
surface, and this release engineering. OpenAI-compatible upstreams (including
Ollama, vLLM, and most BYOK providers) work today; native Anthropic/Gemini
adapters are next, guided by community feedback.

Design documents live under [docs/](docs/) (currently in Chinese; the
architecture speaks fluent diagram).

## License

Apache-2.0. The routing kernel, plugin ABI, and this client are the open
core; a hosted platform (accounts, cloud sync of **metadata only**, bills
reconciliation) is being built on the same crates.

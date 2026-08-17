# Token Station reference

This page keeps the detail that the README omits. Product behavior on a given release is defined by that release.

## Routing

| Mode | Behavior |
|---|---|
| Direct | Pins each request to one managed provider and model. |
| Smart tiers | Selects High, Mid, or Low from explicit rules, agent hints, deterministic heuristics, and a configured default. It makes one decision. It does not try a cheaper tier and then escalate. |
| Quota first | Favors buckets that reset sooner. Accounts without a reset window, including metered accounts, come last. Inside one bucket it considers conversation affinity, instantaneous rate headroom, and pressure. Configured order only breaks an exact tie. Remaining quota is an exhaustion gate, not a "most remaining" sort. |

Each agent can inherit the Home route or override its mode and targets. Smart tiers can use custom mappings or a reusable profile. Quota accounts are a shared global pool.

Quota first prefers recognized provider rate-limit headers when they are present. Without those headers, the local estimate only sees traffic that passed through this gateway.

Do not use Quota first when the workload must stay on the device. That path does not apply the strict-local candidate filter.

## Providers

Start from 40+ editable official, managed, and local presets, or add a custom OpenAI-compatible endpoint. The app can discover and compare models, record capabilities and limits, and run model-level health checks.

Before a selected free or trial entry is added, a real completion validates reachability and protocol behavior. That check does not prove later requests stay free. Offer availability and billing stay under provider control.

Provider presets are editable starting points, not availability guarantees. Models, free offers, regions, pricing, and limits can change at the provider.

Provider requests, model discovery, and health probes can use direct access, HTTP CONNECT, or SOCKS5, with validated `no_proxy` rules and separate proxy credentials. These flows do not inherit ambient proxy environment variables. The desktop updater uses its own HTTP stack and may follow system or environment proxy settings.

## Desktop

The app includes first-run guidance, Agent rescanning, Provider, Usage, Settings, light and dark themes, English and Simplified Chinese, request-log inspection, encrypted connector snapshots, a provider recycle bin, and safe-mode recovery export.

Signed in-app update checks and installation are limited to supported official macOS builds. Source or local builds without the official public key, plus Windows and Linux, use manual updates. Plugin management is a CLI workflow.

On macOS, closing the main window hides it. The process stays resident and a running proxy keeps serving connected agents. The menu bar item shows proxy state, start and stop controls, managed agents, and shortcuts to Add Provider, Request logs, and Settings. Quit from the menu to stop the app and its proxy.

Background mode is not a system daemon, a login item, or an automatic crash-restart service.

## Agents

| Agent | Integration | Inbound protocol |
|---|---|---|
| [Claude Code](https://github.com/anthropics/claude-code) | Built-in connector | Anthropic Messages |
| [Claude Desktop](https://github.com/anthropics) | Built-in connector | Anthropic Messages |
| [Codex](https://github.com/openai/codex) | Built-in connector | OpenAI Responses |
| [Gemini CLI](https://github.com/google-gemini/gemini-cli) | Built-in connector | Gemini |
| [Grok Build](https://github.com/xai-org/grok-cli) | Built-in connector | OpenAI Chat Completions |
| [Kimi Code](https://github.com/MoonshotAI/kimi-code) | Built-in connector | OpenAI Chat Completions |
| [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) | Built-in connector (developer preview upstream) | OpenAI Chat Completions |
| [Hermes Agent](https://github.com/NousResearch/hermes-agent) | Built-in connector | OpenAI Chat Completions |
| [OpenClaw](https://github.com/openclaw/openclaw) | Built-in connector | OpenAI Chat Completions |
| [WorkBuddy](https://www.workbuddy.ai/) | Built-in connector | OpenAI Chat Completions |
| [OpenCode](https://github.com/anomalyco/opencode) | Built-in connector | OpenAI Chat Completions |
| [Cursor](https://github.com/cursor/cursor) | Dedicated setup on macOS and Windows | OpenAI-compatible endpoint |

Claude Desktop does not currently have a public product repository. Its link opens Anthropic's official GitHub organization.

DeepSeek Harness discovery covers both a normal `dsh` command and the bounded npm cache layout created by `npx @deepseek-ai/dsh web`. Token Station reads existing cache entries only. It never runs `npx` or installs the package during a scan.

Grok Build uses `~/.grok/config.toml`, or `$GROK_HOME/config.toml` when `GROK_HOME` is set. Kimi Code uses `~/.kimi-code/config.toml`, or `$KIMI_CODE_HOME/config.toml` when `KIMI_CODE_HOME` is set. DeepSeek Harness uses `~/.dsh/settings.yaml` and `~/.dsh/.credentials.yaml`. When `DSH_HOME` is set, both files use that directory. All three connectors route OpenAI Chat Completions through the built-in `agent-openai` ingress.

For the eleven built-in connectors, clicking **Connect** is consent to apply a bounded plan immediately. Token Station starts the gateway when needed and shows the fields changed by the first connection. Connector availability depends on the agent and operating system. It does not imply that Token Station publishes an installer for that platform.

Cursor uses a separate path on macOS and Windows. Quit Cursor first. Token Station privately backs up the relevant SQLite records, writes the OpenAI-compatible endpoint, virtual key, and enablement flag, verifies the result, and restores the previous values if verification fails. Restart Cursor afterward and choose a model that supports its custom OpenAI key path. This path is not covered by standard connector ownership or managed disconnect.

## Workloads that must stay on the device

1. Add Ollama or another provider whose Base URL host is a verified loopback host. Confirm that the local runtime itself does not relay requests to a cloud service.
2. Use Direct or Smart tiers and enable strict local routing.
3. Keep cloud fallback disabled and confirm the selected models use that local endpoint.
4. Set Egress to direct access, or put the exact loopback host in `no_proxy` and verify that runtime traffic bypasses the configured HTTP or SOCKS proxy.

Provider labels and model names alone are not proof of locality. A loopback endpoint only proves the first network hop.

## Security

| Boundary | Current behavior |
|---|---|
| Listener | Non-loopback listen addresses are rejected. Local authentication is enabled by default with a per-installation virtual key. |
| Desktop request bodies | The desktop gateway stores client input and final client-facing output as owner-only plaintext JSON so Request logs can inspect them. Cleanup uses a 7-day retention threshold and a target of 1,000 request files. Each side is capped at 256 KiB and marked when truncated. Request headers and host-injected provider credentials are not captured. Secrets inside a request or response body are retained as body content. Cleanup runs at startup and periodically while writing. |
| Receipt logs and metrics | Rotating `requests.log` receipts and `metrics.sqlite` do not contain prompt or response bodies. The CLI gateway does not enable the separate body store by default. |
| Cloud routing | A cloud upstream receives requests routed to it. Token Station cannot override that provider's retention, logging, or training policy. |
| Provider credentials | The default store is plaintext `secrets.json` with owner-only permissions. Other processes running as the same operating-system user may still read it. Environment-variable and standalone-file sources are supported. Credential values are excluded from logs, errors, and sandboxed plugins. |
| Plugin sandbox | WASM adapters receive no direct network, filesystem, environment, arguments, inherited standard I/O, or plaintext credential access. Memory and call time are limited. |
| Outbound authorization | Before attaching a credential, the host checks that the destination origin, path boundary, and credential slot match the configured provider. |
| Agent configuration | Built-in connectors use revision and ownership checks, private AES-256-GCM snapshots, atomic private writes, and recovery flows. The snapshot key is an owner-only local file, not an operating-system keychain entry. Cursor uses the separate SQLite path above. |

Private file permissions isolate local state from other operating-system accounts. Request-body history and the default credential store are not encrypted at rest. Use the [request-body cleanup script](../scripts/cleanup-request-bodies.sh) to prune files older than a chosen retention window. The Bash script defaults to the macOS app data path. Pass `--data-dir` for another Unix-like location. Use environment variables or a separately managed secret file when your credential custody requirements differ.

## Build and verify

The command blocks below use a POSIX-compatible shell. On Windows, run the repository scripts from Git Bash or adapt environment-variable assignments for PowerShell.

Desktop development requires Rust stable with MSRV 1.95, Node.js 22.23.1, npm, the `wasm32-wasip2` Rust target, and the platform-specific Tauri dependencies.

```bash
git clone https://github.com/ballast-ai/token-station.git
cd token-station
rustup target add wasm32-wasip2
npm --prefix apps/desktop ci
npm --prefix apps/desktop run tauri:dev
```

Use the repository `tauri:dev` command. It builds and embeds the five official WASM adapters before starting Tauri. For frontend-only work, use `npm --prefix apps/desktop run dev`.

```bash
scripts/build-desktop.sh --local
```

On macOS, this guarded workflow builds, audits, installs, verifies, and launches the local app:

```bash
scripts/install-local-desktop.sh
```

```bash
cargo build -p token-station-cli
./target/debug/token-station-cli --help
```

A normal debug or release-profile Cargo build does not embed the five official adapters. Supply an external plugin directory when serving locally. Official packaging uses `scripts/build-release.sh <target-triple>`.

The Tauri crate is excluded from the root Cargo workspace, so its Rust checks must run separately.

```bash
scripts/check-rust-format.sh
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

scripts/prepare-desktop-test-plugins.sh
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
RUSTDOCFLAGS="-D warnings" cargo doc --manifest-path apps/desktop/src-tauri/Cargo.toml --no-deps

npm --prefix apps/desktop run test:coverage
npm --prefix apps/desktop run build
```

See the [CI workflow](../.github/workflows/ci.yml) for the complete matrix.

## Repository layout

```text
apps/cli/                    Native CLI and local gateway
apps/desktop/                React and Tauri desktop app
crates/                      Shared routing, protocol, storage, and security crates
plugins/official/            Five official WASM adapters
docs/guides/                 Agent integration guides
scripts/                     Build, release, validation, and maintenance scripts
```

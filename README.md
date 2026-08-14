<div align="center">
  <img src="apps/desktop/public/icon.png" alt="Token Station icon" width="112" />

# Token Station

### Local routing control plane for AI agents and LLM providers

Connect Claude Code, Codex, Gemini CLI, WorkBuddy, and other agents to one loopback-only gateway with authentication enabled by default. Pin traffic to a provider and model, route by task complexity, or make better use of quota windows across the API providers and local runtimes you control.

[![Release](https://img.shields.io/github/v/release/ballast-ai/token-station?display_name=tag&sort=semver)](https://github.com/ballast-ai/token-station/releases/latest) [![CI](https://github.com/ballast-ai/token-station/actions/workflows/ci.yml/badge.svg)](https://github.com/ballast-ai/token-station/actions/workflows/ci.yml) [![License](https://img.shields.io/github/license/ballast-ai/token-station)](LICENSE)

[Download](https://github.com/ballast-ai/token-station/releases/latest) · [Quick start](#quick-start) · [Documentation](docs/README.md) · [Report an issue](https://github.com/ballast-ai/token-station/issues) · [简体中文](README.zh-CN.md)
</div>

> The source tree can be ahead of the latest published release. Treat each release page as the source of truth for its actual installers, architectures, signatures, checksums, and upgrade notes.

## What Token Station does

Token Station keeps one local gateway in the request path between your AI agents and model providers. Routing decisions, provider configuration, quota estimates, and usage metadata stay under your control, while requests still reach whichever local or cloud upstream you select.

| Concern | Current behavior |
|---|---|
| Gateway | Rust proxy restricted to loopback, with authentication enabled by default at `127.0.0.1:8787` |
| Routing | Direct, Smart tiers, and Quota first, with global defaults and per-agent overrides |
| Inbound protocols | Anthropic Messages, OpenAI Chat Completions, OpenAI Responses, and Gemini |
| Upstreams | 40+ editable managed and local presets, a curated free or trial catalog, and custom OpenAI-compatible endpoints |
| Control surfaces | Tauri desktop app and native CLI backed by the same Rust core |
| Local state | Provider configuration, credentials, routing rules, quota estimates, request receipts, metrics, and desktop request-body history |

## How it works

<p align="center">
  <picture>
    <source media="(max-width: 600px)" srcset="docs/assets/token-station-architecture-en-mobile.svg">
    <img src="docs/assets/token-station-architecture-en.svg" alt="Token Station request-routing architecture" width="720">
  </picture>
</p>

1. A managed agent sends its native protocol to the loopback gateway, which requires local authentication by default.
2. Token Station applies the agent's route, selects one provider and model, and converts the request when required.
3. The Rust host validates the destination and credential slot before resolving, injecting, and forwarding credentials as required.
4. The response is converted back to the agent's protocol, while routing, timing, usage, and quota metadata are recorded locally.

The gateway only accepts loopback listen addresses. A request routed to a cloud provider still leaves the device and is subject to that provider's data policy.

## Current capabilities

- **Three routing modes.** Direct pins requests to one managed provider and model. Smart tiers selects High, Mid, or Low using explicit rules, agent hints, deterministic heuristics, and a configured default. It makes one routing decision rather than first trying a cheaper tier and silently escalating. Quota first favors earlier reset buckets and puts accounts without a reset window, including metered accounts, last. Within one bucket it considers conversation affinity, instantaneous rate headroom, and pressure; configured order only breaks an exact tie. Remaining quota acts as an exhaustion gate, not a "most remaining" sort.
- **Per-agent routing.** Each agent can inherit the Home route or override its mode and targets. Smart tiers can use custom mappings or a reusable profile. Quota accounts remain a shared global pool rather than separate per-agent balances.
- **Provider and model management.** Start from 40+ editable official, managed, and local presets, add a custom OpenAI-compatible endpoint, discover and compare models, record capabilities and limits, and run model-level health checks. Before a selected free or trial entry is added, a real completion validates reachability and protocol behavior. It does not prove that future requests remain free; offer availability and billing stay under provider control.
- **Usage and diagnosis.** Filter by time, agent, provider, and model; inspect requests, tokens, estimated cost, success rate, P95 latency, quota state, routing attempts, and protocol conversions. Agent budgets are informational and do not block traffic.
- **Controlled provider egress.** Provider requests, model discovery, and health probes can use direct access, HTTP CONNECT, or SOCKS5 with validated `no_proxy` rules and separate proxy credentials. These flows do not silently inherit ambient proxy environment variables. The desktop updater uses its own HTTP stack and may follow system or environment proxy settings.
- **Reversible agent integration.** Built-in connectors use bounded change plans, ownership checks, private encrypted snapshots, atomic writes, and recovery flows. Disconnect removes Token Station-owned fields without deleting unrelated agent settings.
- **Sandboxed WASM adapters.** Five official adapters cover the four inbound protocols and OpenAI-compatible providers. Adapters receive no direct network, filesystem, environment, argument, standard I/O, or plaintext credential access; the Rust host mediates privileged operations with memory and call-time limits.
- **Desktop-first operation.** The app includes first-run guidance, Agent rescanning, Provider, Usage, Settings, light and dark themes, English and Simplified Chinese, request-log inspection, encrypted connector snapshots, a provider recycle bin, and safe-mode recovery export. Signed in-app update checks and installation are limited to supported official macOS builds; source or local builds without the official public key, plus Windows and Linux, use manual updates. Plugin management is currently a CLI workflow, not a mounted desktop page.

Provider presets are editable starting points, not availability guarantees. Models, free offers, regions, pricing, and limits can change at the provider.

## macOS background mode and status menu

On macOS, closing the main window hides it instead of terminating Token Station. The app process stays resident and a running proxy keeps serving connected agents.

The menu bar status item provides:

- the current proxy state and listen address;
- start, stop, and retry controls;
- the number of managed agents and direct links to their routing pages;
- shortcuts to Add Provider, Request logs, and Settings;
- actions to reopen the existing window or quit Token Station.

Clicking the Dock icon or choosing the open action from the menu bar reopens the existing window. Use the menu's quit action to stop the app and its proxy.

Background mode means that the desktop process remains alive after its window is closed. It is not a system daemon, a login item, automatic launch after reboot, or an automatic crash-restart service.

## Supported agents

| Agent | Integration | Inbound protocol |
|---|---|---|
| <a href="https://github.com/anthropics/claude-code"><img src="docs/assets/agents/claude-code.svg" width="20" height="20" alt=""> Claude Code</a> | Built-in connector | Anthropic Messages |
| <a href="https://github.com/anthropics"><img src="docs/assets/agents/claude-desktop.svg" width="20" height="20" alt=""> Claude Desktop</a> | Built-in connector | Anthropic Messages |
| <a href="https://github.com/openai/codex"><img src="docs/assets/agents/codex.svg" width="20" height="20" alt=""> Codex</a> | Built-in connector | OpenAI Responses |
| <a href="https://github.com/google-gemini/gemini-cli"><img src="docs/assets/agents/gemini-cli.svg" width="20" height="20" alt=""> Gemini CLI</a> | Built-in connector | Gemini |
| <a href="https://github.com/NousResearch/hermes-agent"><img src="apps/desktop/public/agents/hermes.png" width="20" height="20" alt=""> Hermes Agent</a> | Built-in connector | OpenAI Chat Completions |
| <a href="https://github.com/openclaw/openclaw"><img src="docs/assets/agents/openclaw.svg" width="20" height="20" alt=""> OpenClaw</a> | Built-in connector | OpenAI Chat Completions |
| <a href="https://www.workbuddy.ai/"><img src="apps/desktop/public/agents/workbuddy.png" width="20" height="20" alt=""> WorkBuddy</a> | Built-in connector | OpenAI Chat Completions |
| <a href="https://github.com/anomalyco/opencode"><img src="docs/assets/agents/opencode.svg" width="20" height="20" alt=""> OpenCode</a> | Built-in connector | OpenAI Chat Completions |
| <a href="https://github.com/cursor/cursor"><img src="docs/assets/agents/cursor.svg" width="20" height="20" alt=""> Cursor</a> | Dedicated setup on macOS and Windows | OpenAI-compatible endpoint |

Claude Desktop does not currently have a public product repository; its link opens Anthropic's official GitHub organization.

For the eight built-in connectors, clicking **Connect** is consent to immediately apply a bounded plan. Token Station starts the gateway when needed and shows the fields changed by the first connection. Connector availability depends on the agent and operating system, and does not imply that Token Station publishes an installer for that platform.

Cursor uses a separate path on macOS and Windows. Quit Cursor first. Token Station privately backs up the relevant SQLite records, transactionally writes the OpenAI-compatible endpoint, virtual key, and enablement flag, verifies the result, and restores the previous values if verification fails. Restart Cursor afterward and choose a model that supports its custom OpenAI key path. This path is not covered by standard connector ownership or managed disconnect.

## Download and install

Download a published build from **[GitHub Releases](https://github.com/ballast-ai/token-station/releases/latest)**, or build the current source tree yourself.

Release assets are version-specific. A configured Tauri or Rust build target does not prove that an installer or CLI archive has been published for that platform. Check the selected release for its exact operating system, CPU architecture, signing or notarization state, checksum, and minimum system version.

If a release provides an unsigned or unnotarized macOS test DMG, verify its published SHA-256 and follow the macOS installation notice. Do not disable Gatekeeper system-wide. Build locally if you need an artifact derived from source you inspected. If you require a Developer ID signed and notarized binary, use only a release that explicitly provides one.

## Quick start

You need at least one provider API key or a local model endpoint. Token Station does not import Claude, Codex, or other agent subscriptions and OAuth sessions as provider accounts.

1. Open Token Station and select **Add Provider**. Choose a preset or enter a custom OpenAI-compatible endpoint, then add its models and a credential when that endpoint requires one.
2. Wait for the startup Agent scan to finish. Open the fixed first row on **Home**, choose Direct, Smart tiers, or Quota first, configure its targets, and apply the global route.
3. Select a detected agent from the Home sidebar. If an installation is missing, use **Rescan**; restarting the app is not required. For a built-in connector, click **Connect** to start the gateway if needed and apply its bounded configuration change. Follow the separate warning above for Cursor.
4. Send a request from the managed agent. With the default settings, it connects to `127.0.0.1:8787` using local authentication.
5. Open **Usage** to inspect the selected route, tokens, latency, failures, quota state, estimated cost, and retained request content where available.

Quota first prefers recognized provider rate-limit headers when available. Without authoritative headers, its local estimate only sees traffic that passed through this gateway, not consumption by other clients using the same credential.

### Workloads that must stay on the device

For a workload that must never reach a cloud provider:

1. Add Ollama or another provider whose Base URL host is verified as a loopback host, and verify that the local runtime itself does not relay requests to a cloud service.
2. Use Direct or Smart tiers and enable strict local routing.
3. Keep cloud fallback disabled and confirm the selected models use that local endpoint.
4. Set Egress to direct access, or put the exact loopback host in `no_proxy` and verify that runtime traffic bypasses the configured HTTP or SOCKS proxy.

Do not use Quota first for this requirement. Its current route path does not apply the strict-local candidate filter. Provider labels and model names alone are not proof of locality, and a loopback endpoint only proves the first network hop.

## Security and data boundaries

| Boundary | Current behavior |
|---|---|
| Listener | Non-loopback listen addresses are rejected. Local authentication is enabled by default with a per-installation virtual key. |
| Desktop request bodies | The desktop gateway stores client input and final client-facing output as owner-only plaintext JSON so the Request logs view can inspect them. The cleanup policy uses a 7-day retention threshold and a target of 1,000 request files. Each side is capped at 256 KiB and marked when truncated. Request headers and host-injected provider credentials are not captured, but secrets present inside a request or response body are retained as body content. Cleanup runs at startup and periodically while writing. |
| Receipt logs and metrics | Rotating `requests.log` receipts and `metrics.sqlite` do not contain prompt or response bodies. The CLI gateway does not enable the separate body store by default. |
| Cloud routing | A cloud upstream receives requests routed to it. Token Station cannot override that provider's retention, logging, or training policy. |
| Provider credentials | The default store is plaintext `secrets.json` with owner-only permissions. Other processes running as the same operating-system user may still read it. Environment-variable and standalone-file sources are supported. Credential values are excluded from logs, errors, and sandboxed plugins. |
| Plugin sandbox | WASM adapters receive no direct network, filesystem, environment, arguments, inherited standard I/O, or plaintext credential access. Memory and call time are limited. |
| Outbound authorization | Before attaching a credential, the host checks that the destination origin, path boundary, and credential slot match the configured provider. |
| Agent configuration | Built-in connectors use revision and ownership checks, private AES-256-GCM snapshots, atomic private writes, and recovery flows. The snapshot key is an owner-only local file, not an operating-system keychain entry. Cursor uses the separate SQLite path described above. |

Private file permissions isolate local state from other operating-system accounts, but request-body history and the default credential store are not encrypted at rest. Use the [request-body cleanup script](scripts/cleanup-request-bodies.sh) to prune files older than a chosen retention window. The Bash script defaults to the macOS app data path; pass `--data-dir` for another Unix-like location. Use environment variables or a separately managed secret file when your credential custody requirements differ.

## Build and run from source

The command blocks below use a POSIX-compatible shell. On Windows, run the repository scripts from Git Bash or adapt environment-variable assignments for PowerShell.

### Desktop app

Desktop development requires Rust stable with MSRV 1.95, Node.js 22.23.1, npm, the `wasm32-wasip2` Rust target, and the platform-specific Tauri dependencies listed in the development guide.

```bash
git clone https://github.com/ballast-ai/token-station.git
cd token-station
rustup target add wasm32-wasip2
npm --prefix apps/desktop ci
npm --prefix apps/desktop run tauri:dev
```

Use the repository's `tauri:dev` command. It builds and embeds the five official WASM adapters before starting Tauri. For frontend-only work, use `npm --prefix apps/desktop run dev`.

Build an audited local bundle with:

```bash
scripts/build-desktop.sh --local
```

On macOS, the repository can build, audit, install, verify, and launch the local app in one guarded workflow:

```bash
scripts/install-local-desktop.sh
```

### CLI

The CLI needs Rust stable with MSRV 1.95. Platform toolchain requirements are documented in the development guide.

```bash
cargo build -p token-station-cli
./target/debug/token-station-cli --help
```

A normal debug or release-profile Cargo build does not embed the five official adapters. Supply an external plugin directory when serving locally. Official packaging uses `scripts/build-release.sh <target-triple>`, which builds the adapters and enables the built-in plugin feature under the release credential requirements.

<details>
<summary><strong>Core local gates</strong></summary>

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

CI also runs dependency policy and advisory checks, Rust coverage and MSRV gates, desktop security and release checks, and platform-specific jobs. See the [CI workflow](.github/workflows/ci.yml) for the complete authoritative matrix.

</details>

## Repository layout

```text
apps/cli/                    Native CLI and local gateway
apps/desktop/                React and Tauri desktop app
crates/                      Shared routing, protocol, storage, and security crates
plugins/official/            Five official WASM adapters
docs/product/                User documentation
docs/guides/                 Agent integration guides
docs/contributing/           Architecture and development guides
docs/design/                 Design records and acceptance boundaries
scripts/                     Build, release, validation, and maintenance scripts
```

## Documentation

- [Agent setup guides](docs/guides/) (English)

## Contributing

Issues and focused pull requests are welcome. Read the contribution guide first. User-visible UI, interaction, state, contract, or release behavior changes require a design document in `docs/design/` before tests and implementation; keep that document in the same pull request as the change.

## License

Licensed under the [Apache License 2.0](LICENSE).

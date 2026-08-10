<div align="center">
  <img src="apps/desktop/public/icon.png" alt="Token Station icon" width="112" />

# Token Station

### Local routing control plane for AI agents and LLM providers

Connect Claude Code, Codex, WorkBuddy, and other agents to one loopback gateway. Route each request by task complexity or remaining quota across your own API providers and local models.

[![Release](https://img.shields.io/github/v/release/ballast-ai/token-station?display_name=tag&sort=semver)](https://github.com/ballast-ai/token-station/releases/latest) [![CI](https://github.com/ballast-ai/token-station/actions/workflows/ci.yml/badge.svg)](https://github.com/ballast-ai/token-station/actions/workflows/ci.yml) [![Platform](https://img.shields.io/badge/public%20desktop-macOS%20Apple%20Silicon-lightgrey.svg)](https://github.com/ballast-ai/token-station/releases/latest) [![License](https://img.shields.io/github/license/ballast-ai/token-station)](LICENSE)

[Download](https://github.com/ballast-ai/token-station/releases/latest) · [Documentation](docs/README.md) · [Report an issue](https://github.com/ballast-ai/token-station/issues) · [简体中文](README.zh-CN.md)
</div>

## Why Token Station?

AI agents use different configuration files and request protocols. LLM providers expose different endpoints, models, limits, and reset windows. A static provider switch chooses one configuration before an agent starts. Token Station stays in the request path, so it can make a different routing decision for every request while keeping that decision on your machine.

- Send difficult work to a high-capability model and routine work to a cheaper or local model.
- Drain quota-bearing accounts before their reset windows, then fall back to metered providers.
- Give every agent its own route, or let all agents inherit one global policy.
- Keep routing rules, provider credentials, usage metadata, and cost estimates under local control.

## How it works

<p align="center">
  <picture>
    <source media="(max-width: 600px)" srcset="docs/assets/token-station-architecture-en-mobile.svg">
    <img src="docs/assets/token-station-architecture-en.svg" alt="Token Station request-routing architecture" width="720">
  </picture>
</p>

The gateway only binds to a loopback address. A request sent to a cloud provider still leaves the device and is subject to that provider's data policy. Strict local routing only admits providers verified as loopback endpoints.

## Features

- **Two routing modes.** Three-tier routing maps requests to High, Mid, or Low models using explicit keywords, request capabilities, and a deterministic complexity score. Quota-first routing favors accounts whose allowance is most useful to spend before reset.
- **Per-agent policies.** Each agent can inherit the global route, use an independent route, or mount a reusable routing profile.
- **Broad provider catalog.** The desktop app includes 40+ editable presets for first-party APIs, managed inference services, and Ollama, plus a curated catalog of free or trial offers. Custom OpenAI-compatible endpoints are supported.
- **Sandboxed WASM plugin architecture.** Five official adapters cover Anthropic Messages, OpenAI Chat Completions, OpenAI Responses, Gemini, and OpenAI-compatible providers. Adapters cannot directly access the network, filesystem, environment variables, or plaintext credentials; the Rust host validates privileged operations.
- **Usage and cost visibility.** Inspect requests, tokens, latency, errors, quota windows, and estimated cost without storing prompt or response bodies in Token Station's request log or metrics database.
- **Failure handling.** Unhealthy upstreams are temporarily ejected, cooldowns are tracked locally, and provider diagnosis separates DNS, TLS, HTTP, authentication, model access, and generation failures.
- **Reversible connector integration.** For the eight built-in connectors, Token Station creates a bounded change plan, writes only owned fields, keeps a private backup, and can remove its fields without deleting unrelated agent settings.
- **Rust-native core, shared by desktop and CLI.** The local gateway, router, credential resolver, metrics layer, and plugin host are implemented in Rust. The Tauri desktop app and native CLI reuse the same core instead of duplicating routing logic.

Provider presets are editable starting points, not availability guarantees. Models, free tiers, regions, and limits can change at the provider.

## Supported agents

| Agent | Integration | Inbound protocol |
|---|---|---|
| Claude Code | Built-in connector | Anthropic Messages |
| Claude Desktop | Built-in connector | Anthropic Messages |
| Codex | Built-in connector | OpenAI Responses |
| Gemini CLI | Built-in connector | Gemini |
| Hermes Agent | Built-in connector | OpenAI Chat Completions |
| OpenClaw | Built-in connector | OpenAI Chat Completions |
| WorkBuddy | Built-in connector | OpenAI Chat Completions |
| OpenCode | Built-in connector | OpenAI Chat Completions |
| Cursor | Dedicated setup on macOS and Windows | OpenAI-compatible endpoint |

For the eight built-in connectors, clicking **Connect** creates and immediately applies a bounded plan. The first connection shows the changed fields after the write. Cursor is a separate path: quit Cursor first, then one-click setup backs up and updates two values in its local SQLite settings. Cursor is not covered by connector ownership or the in-app disconnect flow; manual setup remains available.

## Download and install

Download the current release from **[GitHub Releases](https://github.com/ballast-ai/token-station/releases/latest)**.

### Current public artifacts

| Target | Availability |
|---|---|
| macOS desktop, Apple Silicon | Available as the `token-station_*_aarch64.dmg` asset |
| macOS CLI, Apple Silicon | Available as the `token-station-cli-*-aarch64-apple-darwin.tar.gz` asset |
| macOS Intel desktop | Not included in the current public release |
| Windows desktop | Build and installer tests exist, but no public installer is included in the current release |
| Linux desktop | Build workflow exists, but no public package is included in the current release |

Use macOS 11.0 or newer. The executable's deployment target is 11.0, although the current app bundle metadata still shows 10.13 and needs correction. The desktop app and CLI are versioned independently, so their asset versions may differ. To confirm an Apple Silicon Mac, open **Apple menu > About This Mac**, or run `uname -m` and check for `arm64`.

The current DMG is not Developer ID signed or Apple notarized. macOS may block the first launch. Drag Token Station to Applications, then right-click the app and choose **Open**, followed by **Open** again. If Gatekeeper still keeps the quarantine attribute, run:

```bash
xattr -dr com.apple.quarantine /Applications/token-station.app
```

Do not disable Gatekeeper system-wide.

The current release also has no checksum or signature asset for independent integrity verification. If your threat model requires a verifiable binary, build from source or wait for a signed release instead of removing quarantine.

## Quick start

You need at least one provider API key or a local model endpoint. Token Station does not import Claude, Codex, or other agent subscriptions and OAuth sessions as provider accounts. Quota-first routing uses recognized provider rate-limit headers when available; otherwise it estimates a quota plan from traffic observed through this gateway and cannot see usage from other clients.

1. Open Token Station and choose **Add Provider**. Pick a preset or enter a custom OpenAI-compatible endpoint, then add its models and API key.
2. Open **Routing**. Choose three-tier or quota-first routing, configure the models or accounts, then choose **Save and apply**. This starts or reloads the local proxy.
3. Open **Agents** and scan installed agents. For a built-in connector, clicking **Connect** writes immediately and then shows what changed. For Cursor, follow the separate setup warning above.
4. Send one request from a managed agent. It will connect to the authenticated loopback gateway at `127.0.0.1:8787`.
5. Open **Usage** to verify the routing decision, tokens, latency, failures, quota state, and estimated cost.

If a workload must not leave the device, add a local provider such as Ollama and enable strict local routing. Do not rely on a model name alone to establish locality.

## Security and data boundaries

| Boundary | Current behavior |
|---|---|
| Listener | The client rejects non-loopback listen addresses. Local authentication is enabled by default with a per-installation virtual key. |
| Request content | Request and response bodies exist in memory while Token Station forwards them, but Token Station does not add them to its request log or metrics database. Automated tests scan those stores for test markers. |
| Cloud routing | A cloud upstream receives the requests routed to it. Token Station cannot override that provider's retention, logging, or training policy. |
| Provider credentials | The default store is a plaintext `secrets.json` readable only by the current user account. Other processes running as the same user may still read it. Environment-variable and standalone-file sources are also supported. Credential values are excluded from logs, errors, and sandboxed plugins. |
| Plugin sandbox | WASM adapters receive no network, filesystem, environment, arguments, or inherited standard I/O. Memory and call time are limited. |
| Outbound authorization | Before adding a credential to a request, the host checks that its destination and credential name match the provider you configured. |
| Agent configuration | Clicking **Connect** is consent to write. Built-in connectors use bounded plans, revision checks, private backups or ownership records, atomic writes, and recovery flows. The first-connection diff is shown after writing. Cursor uses the separate SQLite path described above. |

Private file permissions separate the default credential store from other user accounts, but the file is not encrypted at rest. Use environment variables or a separately managed secret file if your threat model requires a different custody mechanism.

## FAQ

<details>
<summary><strong>Is Token Station a cloud gateway?</strong></summary>

No. The router and proxy run on your machine and require no Token Station account. Traffic only reaches a cloud service when the selected upstream is a cloud provider you configured, or when you explicitly invoke a feature that fetches public catalog, pricing, or release data.

</details>

<details>
<summary><strong>How is this different from switching provider configs?</strong></summary>

A config switcher selects one provider before an agent runs. Token Station keeps a local gateway in the request path and can choose a provider and model per request. It can also apply different policies to different agents without duplicating the routing engine.

</details>

<details>
<summary><strong>Does Token Station store prompts or responses?</strong></summary>

Not in its application request log or metrics database. Those stores contain routing decisions, closed status values, timings, usage, configured names, and cost estimates. Request bodies still exist in process memory while being proxied, and cloud providers can retain what you send under their own policies.

</details>

<details>
<summary><strong>Can it use my Claude or Codex subscription?</strong></summary>

Not automatically. Agent subscriptions and OAuth sessions are not provider accounts in Token Station. Configure your own provider API key, a supported free or trial API offer, or a local model endpoint.

</details>

<details>
<summary><strong>What happens when I disconnect an agent?</strong></summary>

For the eight built-in connectors, disconnect removes Token Station-owned fields and returns the agent to its official configuration path while preserving unrelated settings. Cursor's dedicated SQLite setup does not yet have this managed disconnect flow.

</details>

<details>
<summary><strong>Can I use a local model only?</strong></summary>

Yes. Add an Ollama or another loopback OpenAI-compatible provider, mark it as local, and enable strict local routing. Cloud fallback is a separate explicit option.

</details>

## Documentation

- [Agent setup guides](docs/guides/) (English)

## Development

Requirements: Rust 1.95 or newer, Node.js 22.23.1, npm, and the `wasm32-wasip2` Rust target. Platform-specific Tauri system dependencies are documented in the development guide.

```bash
git clone https://github.com/ballast-ai/token-station.git
cd token-station
rustup target add wasm32-wasip2
npm --prefix apps/desktop ci
npm --prefix apps/desktop run tauri:dev
```

Use the repository's `tauri:dev` command instead of invoking Tauri directly. It builds and embeds the five official WASM adapters required by the desktop gateway.

<details>
<summary><strong>Quality gates</strong></summary>

```bash
scripts/check-rust-format.sh
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
npm --prefix apps/desktop run test:coverage
npm --prefix apps/desktop run build
```

</details>

## Contributing

Issues and focused pull requests are welcome. Read the contribution guide first. User-visible UI, interaction, state, contract, or release behavior changes require a design document under `docs/design/` before tests and implementation.

## Project status

Token Station is an early public release. The local gateway, desktop control plane, two routing modes, built-in agent connectors, provider catalog, usage views, and recovery paths are implemented. Distribution is narrower than the source compatibility matrix: the current public desktop release is macOS Apple Silicon only, and the DMG is not yet Apple-signed or notarized.

## License

Licensed under the [Apache License 2.0](LICENSE).

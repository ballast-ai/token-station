# Secure OpenClaw Connection Guide

Token Station Desktop can automatically find and connect OpenClaw starting with
`0.1.0`. The built-in compatibility catalog currently has no version blocklist. It does
not reject a connection only because the version differs from one exact
version. This does not mean that every future version has passed real
acceptance. Paths, configuration structure, adapter readiness, and the internal
plan still fail closed before a write.

Official sources:

- [OpenClaw v2026.6.11](https://github.com/openclaw/openclaw/releases/tag/v2026.6.11)
- [JSON5 configuration and paths](https://docs.openclaw.ai/gateway/configuration)
- [Custom Provider fields](https://docs.openclaw.ai/gateway/config-tools)

## 1. Automatic discovery

The desktop app performs these read-only checks:

- Executable: `openclaw`. Version command: `openclaw --version`.
- Explicit path: `OPENCLAW_CONFIG_PATH`.
- State directory: `OPENCLAW_STATE_DIR/openclaw.json`.
- Default path: `~/.openclaw/openclaw.json`.
- Compatible environments: macOS, Linux, Windows, and WSL fixtures.

The scan does not run install, update, doctor, or repair. It does not create an
OpenClaw directory. It does not start Gateway. If the scan finds multiple
installations, select one target.

## 2. Connection changes

Select OpenClaw on the Agents page and select **One-click Connect**.
The desktop app creates an internal plan and writes it immediately. After the
first connection, it shows the key changes.

The Connector owns these paths:

```text
/models/providers/tokenstation
/agents/defaults/model/primary
```

The Connector writes this core structure:

```json5
{
  models: {
    providers: {
      tokenstation: {
        baseUrl: "http://127.0.0.1:8787/v1",
        apiKey: "<local virtual key>",
        api: "openai-completions",
        models: [{
          id: "auto",
          name: "Token Station Auto",
          reasoning: false,
          input: ["text"],
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
          contextWindow: 200000,
          maxTokens: 32000,
        }],
      },
    },
  },
  agents: {
    defaults: { model: { primary: "tokenstation/auto" } },
  },
}
```

Token Station does not own existing `channels`, Gateway, MCP, browser access,
skills, other providers, or other Agent defaults. The syntax-tree projection
preserves JSON5 comments, trailing commas, and unknown fields. It rejects
duplicate keys, invalid JSON5, and parent-type errors on owned paths before
writing.

The Connector also rejects the connection when the root object or an ancestor
of an owned path uses `$include`. Adding a sibling directly can change
OpenClaw include merge and override semantics. Do not risk configuration loss
until include-aware multi-file transactions are available.

## 3. Write and disconnect

The backend writes only when all conditions are true:

1. The Agent descriptor is admitted and its version is not in the compatibility blocklist.
2. The installation and configuration path still match the latest scan.
3. `agent-openai` is loaded in the Token Station runtime.
4. The internal plan, file revision, and confirmation token are still valid.

Selecting **One-click Connect** gives consent to write. The backend creates an AES-256-GCM encrypted snapshot.
It atomically replaces and reparses the configuration. It then runs a self-check
and commits ownership. The snapshot master key is stored in a private local
`snapshot-master.key`.

**Restore Official Configuration and Disconnect** removes only the two owned paths. It
preserves other fields that the user changed after connection. If the user or
another tool changes an owned value, Token Station refuses to overwrite it and requires a new scan.

Historical `openclaw.json.token-station.bak` files are read-only candidates.
Token Station does not overwrite, delete, or restore them automatically.

## 4. Behavior after an OpenClaw update

After an OpenClaw update, Token Station scans the version, paths, and
configuration structure again. An empty blocklist does not reject a connection
only because the version changed. The signed compatibility catalog can add an
explicit blocked range. The Connector still rejects an incompatible schema or
owned-path structure before writing. Token Station does not downgrade or
upgrade OpenClaw automatically. The catalog cannot deliver Connector code
remotely.

If a new version changes the `openclaw.json` schema, add an `openclaw-v2`
Connector. Keep old versions bound to `openclaw-v1`. Do not silently change
owned paths under the same Connector ID.

## 5. Protocol boundaries

OpenClaw `openai-completions` requests enter Token Station at
`/v1/chat/completions` and use `agent-openai`. Existing protocol regressions
cover text, streaming, and the main function-tool path. Gateway, remote
channels, MCP, browser access, user skills, and OpenClaw installation or upgrade
are outside this feature.

This connection does not change `crates/router-core/**`. The Router receives
normalized Canonical IR and has no special case for the name “OpenClaw.”

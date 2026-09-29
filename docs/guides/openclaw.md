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

The scanner supports native executables, standard Node package entries, Windows npm shims,
and literal shell launchers that pin an absolute Node path and an npm package entry.
It reads supported launchers without running a shell. The declared entry must match the package's `bin` field.
An optional `export PATH` line must prepend only the declared Node directory.
Additional commands, shell substitutions, and other environment overrides are not supported.

Each installation must pass its runtime, configuration, and compatibility checks before connection.
Selecting a path does not bypass these checks. If the selected installation fails, select another installation
or repair its launcher and runtime, then rescan. An unknown version does not mean that no installation is selected.

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
        auth: "api-key",
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

## Runtime credentials and authentication errors

After connection, restart the OpenClaw Gateway before testing a conversation.
Token Station sets explicit API key authentication for its provider. This prevents automatic selection of an old authentication profile.
An explicitly pinned session profile remains a user override. Remove that pin if the session must use the managed key.

OpenClaw can retain a separate key in each agent's `models.json`.
Token Station synchronizes existing tokenstation keys and URLs in these catalogs during connection.
The scan resolves runtime catalogs from `OPENCLAW_STATE_DIR`, independently from `OPENCLAW_CONFIG_PATH`.
Without a state override, it uses `.openclaw` under `OPENCLAW_HOME`, `HOME`, or `USERPROFILE`, in that order.
Custom `agentDir` paths beginning with `~` use the same effective home.
If only a legacy `.clawdbot` directory exists, set `OPENCLAW_STATE_DIR` explicitly and rescan.
Invalid or missing runtime path context blocks connection. Rescan after changing these environment settings.
It includes these fields in encrypted snapshots, revision checks, rollback, restore, and disconnect.
It preserves other providers and model definitions. It does not create missing catalogs or edit the authentication database.

Connection status also checks existing runtime catalogs. A matching main configuration alone cannot establish a valid connection.
If an older connection does not own these companion fields, disconnect and reconnect to establish their ownership.
If authentication fails, compare the active runtime credential with the managed credential without displaying either secret.
A listener on port 8787 does not prove that authentication succeeds.

# Secure Hermes Agent Connection Guide

Token Station Desktop can automatically find and connect NousResearch Hermes
Agent. The built-in compatibility catalog currently has no version blocklist.
It does not reject all versions other than `hermes-agent 0.18.0`. This does
not mean that every future version has passed real acceptance. Configuration
structure, adapter readiness, and the internal plan still fail closed before
a write.

Official sources:

- [NousResearch/hermes-agent](https://github.com/NousResearch/hermes-agent)
- [v2026.7.1](https://github.com/NousResearch/hermes-agent/releases/tag/v2026.7.1)
- [Configuration](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/configuration.md)
- [Provider runtime](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/developer-guide/provider-runtime.md)

## 1. Automatic discovery

The desktop app performs these read-only checks:

- Executable: `hermes`. Version command: `hermes version`.
- It normalizes version output to the first package SemVer.
- Environment override: `HERMES_HOME/config.yaml`.
- Default path: `~/.hermes/config.yaml` on macOS, Linux, and WSL.
  The Windows path is `%LOCALAPPDATA%/hermes/config.yaml`.
- Configuration format: one YAML document.

The scan does not run setup, update, doctor, migrate, or repair. It does not
install or upgrade Hermes. It does not start the Hermes gateway. If the scan
finds multiple installations, select one target.

## 2. Connection changes

`hermes-v1` owns only these five scalar paths:

```text
/model/default
/model/provider
/model/base_url
/model/api_key
/model/api_mode
```

When you select **One-click Connect**, the desktop app creates an internal plan
and writes it immediately. After the first connection, it shows the key
changes. The target structure is:

```yaml
model:
  default: auto
  provider: custom
  base_url: http://127.0.0.1:8787/v1
  api_key: <local virtual key>
  api_mode: chat_completions
```

This structure follows the official Hermes custom OpenAI-compatible endpoint
contract. Requests enter Token Station at `/v1/chat/completions` and use
`agent-openai`. Token Station does not own `display`, terminal, gateway,
tools, skills, memory, other providers, or undeclared fields under `model`.

Hermes recommends `.env` for normal long-term keys. This Connector writes a
local Token Station virtual key. It manages one YAML transaction to avoid a
non-atomic update across `config.yaml` and `.env`. It declares
`model.api_key` as sensitive. IPC and the post-write change summary show only
a redacted value. A private local `snapshot-master.key` protects the encrypted
snapshot of the original configuration. The secure writer normalizes new-file
permissions. Consider a dedicated environment variable only after multi-file
atomic transactions are available.

## 3. YAML safety boundaries

Token Station does not deserialize and rewrite the full YAML file. The lossless
CST changes only owned paths. It preserves:

- top-level, inline, and unrelated-field comments
- unknown fields, field order, whitespace, and untouched scalar styles
- user changes to unowned fields after connection

Reject the write before it starts if the YAML is invalid. Also reject multiple
YAML documents, duplicate keys, merge keys, a non-object root, a non-object
`model`, or a parent-type conflict on an owned path. Error messages do not
include original configuration lines or the virtual key.

## 4. Write and disconnect

The backend writes only when all conditions are true. The Agent descriptor must
be admitted and outside the blocklist. The installation must be unique. The
configuration fingerprint must be unchanged. `agent-openai` must be ready.
The internal plan and confirmation token must still be valid.

The backend then runs this sequence:

```text
encrypted snapshot → revision check → same-directory atomic replacement → YAML reparse → Connector self-check → ownership commit
```

Selecting **One-click Connect** gives consent to write. **Restore Official
Configuration and Disconnect** removes only the five owned paths.
If another tool changes an owned value, revision or ownership checks invalidate
the old plan. Scan again. Historical `.bak` files are read-only candidates.
Token Station does not overwrite, delete, or restore them automatically.

## 5. Behavior after a Hermes update

After a Hermes update, Token Station scans the version, paths, and configuration
structure again. An empty blocklist does not reject a connection only because
the version changed. The signed compatibility catalog can add an explicit
blocked range. The Connector still rejects an incompatible YAML schema or
owned-path structure before writing.

To expand real acceptance, verify the official tag, Provider runtime, version
output, lossless fixtures, and isolated E2E. If owned paths or protocols change,
add `hermes-v2`. Do not change `hermes-v1` silently.

Token Station does not run `hermes update` automatically. It does not change
`crates/router-core/**` to pass acceptance. cc-Switch is a competitor
reference, not a source for the Hermes product contract.

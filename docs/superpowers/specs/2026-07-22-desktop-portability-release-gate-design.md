# Desktop App Portability and Release Gate Design

**Date:** 2026-07-22
**Status:** Approved for implementation
**Scope:** `apps/desktop/src-tauri/`, desktop release scripts, and desktop CI

## Goal

Make the packaged token-station desktop application independent of the source checkout and usable on a clean computer. A release artifact must:

1. read and write configuration and runtime data only in OS application directories;
2. contain the four official adapter packages without requiring `plugins-dist/` beside the app;
3. have an auditable macOS/Windows signing path and reject incomplete release artifacts;
4. preserve the existing gateway, configuration validation, keychain, Agent snapshot, transaction, and rollback behavior.

The immediate local installation on the author's Mac will also receive a one-time copy of the current repository-root configuration and data. That copy is an installation operation, not a legacy path embedded in product code. The original files remain untouched until the new installation has been verified.

## Non-goals

- Do not teach the packaged app to search for a source checkout or a particular developer's home directory.
- Do not make bundled resources writable.
- Do not weaken configuration validation when migrating or loading data.
- Do not store signing certificates, notarization credentials, or passwords in the repository.
- Do not redesign the CLI release or plugin trust model.

## Runtime path model

Introduce one resolved desktop path value created inside Tauri's `setup` callback:

```text
DesktopPaths
├── config_file  = app_config_dir/token-station.json
├── data_dir     = app_data_dir/token-station-data
├── plugins_dir  = app_data_dir/plugins
└── agent roots  = app_data_dir/agent-integration/{snapshots,ownership}
```

`config_file` and `data_dir` remain conceptually distinct even on platforms where Tauri resolves their parent directories to the same location. `plugins_dir` is a writable extension directory for user-installed packages; the official packages do not depend on it because they are compiled into the release binary.

The existing `repo_root()` and all production use of `env!("CARGO_MANIFEST_DIR")` are removed. Template generation accepts resolved data and plugin paths explicitly. Relative paths loaded from an existing configuration are anchored to the configuration file's parent, never to a source tree.

### Startup ordering

The current application state is constructed before `tauri::Builder` exists, which prevents use of `app.path()`. Initialization moves into `setup`:

1. resolve Tauri `app_config_dir()` and `app_data_dir()`;
2. create the required application-specific directories;
3. load and validate the config from `config_file`, or create an in-memory template;
4. construct and `app.manage(...)` the main state;
5. construct the existing Agent integration state from the same `app_data_dir`;
6. finish setup and expose commands.

Failure to resolve or create these directories aborts startup with a path-specific error. An existing unreadable or invalid config retains the current read-only protection and is never overwritten by a template.

Tests continue to construct state from explicit temporary paths. Path selection and draft anchoring are separated from Tauri so their behavior can be covered without launching a GUI.

## Official plugin embedding

The desktop crate exposes an artifact-assembly feature that enables `token-station-cli/builtin-plugins`. Development and ordinary unit tests can keep the feature disabled; official desktop builds must use it.

A desktop release wrapper performs the required ordered build:

1. compile the four official WASI plugins;
2. stage each `manifest.json` and `adapter.wasm` under a temporary/staging `plugins-dist`;
3. set `TOKEN_STATION_PLUGINS_DIST` to that staging directory;
4. invoke `tauri build` with the desktop builtin-plugin feature enabled;
5. inspect the resulting package before it is accepted.

This reuses the existing CLI `builtin-plugins` loader and trust semantics. Builtin packages remain trusted, cannot be shadowed by a local package, and are loaded from signed executable bytes. The writable `plugins_dir` continues to support optional third-party packages and receipts; a missing directory is valid.

Direct release packaging that does not use the wrapper is unsupported and is made visible through documentation and CI. The artifact audit proves that all four official packages are present, so a successful frontend/Rust compilation alone cannot be mistaken for a releasable desktop build.

## Signing, notarization, and publishing

Desktop packaging is a separate job from the existing deterministic CLI tarball workflow. It uses platform-native runners and secrets supplied only by the CI secret store.

### macOS

- Build Apple Silicon and Intel artifacts on macOS runners.
- Import a Developer ID Application certificate from CI secrets.
- Pass the signing identity through `APPLE_SIGNING_IDENTITY`.
- Authenticate notarization with App Store Connect API credentials or Apple ID app-specific credentials supported by Tauri.
- Verify the app and installer with `codesign --verify --deep --strict`, Gatekeeper assessment, and notarization/stapling checks before upload.

For local engineering tests without credentials, the wrapper may produce an ad-hoc-signed artifact. Such an artifact is clearly labelled as local-only and must never satisfy the production release gate.

### Windows

- Build the Windows installer on a Windows runner.
- Obtain the signing certificate/tool configuration from CI secrets or the selected external signing service.
- Verify the produced executable and installer signature before upload.

The Windows job can be implemented structurally before credentials exist, but production publishing remains blocked until a valid signature is observed. Missing secrets must fail a release job rather than silently producing an unsigned public artifact.

## Artifact release gate

The desktop artifact audit is the authoritative packaging check. It rejects an artifact when any of the following is true:

- executable strings contain the checkout's absolute path or `CARGO_MANIFEST_DIR` value;
- any official builtin plugin manifest/adapter is absent;
- the app starts with config/data/plugin paths outside the Tauri application directories;
- macOS production artifacts fail signature, Gatekeeper, notarization, or stapling verification;
- Windows production artifacts lack a valid code signature;
- the package cannot pass a clean-home startup smoke test.

Rust path tests use temporary directories and cover:

- a fresh install template;
- relative path anchoring to the config directory;
- invalid existing config entering read-only protection;
- writable data/plugin directory creation;
- no repository-root fallback.

Existing desktop Rust tests, clippy, TypeScript checking, and Vitest remain mandatory. The plugin-enabled artifact build additionally exercises the builtin registry tests.

## One-time local migration

After the new app is built and before it is launched for the first time on this Mac:

1. stop the currently running old app;
2. resolve the new standard application directories from the same Tauri identifier;
3. back up any pre-existing destination files;
4. copy the current repository-root `token-station.json`, `token-station-data`, and relevant writable plugin receipts/configuration into their new destinations;
5. do not copy `plugins-dist`, because official packages are embedded and third-party plugins require an explicit trust-preserving install;
6. install and launch the new app;
7. verify configuration loading, provider visibility, plugin registry contents, gateway startup, and one routed request;
8. retain the source-root files and backup until the user separately authorizes deletion.

The migration never modifies the copied configuration to bypass validation. If absolute legacy paths are present inside the config, only the known desktop-owned `data.dir` and `plugins.dir` fields are rewritten to the resolved destinations; all other content is preserved and revalidated.

## Failure handling and rollback

- Startup path/config failures leave existing files unchanged and produce a clear error.
- Plugin build or artifact inspection failure prevents packaging.
- Signing or notarization failure prevents publishing.
- Local migration uses copy plus backup, not move or delete.
- If the new app fails real testing, remove only the new app installation and restore the destination backup; the repository-root data remains available.

## External prerequisites

The code, build wrapper, CI structure, and local ad-hoc package can be completed without signing credentials. A distributable macOS release still requires an Apple Developer ID certificate plus notarization credentials. A distributable Windows release requires an accepted Windows code-signing identity or service. These are deployment credentials, not code changes.

## Reference

- [Tauri application-specific file-system directories](https://v2.tauri.app/plugin/file-system/)
- [Tauri macOS signing and notarization](https://v2.tauri.app/zh-cn/distribute/sign/macos/)
- [Tauri GitHub Actions distribution](https://v2.tauri.app/zh-cn/distribute/pipelines/github/)

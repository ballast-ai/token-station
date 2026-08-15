# CI Failure Recovery

## Problem, goal, scope, and non-goals

The `main` branch has five failed CI jobs at commit `8f411c5`.
The failures come from stale CI metadata, a missing security ledger, platform-specific test code, and timing-sensitive tests.

This urgent fix restores deterministic CI checks without changing product behavior.
It does not change routing, desktop interfaces, public contracts, or release artifacts.

## Security and data boundaries

Keep the frozen Router tree unchanged.
Record the existing `RUSTSEC-2024-0429` exception instead of weakening `cargo audit`.
Limit the exception to the Linux Tauri GTK dependency graph.
Do not add credentials, user data, network writes, or release permissions.

## Expected behavior and failure handling

The Rust format gate must compare the Router tree with the current reviewed tree.
The RustSec ledger gate must validate a committed, time-bounded exception record.
Windows Clippy must compile test targets without unused imports.
Coverage tests must exclude dependency preparation from bounded lifecycle timing.
Frontend tests must wait for asynchronous UI updates and allow a loaded CI runner to finish valid interactions.
Test-local IPC mocks must also tolerate the app's background runtime poll unless a test explicitly exercises poll failure behavior.
Proxy tests must wait on the asynchronous metrics write condition instead of assuming a fixed delay is sufficient on Windows.
Cursor database tests must close SQLite handles before deleting their temporary directory on Windows.

## Accessibility and responsive behavior

This fix does not change rendered UI, keyboard behavior, responsive layout, or accessibility semantics.
Frontend assertions continue to use public roles, labels, and status regions.

## Regression boundaries and acceptance criteria

- Run `scripts/check-rust-format.sh`.
- Run `node scripts/check-rustsec-exceptions.mjs`.
- Run the four affected frontend tests with coverage.
- Run the full frontend coverage suite and build.
- Run the affected desktop Rust lifecycle test.
- Run desktop Rust format, Clippy, and tests.
- Verify the Windows-only import with a Windows-target compile check or the equivalent `cfg` boundary review.

CI passes when all five failed jobs have a deterministic local counterpart and no security gate is disabled.

## Implementation locations and release requirements

Update `scripts/check-rust-format.sh` and add `docs/security/rustsec-exceptions.json`.
Update test-only code in `apps/desktop/src/App.test.tsx`, `apps/desktop/src-tauri/src/cursor_tunnel.rs`, `apps/desktop/src-tauri/src/lib.rs`, and `apps/cli/tests/proxy.rs`.

No local App installation is required because this fix changes only tests, CI metadata, and security evidence.
No release action is authorized by this record.

## Implementation status

Implementation is complete.

The Rust format gate and RustSec ledger gate pass.
The frontend suite passes 413 tests with 86.18 percent line coverage.
The frontend production build passes.
The shared frontend IPC test fixture preserves the latest runtime returned by state-changing IPC and supplies it only when a test-local mock rejects the background `get_runtime_state` command as unexpected.
Runtime polling tests that return a state or a deliberate failure keep their explicit behavior.
The stopped-runtime cache test now keeps its polled runtime in sync with the emitted lifecycle event.
The proxy metrics helper retries `QueryReturnedNoRows` for up to five seconds, replacing the Windows-sensitive assumption that a 300 ms delay always completes persistence; redirect-attempt assertions wait for the parent receipt transaction before reading child rows.
The Cursor restore test drops its final SQLite connection before temporary-directory cleanup, matching Windows file-lock semantics.
Desktop Clippy passes with warnings denied.
The desktop Rust suite passes 365 tests, with 2 explicit ignores.
The exact desktop `cargo llvm-cov` job command passes and writes the LCOV report.

The Windows cross-target check cannot run on this macOS host because third-party C dependencies require the Windows SDK.
The Windows-only unused import is fixed by matching the import to its Unix-only test.
Run the native Windows job after an authorized push.

No product behavior changed, so the local desktop App was not replaced.

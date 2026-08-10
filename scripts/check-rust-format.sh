#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly frozen_router_tree="4d08bff5d8ad44ed007f7bc7b086e62d6d5a92e4"
readonly workspace_packages=(
  token-station-cli
  token-station-conformance
  token-station-plugin-api
  token-station-protocol
  token-station-metrics
  token-station-plugin-runtime
  token-station-private-fs
  token-station-release
)
readonly plugin_manifests=(
  plugins/official/agent-anthropic/Cargo.toml
  plugins/official/agent-openai/Cargo.toml
  plugins/official/agent-openai-responses/Cargo.toml
  plugins/official/agent-gemini/Cargo.toml
  plugins/official/provider-openai-compatible/Cargo.toml
)

cd "$root"

router_changes="$(git status --porcelain=v1 --untracked-files=all -- crates/router-core)"
[[ -z "$router_changes" ]] || {
  echo "crates/router-core is frozen and has worktree changes:" >&2
  echo "$router_changes" >&2
  exit 1
}
actual_router_tree="$(git rev-parse HEAD:crates/router-core)"
[[ "$actual_router_tree" == "$frozen_router_tree" ]] || {
  echo "crates/router-core tree changed: expected $frozen_router_tree, got $actual_router_tree" >&2
  exit 1
}

format_arguments=(fmt --check)
for package in "${workspace_packages[@]}"; do
  format_arguments+=(-p "$package")
done
cargo "${format_arguments[@]}"

for manifest in "${plugin_manifests[@]}"; do
  cargo fmt --manifest-path "$manifest" -- --check
done
cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml -- --check

echo "Rust format and frozen Router tree: PASS"

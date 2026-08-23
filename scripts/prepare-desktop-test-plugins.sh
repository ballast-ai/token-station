#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly target="wasm32-wasip2"
readonly output="$root/plugins-dist"
readonly plugins=(
  agent-openai
  agent-anthropic
  agent-openai-responses
  agent-gemini
)
# The South provider components ship beside the agents under the South
# loader's file name (`component.wasm`); the directory scan dispatches on
# each manifest's api_version.
readonly south_components=(
  provider-openai-compatible-v2
  provider-anthropic-v2
)

# Desktop startup tests exercise the real Gateway, whose development config
# resolves official adapters from the repository-level plugins-dist directory.
# Start from empty. A dist carrying packages an earlier revision staged — a v1
# provider manifest, say — makes the host fail startup on a package this tree no
# longer builds, and the error names the stale file rather than the stale dist.
rm -rf "$output"

for plugin in "${plugins[@]}"; do
  source="$root/plugins/official/$plugin"
  cargo build --locked --release --manifest-path "$source/Cargo.toml" --target "$target"
  mkdir -p "$output/$plugin"
  cp "$source/manifest.json" "$output/$plugin/manifest.json"
  cp "$source/target/$target/release/${plugin//-/_}.wasm" \
    "$output/$plugin/adapter.wasm"
done

for component in "${south_components[@]}"; do
  source="$root/plugins/official/$component"
  cargo build --locked --release --manifest-path "$source/Cargo.toml" --target "$target"
  mkdir -p "$output/$component"
  cp "$source/manifest.json" "$output/$component/manifest.json"
  cp "$source/target/$target/release/${component//-/_}.wasm" \
    "$output/$component/component.wasm"
done

echo "desktop test plugins: PASS ($((${#plugins[@]} + ${#south_components[@]})) packages in $output)"

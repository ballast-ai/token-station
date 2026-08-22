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
  provider-openai-compatible
)
# The v2 south component ships under the south loader's file name and never
# enters the v1 registry: it is staged under the south/ subtree, which the v1
# directory scan skips (no manifest.json at its top level).
readonly south_components=(
  provider-openai-compatible-v2
  provider-anthropic-v2
)

# Desktop startup tests exercise the real Gateway, whose development config
# resolves official adapters from the repository-level plugins-dist directory.
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
  mkdir -p "$output/south/$component"
  cp "$source/manifest.json" "$output/south/$component/manifest.json"
  cp "$source/target/$target/release/${component//-/_}.wasm" \
    "$output/south/$component/component.wasm"
done

echo "desktop test plugins: PASS ($((${#plugins[@]} + ${#south_components[@]})) packages in $output)"

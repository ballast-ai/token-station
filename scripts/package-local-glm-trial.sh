#!/usr/bin/env bash
# Build a distributable local GLM trial package without accepting or writing API keys.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
binary="$root/target/release/token-station-cli"
agent_wasm="$root/plugins/official/agent-openai/target/wasm32-wasip2/release/agent_openai.wasm"
provider_wasm="$root/plugins/official/provider-openai-compatible/target/wasm32-wasip2/release/provider_openai_compatible.wasm"
output="$root/dist"

usage() {
  echo "usage: $0 [--binary PATH] [--agent-wasm PATH] [--provider-wasm PATH] [--output DIR]" >&2
}

while (($#)); do
  case "$1" in
    --binary) binary=${2:?missing value for --binary}; shift 2 ;;
    --agent-wasm) agent_wasm=${2:?missing value for --agent-wasm}; shift 2 ;;
    --provider-wasm) provider_wasm=${2:?missing value for --provider-wasm}; shift 2 ;;
    --output) output=${2:?missing value for --output}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage; exit 2 ;;
  esac
done

for file in "$binary" "$agent_wasm" "$provider_wasm"; do
  test -f "$file" || { echo "required artifact not found: $file" >&2; exit 1; }
done

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/apps/cli/Cargo.toml" | head -n 1)
test -n "$version"
platform="$(uname -s | tr '[:upper:]' '[:lower:]')-$(uname -m)"
package="$output/token-station-glm-trial-$version-$platform"
rm -rf "$package"
mkdir -p "$package/plugins-dist/agent-openai" "$package/plugins-dist/provider-openai-compatible"

cp "$binary" "$package/token-station-cli"
cp "$root/plugins/official/agent-openai/manifest.json" "$package/plugins-dist/agent-openai/manifest.json"
cp "$agent_wasm" "$package/plugins-dist/agent-openai/adapter.wasm"
cp "$root/plugins/official/provider-openai-compatible/manifest.json" "$package/plugins-dist/provider-openai-compatible/manifest.json"
cp "$provider_wasm" "$package/plugins-dist/provider-openai-compatible/adapter.wasm"
cp "$root/apps/cli/glm-5.2-local-config.json" "$package/token-station.json"
cp "$root/docs/guides/GLM-5.2-本地启动指南.md" "$package/GLM-5.2-本地启动指南.md"
cp "$root/LICENSE" "$package/LICENSE"
chmod +x "$package/token-station-cli"

echo "created: $package"

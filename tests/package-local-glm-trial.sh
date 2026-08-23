#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

bin="$work/token-station-cli"
agent="$work/agent-openai.wasm"
provider="$work/provider-openai-compatible.wasm"
printf '#!/bin/sh\nexit 0\n' > "$bin"
chmod +x "$bin"
printf 'agent' > "$agent"
printf 'provider' > "$provider"

output="$work/output"
"$root/scripts/package-local-glm-trial.sh" \
  --binary "$bin" \
  --agent-wasm "$agent" \
  --provider-wasm "$provider" \
  --output "$output"

package=$(find "$output" -maxdepth 1 -type d -name 'token-station-glm-trial-*' | head -n 1)
test -n "$package"
test -x "$package/token-station-cli"
test -f "$package/plugins-dist/agent-openai/manifest.json"
test -f "$package/plugins-dist/agent-openai/adapter.wasm"
test -f "$package/plugins-dist/provider-openai-compatible-v2/manifest.json"
test -f "$package/plugins-dist/provider-openai-compatible-v2/component.wasm"
test -f "$package/token-station.json"
test -f "$package/GLM-5.2-本地启动指南.md"
grep -Fq 'GLM_API_KEY' "$package/token-station.json"
grep -Fq 'https://api.z.ai/api/coding/paas/v4' "$package/token-station.json"
! grep -Fq 'YOUR_API_KEY' "$package/token-station.json"

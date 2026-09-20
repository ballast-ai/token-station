#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
: "${TOKEN_STATION_TASK_BAILIAN_PACKAGE_DIR:?set the official Bailian task-v2 package directory}"
for file in manifest.json component.wasm; do
  [[ -f "$TOKEN_STATION_TASK_BAILIAN_PACKAGE_DIR/$file" ]] || {
    echo "required task package file is missing: $file" >&2
    exit 2
  }
done
cd "$root"
# Each real subprocess scenario enforces its own 55-second deadline and reaps its child.
rustup run 1.96.0 cargo test --locked -p token-station-cli --test task_cli -- --include-ignored --test-threads=1
rustup run 1.96.0 cargo test --locked -p token-station-cli --lib tasks::

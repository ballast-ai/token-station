#!/usr/bin/env bash
# 构建时间单列；优化 FFI 测试进程最多运行 55 秒。
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
build_log="$(mktemp)"
trap 'rm -f "$build_log"' EXIT
cargo test --locked --release --lib --no-run --message-format=json \
  --manifest-path "$root/scripts/fixtures/glib-variant-regression/Cargo.toml" \
  --target-dir "${GLIB_REGRESSION_TARGET_DIR:-$root/target/glib-regression}" > "$build_log"
python3 - "$build_log" <<'PY'
import json, subprocess, sys
executables = []
for line in open(sys.argv[1]):
    item = json.loads(line)
    if item.get('reason') == 'compiler-artifact' and item.get('executable') and item.get('target', {}).get('name') == 'glib_variant_regression':
        executables.append(item['executable'])
if len(executables) != 1:
    raise SystemExit('expected exactly one glib regression test executable')
try:
    result = subprocess.run([executables[0], '--test-threads=1'], timeout=55)
except subprocess.TimeoutExpired:
    raise SystemExit('glib regression exceeded 55 seconds')
raise SystemExit(0 if result.returncode == 0 else 1)
PY

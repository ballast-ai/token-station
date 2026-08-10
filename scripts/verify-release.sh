#!/usr/bin/env bash
# Verify that official artifacts were built from public source (C1#7 release gate). Usage:
#
#   git checkout v<version>
#   scripts/verify-release.sh <target-triple> <official.tar.gz>
#
# First compare archive bytes. If tar or gzip environment differences prevent a
# match, compare each archived file's SHA-256. The latter proves binaries came
# from this source. Report a mismatch only when both checks fail.

set -euo pipefail

TARGET=${1:?usage: scripts/verify-release.sh <target-triple> <official.tar.gz>}
OFFICIAL=${2:?usage: scripts/verify-release.sh <target-triple> <official.tar.gz>}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

sha() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

scripts/build-release.sh "$TARGET"
REBUILT=$(ls dist/token-station-cli-*-"${TARGET}".tar.gz)

if [ "$(sha "$REBUILT")" = "$(sha "$OFFICIAL")" ]; then
  echo "ok: archive is byte-identical ($(sha "$OFFICIAL"))"
  exit 0
fi

echo "archive bytes differ; comparing contained files instead" >&2
WORK=$(mktemp -d)
mkdir -p "$WORK/official" "$WORK/rebuilt"
tar -xzf "$OFFICIAL" -C "$WORK/official"
tar -xzf "$REBUILT" -C "$WORK/rebuilt"

STATUS=0
(cd "$WORK/official" && find . -type f | sort) > "$WORK/official.list"
(cd "$WORK/rebuilt" && find . -type f | sort) > "$WORK/rebuilt.list"
diff "$WORK/official.list" "$WORK/rebuilt.list" || STATUS=1

while IFS= read -r file; do
  if [ "$(sha "$WORK/official/$file")" != "$(sha "$WORK/rebuilt/$file")" ]; then
    echo "MISMATCH: $file" >&2
    STATUS=1
  fi
done < "$WORK/official.list"

rm -rf "$WORK"
if [ "$STATUS" = 0 ]; then
  echo "ok: every contained file is byte-identical (archive envelope differed)"
else
  echo "FAIL: rebuilt files do not match the official release" >&2
fi
exit "$STATUS"

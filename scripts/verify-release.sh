#!/usr/bin/env bash
# 验证官方产物确实由公开源码编出（C1#7 出口标准）。用法：
#
#   git checkout v<version>
#   scripts/verify-release.sh <target-triple> <official.tar.gz>
#
# 第一道比对是归档字节；归档层若因 tar/gzip 环境差异不一致，退到第二道：
# 逐个比对归档内文件的 SHA-256——那才是「二进制由此源码编出」的实质命题。
# 两道都过不了才是不一致。

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

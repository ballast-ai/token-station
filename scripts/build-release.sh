#!/usr/bin/env bash
# Reproducible build recipe (C1#7). Usage:
#
#   scripts/build-release.sh <target-triple>
#
# This script is the recipe: rerunning it from a release tag should produce
# byte-identical artifacts. Each element removes one source of nondeterminism:
#
#   - RELEASE_TOOLCHAIN pins the compiler. rust-toolchain.toml tracks stable for
#     development, while releases require an exact version.
#   - --locked pins every dependency version through the committed Cargo.lock.
#   - --remap-path-prefix rewrites build and cargo-home paths to fixed values.
#   - SOURCE_DATE_EPOCH comes from the release commit rather than the wall clock.
#   - GNU tar ustar format, fixed order, owner, timestamps, and gzip -n make the
#     archive deterministic.
#
# Output goes to dist/: a tar.gz containing the CLI binary, five official plugin
# packages, example configuration, and LICENSE.
#
# Official binaries embed official plugins in the builtin tier from architecture
# section 12.1. Build the five WASM plugins first, then build the CLI with
# builtin-plugins and TOKEN_STATION_PLUGINS_DIST. include_bytes! embeds plugin
# bytes so the standalone binary needs no installation. The tarball also carries
# plugins-dist; registry prefers builtin for the same dialect, so duplication is
# harmless. Embedded content, not paths, preserves reproducibility.

set -euo pipefail

TARGET=${1:?usage: scripts/build-release.sh <target-triple>}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

: "${TOKEN_STATION_RELEASE_PUBKEY_HEX:?正式 CLI 构建缺少 TOKEN_STATION_RELEASE_PUBKEY_HEX 公钥}"
if [[ ! "$TOKEN_STATION_RELEASE_PUBKEY_HEX" =~ ^[0-9a-f]{64}$ ]]; then
  echo "正式 CLI 构建的发布公钥必须是 64 位小写十六进制字符。" >&2
  exit 1
fi
export TOKEN_STATION_RELEASE_PUBKEY_HEX

# Release toolchain: changing this changes the recipe and reproducible baseline and must be announced.
RELEASE_TOOLCHAIN=1.96.0

VERSION=$(grep -m1 '^version' apps/cli/Cargo.toml | cut -d'"' -f2)
SOURCE_DATE_EPOCH=$(git log -1 --pretty=%ct)
export SOURCE_DATE_EPOCH
export RUSTFLAGS="--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=${ROOT}=/build"

rustup toolchain install "$RELEASE_TOOLCHAIN" --profile minimal >/dev/null
rustup target add --toolchain "$RELEASE_TOOLCHAIN" "$TARGET" wasm32-wasip2 >/dev/null

for plugin in agent-openai agent-anthropic agent-openai-responses agent-gemini provider-openai-compatible; do
  (cd "plugins/official/${plugin}" \
    && cargo "+${RELEASE_TOOLCHAIN}" build --locked --release --target wasm32-wasip2)
done

NAME="token-station-cli-${VERSION}-${TARGET}"
STAGE="dist/${NAME}"
rm -rf "$STAGE"
mkdir -p "$STAGE/plugins-dist"

for plugin in agent-openai agent-anthropic agent-openai-responses agent-gemini provider-openai-compatible; do
  mkdir -p "$STAGE/plugins-dist/${plugin}"
  cp "plugins/official/${plugin}/manifest.json" "$STAGE/plugins-dist/${plugin}/"
  cp "plugins/official/${plugin}/target/wasm32-wasip2/release/${plugin//-/_}.wasm" \
     "$STAGE/plugins-dist/${plugin}/adapter.wasm"
  cp -R "plugins/official/${plugin}/fixtures" "$STAGE/plugins-dist/${plugin}/fixtures"
done

echo "building token-station-cli ${VERSION} for ${TARGET} (rust ${RELEASE_TOOLCHAIN})" >&2
TOKEN_STATION_PLUGINS_DIST="${ROOT}/${STAGE}/plugins-dist" \
  cargo "+${RELEASE_TOOLCHAIN}" build --locked --release --target "$TARGET" \
  -p token-station-cli --features builtin-plugins

cp "target/${TARGET}/release/token-station-cli" "$STAGE/"
cp apps/cli/example-config.json LICENSE "$STAGE/"

# Deterministic archives require GNU tar; on macOS use Homebrew gtar because bsdtar lacks --sort and --mtime.
TAR=tar
if ! tar --version 2>/dev/null | grep -q "GNU tar"; then
  TAR=gtar
  command -v gtar >/dev/null || { echo "GNU tar required (brew install gnu-tar)" >&2; exit 1; }
fi
"$TAR" --format=ustar --sort=name --mtime="@${SOURCE_DATE_EPOCH}" \
  --owner=0 --group=0 --numeric-owner \
  -C dist -cf - "$NAME" | gzip -n > "dist/${NAME}.tar.gz"
rm -rf "$STAGE"

if command -v sha256sum >/dev/null; then
  sha256sum "dist/${NAME}.tar.gz"
else
  shasum -a 256 "dist/${NAME}.tar.gz"
fi

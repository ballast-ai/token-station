#!/usr/bin/env bash
# Reproducible build recipe (C1#7). Usage:
#
#   scripts/build-release.sh <target-triple>
#
# This script is the recipe: rerunning it from a release tag should produce
# byte-identical artifacts. Each element removes one source of nondeterminism:
#
#   RELEASE_TOOLCHAIN pins the compiler version. rust-toolchain.toml tracks stable.
#     That applies to development. A release requires an exact version. A version change changes the recipe.
#   `--locked` pins all dependency versions. Cargo.lock is committed.
#   `--remap-path-prefix` rewrites the build path and cargo home to fixed values.
#     Remove traces of who built in each directory.
#   SOURCE_DATE_EPOCH uses the release commit timestamp, not the wall clock.
#   GNU tar uses `--format=ustar`, fixed order, owner, and timestamp, plus `gzip -n`.
#     Remove nondeterminism from the archive layer.
#
# Artifacts go to dist/ as one tar.gz with the CLI binary and two official plugin packages.
# packages, example configuration, and LICENSE.
#
# Official binaries embed official plugins at the builtin tier (architecture §12.1). First build two WASM plugins,
# Then build the CLI with `--features builtin-plugins` and TOKEN_STATION_PLUGINS_DIST.
# `include_bytes!` embeds plugin bytes in the binary, so the bare binary needs no installation. The tarball still includes
# plugins-dist/ copy. The registry selects builtin for duplicate dialects, so duplicates are harmless. Plugin builds
# This must occur before the CLI build. The binary embeds file contents, not paths, so reproducibility is unchanged.

set -euo pipefail

TARGET=${1:?usage: scripts/build-release.sh <target-triple>}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

# Release toolchain: changing this changes the recipe and reproducible baseline and must be announced.
RELEASE_TOOLCHAIN=1.96.0

VERSION=$(grep -m1 '^version' apps/cli/Cargo.toml | cut -d'"' -f2)
SOURCE_DATE_EPOCH=$(git log -1 --pretty=%ct)
export SOURCE_DATE_EPOCH
export RUSTFLAGS="--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=${ROOT}=/build"

rustup toolchain install "$RELEASE_TOOLCHAIN" --profile minimal >/dev/null
rustup target add --toolchain "$RELEASE_TOOLCHAIN" "$TARGET" wasm32-wasip2 >/dev/null

for plugin in agent-openai provider-openai-compatible; do
  (cd "plugins/official/${plugin}" \
    && cargo "+${RELEASE_TOOLCHAIN}" build --locked --release --target wasm32-wasip2)
done

NAME="token-station-cli-${VERSION}-${TARGET}"
STAGE="dist/${NAME}"
rm -rf "$STAGE"
mkdir -p "$STAGE/plugins-dist"

for plugin in agent-openai provider-openai-compatible; do
  mkdir -p "$STAGE/plugins-dist/${plugin}"
  cp "plugins/official/${plugin}/manifest.json" "$STAGE/plugins-dist/${plugin}/"
  cp "plugins/official/${plugin}/target/wasm32-wasip2/release/${plugin//-/_}.wasm" \
     "$STAGE/plugins-dist/${plugin}/adapter.wasm"
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

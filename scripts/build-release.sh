#!/usr/bin/env bash
# 可复现构建配方（C1#7）。用法：
#
#   scripts/build-release.sh <target-triple>
#
# 这份脚本本身就是配方：任何人在发布 tag 的检出上重跑它，产物字节一致。
# 配方的组成部分，每一项都在消除一类非确定性：
#
#   - RELEASE_TOOLCHAIN 钉死编译器版本（rust-toolchain.toml 跟踪 stable，
#     那是开发口径；发布口径必须是精确版本——换版本就是换配方）；
#   - --locked 钉死全部依赖版本（Cargo.lock 已入库）；
#   - --remap-path-prefix 把构建路径与 cargo home 重写成固定值，
#     消除「谁在哪个目录编译」的痕迹；
#   - SOURCE_DATE_EPOCH 取自发布 commit 的时间戳，不取墙钟；
#   - GNU tar --format=ustar + 固定排序/属主/时间戳 + gzip -n，
#     消除归档层的非确定性。
#
# 产物落在 dist/：一个 tar.gz，内含 CLI 二进制、两个官方插件包
# （manifest.json + adapter.wasm）、示例配置与 LICENSE。

set -euo pipefail

TARGET=${1:?usage: scripts/build-release.sh <target-triple>}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

# 发布工具链：改这里 = 改配方 = 官方产物换了可复现基线，必须随发布说明公告。
RELEASE_TOOLCHAIN=1.96.0

VERSION=$(grep -m1 '^version' apps/cli/Cargo.toml | cut -d'"' -f2)
SOURCE_DATE_EPOCH=$(git log -1 --pretty=%ct)
export SOURCE_DATE_EPOCH
export RUSTFLAGS="--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=${ROOT}=/build"

rustup toolchain install "$RELEASE_TOOLCHAIN" --profile minimal >/dev/null
rustup target add --toolchain "$RELEASE_TOOLCHAIN" "$TARGET" wasm32-wasip2 >/dev/null

echo "building token-station-cli ${VERSION} for ${TARGET} (rust ${RELEASE_TOOLCHAIN})" >&2
cargo "+${RELEASE_TOOLCHAIN}" build --locked --release --target "$TARGET" -p token-station-cli

for plugin in agent-openai provider-openai-compatible; do
  (cd "plugins/official/${plugin}" \
    && cargo "+${RELEASE_TOOLCHAIN}" build --locked --release --target wasm32-wasip2)
done

NAME="token-station-cli-${VERSION}-${TARGET}"
STAGE="dist/${NAME}"
rm -rf "$STAGE"
mkdir -p "$STAGE/plugins-dist"

cp "target/${TARGET}/release/token-station-cli" "$STAGE/"
for plugin in agent-openai provider-openai-compatible; do
  mkdir -p "$STAGE/plugins-dist/${plugin}"
  cp "plugins/official/${plugin}/manifest.json" "$STAGE/plugins-dist/${plugin}/"
  cp "plugins/official/${plugin}/target/wasm32-wasip2/release/${plugin//-/_}.wasm" \
     "$STAGE/plugins-dist/${plugin}/adapter.wasm"
done
cp apps/cli/example-config.json LICENSE "$STAGE/"

# 确定性归档需要 GNU tar；macOS 上是 brew 的 gtar（bsdtar 无 --sort/--mtime）。
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

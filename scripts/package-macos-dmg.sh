#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "用法：scripts/package-macos-dmg.sh --app <app> --output <dmg> --volume-name <name> --signing-identity <identity> --version <x.y.z> --architecture <aarch64|x86_64>" >&2
  exit 2
}

app_path=""
output_path=""
volume_name=""
signing_identity=""
version=""
architecture=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --app) app_path=${2:-}; shift 2 ;;
    --output) output_path=${2:-}; shift 2 ;;
    --volume-name) volume_name=${2:-}; shift 2 ;;
    --signing-identity) signing_identity=${2:-}; shift 2 ;;
    --version) version=${2:-}; shift 2 ;;
    --architecture) architecture=${2:-}; shift 2 ;;
    *) usage ;;
  esac
done

[[ "$(uname -s)" == "Darwin" ]] || {
  echo "正式 macOS DMG 只能在 macOS 上创建。" >&2
  exit 1
}
[[ -n "$app_path" && -n "$output_path" && -n "$volume_name" && -n "$signing_identity" && -n "$version" && -n "$architecture" ]] || usage
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || usage
[[ "$architecture" == "aarch64" || "$architecture" == "x86_64" ]] || usage
[[ -d "$app_path" ]] || {
  echo "没有找到准备打包的 App：$app_path" >&2
  exit 1
}
[[ ! -e "$output_path" ]] || {
  echo "输出文件已经存在，为避免覆盖已停止：$output_path" >&2
  exit 1
}

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly expected_bundle_id="com.tokenstation.desktop"
readonly installer="$root/packaging/macos/安装 Token Station.command"
readonly readme="$root/packaging/macos/安装前必读.md"
readonly agent_rules="$root/packaging/macos/AGENTS.md"

actual_bundle_id=$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$app_path/Contents/Info.plist" 2>/dev/null || true)
[[ "$actual_bundle_id" == "$expected_bundle_id" ]] || {
  echo "准备打包的 App 不是 Token Station，已停止。" >&2
  exit 1
}
codesign --verify --deep --strict "$app_path" || {
  echo "准备打包的 App 代码签名无效，已停止。" >&2
  exit 1
}

output_parent=$(dirname "$output_path")
/bin/mkdir -p "$output_parent"
output_parent=$(cd "$output_parent" && pwd)
output_path="$output_parent/$(basename "$output_path")"

stage=$(mktemp -d "${TMPDIR:-/tmp}/token-station-dmg.XXXXXX")
readonly stage
publish_stage=$(mktemp -d "$output_parent/.token-station-dmg.XXXXXX")
readonly publish_stage
temporary_dmg="$publish_stage/$(basename "$output_path")"
readonly temporary_dmg
cleanup() {
  /bin/rm -rf -- "$stage"
  /bin/rm -rf -- "$publish_stage"
}
trap cleanup EXIT

/usr/bin/ditto "$app_path" "$stage/token-station.app"
ln -s /Applications "$stage/Applications"
/bin/cp "$readme" "$stage/安装前必读.md"
/bin/cp "$installer" "$stage/安装 Token Station.command"
/bin/chmod 755 "$stage/安装 Token Station.command"
/bin/cp "$agent_rules" "$stage/AGENTS.md"

hdiutil create -volname "$volume_name" -srcfolder "$stage" -format UDZO "$temporary_dmg"
codesign --force --timestamp --sign "$signing_identity" "$temporary_dmg"

: "${APPLE_API_ISSUER:?DMG 公证缺少 APPLE_API_ISSUER}"
: "${APPLE_API_KEY:?DMG 公证缺少 APPLE_API_KEY}"
: "${APPLE_API_KEY_PATH:?DMG 公证缺少 APPLE_API_KEY_PATH}"
xcrun notarytool submit "$temporary_dmg" --wait \
  --issuer "$APPLE_API_ISSUER" --key-id "$APPLE_API_KEY" --key "$APPLE_API_KEY_PATH"

xcrun stapler staple "$temporary_dmg"
xcrun stapler validate "$temporary_dmg"
"$root/scripts/audit-macos-dmg.sh" \
  --dmg "$temporary_dmg" \
  --expected-version "$version" \
  --expected-arch "$architecture"
/bin/mv -n "$temporary_dmg" "$output_path"
if [[ -e "$temporary_dmg" ]]; then
  echo "正式 DMG 输出路径已被占用，没有覆盖现有文件：$output_path" >&2
  exit 1
fi
shasum -a 256 "$output_path"
echo "正式 macOS DMG 已创建并通过审计：$output_path"

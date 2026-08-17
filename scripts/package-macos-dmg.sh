#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "用法：scripts/package-macos-dmg.sh --app <app> --output <dmg> --volume-name <name> --version <x.y.z> --architecture <aarch64|x86_64> [--signing-identity <identity> | --unsigned-test --app-source-commit <40位提交>]" >&2
  exit 2
}

app_path=""
output_path=""
volume_name=""
signing_identity=""
version=""
architecture=""
unsigned_test=false
app_source_commit=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --app) app_path=${2:-}; shift 2 ;;
    --output) output_path=${2:-}; shift 2 ;;
    --volume-name) volume_name=${2:-}; shift 2 ;;
    --signing-identity) signing_identity=${2:-}; shift 2 ;;
    --version) version=${2:-}; shift 2 ;;
    --architecture) architecture=${2:-}; shift 2 ;;
    --unsigned-test) unsigned_test=true; shift ;;
    --app-source-commit) app_source_commit=${2:-}; shift 2 ;;
    *) usage ;;
  esac
done

[[ "$(uname -s)" == "Darwin" ]] || {
  echo "macOS DMG 只能在 macOS 上创建。" >&2
  exit 1
}
[[ -n "$app_path" && -n "$output_path" && -n "$volume_name" && -n "$version" && -n "$architecture" ]] || usage
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
readonly readme="$root/packaging/macos/installation-guide.md"
readonly troubleshooting_guide="$root/macos-troubleshooting.md"
readonly unsigned_terminal_command="$root/packaging/macos/终端启动命令.txt"
readonly formal_finder_layout_template="$root/packaging/macos/dmg-layout.dsstore.base64"
readonly unsigned_finder_layout_template="$root/packaging/macos/dmg-layout-unsigned.dsstore.base64"
finder_layout_template="$formal_finder_layout_template"
packaging_source_commit=""
tag_commit=""

if [[ "$unsigned_test" == "true" ]]; then
  [[ -z "$signing_identity" ]] || {
    echo "未签名测试模式不接受签名身份，请移除 --signing-identity。" >&2
    exit 1
  }
  [[ "$app_source_commit" =~ ^[0-9a-f]{40}$ ]] || {
    echo "未签名测试模式需要 40 位 --app-source-commit。" >&2
    exit 1
  }
  expected_filename="token-station_${version}_${architecture}_UNSIGNED-UNNOTARIZED.dmg"
  [[ "$(basename "$output_path")" == "$expected_filename" ]] || {
    echo "测试 DMG 文件名必须明确标出未签名和未公证，应为：$expected_filename" >&2
    exit 1
  }
  tag_commit=$(git -C "$root" rev-parse "v${version}^{}" 2>/dev/null || true)
  [[ "$tag_commit" == "$app_source_commit" ]] || {
    echo "App 源码提交与 v${version} 标签不一致，已停止打包。" >&2
    exit 1
  }
  packaging_source_commit=$(git -C "$root" rev-parse HEAD)
  if ! git -C "$root" diff --quiet || ! git -C "$root" diff --cached --quiet; then
    echo "打包工作区有未提交改动，不能生成可发布测试 DMG。" >&2
    exit 1
  fi
  if ! git -C "$root" diff --quiet "$tag_commit" "$packaging_source_commit" -- apps crates plugins; then
    echo "当前打包提交中的 App 源码已与 v${version} 标签分叉，已停止。" >&2
    exit 1
  fi
  finder_layout_template="$unsigned_finder_layout_template"
else
  [[ -n "$signing_identity" && "$signing_identity" != "-" ]] || {
    echo "正式 DMG 必须提供非 ad-hoc 签名身份。" >&2
    exit 1
  }
  [[ -z "$app_source_commit" ]] || usage
  expected_filename="token-station_${version}_${architecture}.dmg"
  [[ "$(basename "$output_path")" == "$expected_filename" ]] || {
    echo "正式 DMG 文件名应为：$expected_filename" >&2
    exit 1
  }
fi

actual_bundle_id=$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$app_path/Contents/Info.plist" 2>/dev/null || true)
[[ "$actual_bundle_id" == "$expected_bundle_id" ]] || {
  echo "准备打包的 App 不是 Token Station，已停止。" >&2
  exit 1
}
codesign --verify --deep --strict "$app_path" || {
  echo "准备打包的 App 代码签名无效，已停止。" >&2
  exit 1
}
if [[ "$unsigned_test" == "true" ]]; then
  signature_details=$(codesign -d --verbose=4 "$app_path" 2>&1 || true)
  [[ "$signature_details" == *"Signature=adhoc"* ]] || {
    echo "测试 DMG 只允许包含 ad-hoc 签名 App，已停止。" >&2
    exit 1
  }
fi

output_parent=$(dirname "$output_path")
/bin/mkdir -p "$output_parent"
output_parent=$(cd "$output_parent" && pwd)
output_path="$output_parent/$(basename "$output_path")"
checksum_path="${output_path}.sha256"
if [[ "$unsigned_test" == "true" && -e "$checksum_path" ]]; then
  echo "校验文件已经存在，为避免覆盖已停止：$checksum_path" >&2
  exit 1
fi

stage=$(mktemp -d "${TMPDIR:-/tmp}/token-station-dmg.XXXXXX")
readonly stage
publish_stage=$(mktemp -d "$output_parent/.token-station-dmg.XXXXXX")
readonly publish_stage
temporary_dmg="$publish_stage/$(basename "$output_path")"
readonly temporary_dmg
temporary_checksum="$publish_stage/$(basename "$checksum_path")"
readonly temporary_checksum
writable_dmg="$publish_stage/token-station-layout-writable.dmg"
readonly writable_dmg
cleanup() {
  /bin/rm -rf -- "$stage"
  /bin/rm -rf -- "$publish_stage"
}
trap cleanup EXIT

/usr/bin/ditto "$app_path" "$stage/token-station.app"
ln -s /Applications "$stage/Applications"
/bin/cp "$readme" "$stage/installation-guide.md"
/bin/cp "$troubleshooting_guide" "$stage/macos-troubleshooting.md"
/bin/cp "$installer" "$stage/安装 Token Station.command"
/bin/chmod 755 "$stage/安装 Token Station.command"
if [[ "$unsigned_test" == "true" ]]; then
  /bin/cp "$unsigned_terminal_command" "$stage/终端启动命令.txt"
  printf '%s\n' \
    '警告：此 DMG 未签名、未经 Apple 公证，仅供测试。安装前请先阅读“installation-guide.md”并核对 SHA-256。' \
    >"$stage/未签名测试版.txt"
  printf 'App source tag: v%s\nApp source commit: %s\nPackaging source commit: %s\n' \
    "$version" "$app_source_commit" "$packaging_source_commit" \
    >"$stage/构建来源.txt"
fi

if ! /usr/bin/base64 -D -i "$finder_layout_template" -o "$stage/.DS_Store"; then
  echo "DMG 拖拽布局模板无法读取，请重新检出完整源码后重试。" >&2
  exit 1
fi
[[ -s "$stage/.DS_Store" ]] || {
  echo "DMG 拖拽布局模板是空文件，已停止生成发布文件。" >&2
  exit 1
}

hdiutil create \
  -volname "$volume_name" \
  -srcfolder "$stage" \
  -fs HFS+ \
  -format UDRW \
  "$writable_dmg"

hdiutil convert \
  "$writable_dmg" \
  -format UDZO \
  -imagekey zlib-level=9 \
  -o "$temporary_dmg" \
  >/dev/null
if [[ "$unsigned_test" == "true" ]]; then
  "$root/scripts/audit-macos-dmg.sh" \
    --unsigned-test \
    --dmg "$temporary_dmg" \
    --expected-version "$version" \
    --expected-arch "$architecture"
  digest=$(shasum -a 256 "$temporary_dmg" | awk '{print $1}')
  printf '%s  %s\n' "$digest" "$(basename "$output_path")" >"$temporary_checksum"
else
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
fi
/bin/mv -n "$temporary_dmg" "$output_path"
if [[ -e "$temporary_dmg" ]]; then
  echo "DMG 输出路径已被占用，没有覆盖现有文件：$output_path" >&2
  exit 1
fi
if [[ "$unsigned_test" == "true" ]]; then
  /bin/mv -n "$temporary_checksum" "$checksum_path"
  if [[ -e "$temporary_checksum" ]]; then
    echo "校验文件输出路径已被占用，没有覆盖：$checksum_path" >&2
    exit 1
  fi
  echo "未签名、未公证的测试 DMG 已创建并通过审计：$output_path"
  echo "SHA-256 校验文件：$checksum_path"
else
  shasum -a 256 "$output_path"
  echo "正式 macOS DMG 已创建并通过审计：$output_path"
fi

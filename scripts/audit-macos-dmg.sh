#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "用法：scripts/audit-macos-dmg.sh --dmg <path> --expected-version <x.y.z> --expected-arch <aarch64|x86_64>" >&2
  exit 2
}

dmg_path=""
expected_version=""
expected_arch=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dmg) dmg_path=${2:-}; shift 2 ;;
    --expected-version) expected_version=${2:-}; shift 2 ;;
    --expected-arch) expected_arch=${2:-}; shift 2 ;;
    *) usage ;;
  esac
done

[[ "$(uname -s)" == "Darwin" ]] || {
  echo "macOS DMG 挂载审计只能在 macOS 上运行。" >&2
  exit 1
}
[[ -n "$dmg_path" && -n "$expected_version" && -n "$expected_arch" ]] || usage
[[ "$expected_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || usage
[[ "$expected_arch" == "aarch64" || "$expected_arch" == "x86_64" ]] || usage
[[ -f "$dmg_path" ]] || {
  echo "没有找到待审计的 DMG：$dmg_path" >&2
  exit 1
}
expected_filename="token-station_${expected_version}_${expected_arch}.dmg"
[[ "$(basename "$dmg_path")" == "$expected_filename" ]] || {
  echo "DMG 文件名与版本或架构不一致，应为：$expected_filename" >&2
  exit 1
}

readonly expected_bundle_id="com.tokenstation.desktop"
mount_point=$(mktemp -d "${TMPDIR:-/tmp}/token-station-dmg-audit.XXXXXX")
readonly mount_point
mounted=false
cleanup() {
  if [[ "$mounted" == "true" ]]; then
    hdiutil detach "$mount_point" >/dev/null || true
  fi
  /bin/rmdir "$mount_point" 2>/dev/null || true
}
trap cleanup EXIT

hdiutil attach "$dmg_path" -readonly -nobrowse -mountpoint "$mount_point" >/dev/null
mounted=true

readonly mounted_app="$mount_point/token-station.app"
readonly mounted_applications="$mount_point/Applications"
readonly mounted_readme="$mount_point/安装前必读.md"
readonly mounted_installer="$mount_point/安装 Token Station.command"
readonly mounted_agent_rules="$mount_point/AGENTS.md"

for required_path in "$mounted_app" "$mounted_applications" "$mounted_readme" "$mounted_installer" "$mounted_agent_rules"; do
  [[ -e "$required_path" || -L "$required_path" ]] || {
    echo "DMG 缺少根目录入口：$(basename "$required_path")" >&2
    exit 1
  }
done

[[ -L "$mounted_applications" && "$(readlink "$mounted_applications")" == "/Applications" ]] || {
  echo "DMG 中的 Applications 不是指向 /Applications 的快捷方式。" >&2
  exit 1
}
[[ -x "$mounted_installer" ]] || {
  echo "DMG 中的安装 Token Station.command 不能双击执行。" >&2
  exit 1
}

actual_bundle_id=$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$mounted_app/Contents/Info.plist" 2>/dev/null || true)
[[ "$actual_bundle_id" == "$expected_bundle_id" ]] || {
  echo "DMG 中的 App bundle id 不正确。" >&2
  exit 1
}
actual_version=$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$mounted_app/Contents/Info.plist" 2>/dev/null || true)
[[ "$actual_version" == "$expected_version" ]] || {
  echo "DMG 中的 App 版本是 ${actual_version:-未知}，应为 $expected_version。" >&2
  exit 1
}
app_executable=$(/usr/libexec/PlistBuddy -c "Print :CFBundleExecutable" "$mounted_app/Contents/Info.plist" 2>/dev/null || true)
[[ -n "$app_executable" && -x "$mounted_app/Contents/MacOS/$app_executable" ]] || {
  echo "DMG 中没有找到可执行的 Token Station 主程序。" >&2
  exit 1
}
native_arch=$([[ "$expected_arch" == "aarch64" ]] && echo "arm64" || echo "x86_64")
actual_archs=$(lipo -archs "$mounted_app/Contents/MacOS/$app_executable")
[[ " $actual_archs " == *" $native_arch "* ]] || {
  echo "DMG 中的 App 架构是 $actual_archs，应包含 $native_arch。" >&2
  exit 1
}
codesign --verify --deep --strict "$mounted_app" || {
  echo "DMG 中的 App 代码签名验证失败。" >&2
  exit 1
}
codesign --verify --strict "$dmg_path" || {
  echo "DMG 自身的代码签名验证失败。" >&2
  exit 1
}
spctl --assess --type execute --verbose=2 "$mounted_app" || {
  echo "DMG 中的 App 没有通过 Gatekeeper。" >&2
  exit 1
}
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg_path" || {
  echo "DMG 没有通过 Gatekeeper。" >&2
  exit 1
}
xcrun stapler validate "$mounted_app" || {
  echo "DMG 中的 App 没有有效的 Apple 公证票据。" >&2
  exit 1
}
xcrun stapler validate "$dmg_path" || {
  echo "DMG 没有有效的 Apple 公证票据。" >&2
  exit 1
}

if /usr/bin/grep -Eq 'spctl[[:space:]]+--master-disable' "$mounted_readme" "$mounted_installer"; then
  echo "安装说明或脚本包含关闭 Gatekeeper 的命令。" >&2
  exit 1
fi
/usr/bin/grep -Fq 'xattr -dr com.apple.quarantine "$DEST_APP"' "$mounted_installer" || {
  echo "安装脚本没有把 quarantine 清理限制在 Token Station。" >&2
  exit 1
}

echo "macOS DMG mounted artifact: PASS ($(basename "$dmg_path"))"

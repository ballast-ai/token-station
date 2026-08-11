#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "用法：scripts/audit-macos-dmg.sh [--unsigned-test] --dmg <path> --expected-version <x.y.z> --expected-arch <aarch64|x86_64>" >&2
  exit 2
}

dmg_path=""
expected_version=""
expected_arch=""
unsigned_test=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --unsigned-test) unsigned_test=true; shift ;;
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
if [[ "$unsigned_test" == "true" ]]; then
  expected_filename="token-station_${expected_version}_${expected_arch}_UNSIGNED-UNNOTARIZED.dmg"
else
  expected_filename="token-station_${expected_version}_${expected_arch}.dmg"
fi
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
readonly mounted_unsigned_test_marker="$mount_point/未签名测试版.txt"
readonly mounted_provenance="$mount_point/构建来源.txt"
readonly mounted_ds_store="$mount_point/.DS_Store"
readonly finder_layout_script="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/packaging/macos/configure-dmg-layout.applescript"

for required_path in "$mounted_app" "$mounted_applications" "$mounted_readme" "$mounted_installer" "$mounted_agent_rules"; do
  [[ -e "$required_path" || -L "$required_path" ]] || {
    echo "DMG 缺少根目录入口：$(basename "$required_path")" >&2
    exit 1
  }
done
if [[ "$unsigned_test" == "true" ]]; then
  for required_path in "$mounted_unsigned_test_marker" "$mounted_provenance"; do
    [[ -f "$required_path" ]] || {
      echo "测试 DMG 缺少根目录警告或构建来源：$(basename "$required_path")" >&2
      exit 1
    }
  done
  actual_entries=$(/usr/bin/find "$mount_point" -mindepth 1 -maxdepth 1 -exec basename {} \; | LC_ALL=C sort)
  expected_entries=$(printf '%s\n' \
    token-station.app \
    Applications \
    '安装前必读.md' \
    '安装 Token Station.command' \
    AGENTS.md \
    '未签名测试版.txt' \
    '构建来源.txt' \
    .DS_Store | LC_ALL=C sort)
  [[ "$actual_entries" == "$expected_entries" ]] || {
    echo "测试 DMG 根目录包含多余或缺失的文件。" >&2
    exit 1
  }
fi

[[ -s "$mounted_ds_store" ]] || {
  echo "DMG 没有保存 Finder 拖拽布局，打开后会变成散乱的自动排列。" >&2
  exit 1
}
finder_layout_lines=( \
  'window=920x600' \
  'view=icon view' \
  'icon_size=128' \
  'arrangement=not arranged' \
  'toolbar=false' \
  'statusbar=false' \
  'pathbar=false' \
  'sidebar_width=0' \
  'app=310,170' \
  'applications=610,170' \
  'installer=100,440' \
  'readme=280,440')
if [[ "$unsigned_test" == "true" ]]; then
  finder_layout_lines+=( \
    'provenance=460,440' \
    'warning=640,440')
fi
finder_layout_lines+=('agent_rules=820,440')
expected_finder_layout=$(printf '%s\n' "${finder_layout_lines[@]}")
if ! actual_finder_layout=$(osascript "$finder_layout_script" inspect "$mount_point"); then
  echo "Finder 无法读取 DMG 的拖拽布局，请确认 Finder 可以正常启动后重试。" >&2
  exit 1
fi
[[ "$actual_finder_layout" == "$expected_finder_layout" ]] || {
  echo "DMG 的 Finder 布局与发布模板不一致。" >&2
  echo "期望布局：" >&2
  printf '%s\n' "$expected_finder_layout" >&2
  echo "实际布局：" >&2
  printf '%s\n' "$actual_finder_layout" >&2
  exit 1
}

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
if [[ "$unsigned_test" == "true" ]]; then
  signature_details=$(codesign -d --verbose=4 "$mounted_app" 2>&1 || true)
  [[ "$signature_details" == *"Signature=adhoc"* ]] || {
    echo "测试 DMG 中的 App 不是预期的 ad-hoc 签名。" >&2
    exit 1
  }
  if codesign --verify --strict "$dmg_path" >/dev/null 2>&1; then
    echo "测试 DMG 意外带有代码签名，不能按未签名资产发布。" >&2
    exit 1
  fi
  if xcrun stapler validate "$mounted_app" >/dev/null 2>&1 || xcrun stapler validate "$dmg_path" >/dev/null 2>&1; then
    echo "测试 DMG 或 App 意外带有 Apple 公证票据，发布状态不明确。" >&2
    exit 1
  fi
  if spctl --assess --type execute --verbose=2 "$mounted_app" >/dev/null 2>&1; then
    echo "测试 App 意外通过 Gatekeeper，不能声称它是已知的未公证测试包。" >&2
    exit 1
  fi
  for phrase in '未签名' '未经 Apple 公证' '仅供测试' 'SHA-256'; do
    /usr/bin/grep -Fq "$phrase" "$mounted_readme" || {
      echo "测试 DMG 安装说明缺少风险提示：$phrase" >&2
      exit 1
    }
  done
  /usr/bin/grep -Fq '未签名、未经 Apple 公证' "$mounted_installer" || {
    echo "测试 DMG 安装脚本没有显示未签名、未公证提示。" >&2
    exit 1
  }
  /usr/bin/grep -Eq '^App source tag: v[0-9]+\.[0-9]+\.[0-9]+$' "$mounted_provenance" || {
    echo "构建来源缺少 App 标签。" >&2
    exit 1
  }
  [[ "$(/usr/bin/grep -Ec '^(App source commit|Packaging source commit): [0-9a-f]{40}$' "$mounted_provenance")" == "2" ]] || {
    echo "构建来源没有记录完整提交。" >&2
    exit 1
  }
else
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
fi

if /usr/bin/grep -Eq 'spctl[[:space:]]+--master-disable' "$mounted_readme" "$mounted_installer"; then
  echo "安装说明或脚本包含关闭 Gatekeeper 的命令。" >&2
  exit 1
fi
/usr/bin/grep -Fq 'xattr -dr com.apple.quarantine "$DEST_APP"' "$mounted_installer" || {
  echo "安装脚本没有把 quarantine 清理限制在 Token Station。" >&2
  exit 1
}

if [[ "$unsigned_test" == "true" ]]; then
  echo "macOS unsigned test DMG mounted artifact: PASS ($(basename "$dmg_path"))"
else
  echo "macOS DMG mounted artifact: PASS ($(basename "$dmg_path"))"
fi

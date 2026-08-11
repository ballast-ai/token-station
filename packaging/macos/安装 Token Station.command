#!/usr/bin/env bash
set -euo pipefail

readonly EXPECTED_BUNDLE_ID="com.tokenstation.desktop"
readonly DEST_APP="/Applications/token-station.app"
readonly LOCK_DIR="/Applications/.token-station-install.lock"
readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly SOURCE_APP="$SCRIPT_DIR/token-station.app"
readonly UNSIGNED_TEST_MARKER="$SCRIPT_DIR/未签名测试版.txt"

TEMP_ROOT=""
BACKUP_ROOT=""
TEMP_APP=""
BACKUP_APP=""

had_previous=false
installed_new=false
completed=false
lock_acquired=false
unsigned_test=false

if [[ -f "$UNSIGNED_TEST_MARKER" ]]; then
  unsigned_test=true
fi

bundle_id() {
  /usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$1/Contents/Info.plist" 2>/dev/null
}

verify_app() {
  local app_path=$1
  local label=$2
  local signature_details

  if [[ ! -d "$app_path" ]]; then
    echo "错误：没有找到${label}：$app_path" >&2
    return 1
  fi
  if [[ "$(bundle_id "$app_path" || true)" != "$EXPECTED_BUNDLE_ID" ]]; then
    echo "错误：${label}不是官方 Token Station，已停止操作。" >&2
    return 1
  fi
  if ! codesign --verify --deep --strict "$app_path" >/dev/null 2>&1; then
    echo "错误：${label}的代码签名没有通过验证，请重新从官方发布页面下载。" >&2
    return 1
  fi
  if [[ "$unsigned_test" == "true" ]]; then
    signature_details=$(codesign -d --verbose=4 "$app_path" 2>&1 || true)
    if [[ "$signature_details" != *"Signature=adhoc"* ]]; then
      echo "错误：${label}不是预期的 ad-hoc 签名测试 App，已停止操作。" >&2
      return 1
    fi
  fi
}

verify_installed_app() {
  verify_app "$DEST_APP" "安装后的 App"
}

remove_managed_directory() {
  local managed_path=${1:-}
  [[ -n "$managed_path" ]] || return 0
  if [[ ! "$managed_path" =~ ^/Applications/\.token-station-(install|backup)\.[[:alnum:]]+$ ]]; then
    echo "错误：拒绝清理不属于本次安装的目录：$managed_path" >&2
    return 1
  fi
  if [[ -d "$managed_path" ]]; then
    sudo /bin/rm -rf -- "$managed_path"
  fi
}

release_lock() {
  if [[ "$lock_acquired" == "true" ]]; then
    sudo /bin/rmdir "$LOCK_DIR"
    lock_acquired=false
  fi
}

restore_previous_app() {
  local restore_failed=false

  if [[ "$installed_new" == "true" && -e "$DEST_APP" ]]; then
    if [[ "$(bundle_id "$DEST_APP" || true)" == "$EXPECTED_BUNDLE_ID" ]]; then
      sudo /bin/rm -rf -- "$DEST_APP" || restore_failed=true
    else
      echo "错误：安装目标出现了非 Token Station 应用，不能自动删除。" >&2
      restore_failed=true
    fi
  fi

  if [[ "$had_previous" == "true" && -e "$BACKUP_APP" ]]; then
    if [[ ! -e "$DEST_APP" ]]; then
      sudo /bin/mv "$BACKUP_APP" "$DEST_APP" || restore_failed=true
      if [[ -e "$DEST_APP" ]]; then
        verify_installed_app || restore_failed=true
      fi
    else
      echo "错误：安装目标仍被占用，旧版本保留在 $BACKUP_APP。" >&2
      restore_failed=true
    fi
  fi

  remove_managed_directory "$TEMP_ROOT" || restore_failed=true
  remove_managed_directory "$BACKUP_ROOT" || restore_failed=true
  if [[ "$restore_failed" == "true" ]]; then
    echo "旧版本未能自动恢复，请保留此窗口并联系官方支持。" >&2
    return 1
  fi
  if [[ "$had_previous" == "true" ]]; then
    echo "已恢复安装前的 Token Station。" >&2
  fi
}

handle_exit() {
  local status=$?
  set +e
  if [[ "$completed" != "true" ]]; then
    echo
    echo "安装没有完成，正在恢复安装前状态……" >&2
    restore_previous_app
  fi
  release_lock
  if [[ $status -ne 0 ]]; then
    echo
    read -r -p "请查看上面的错误说明，按回车键关闭窗口。" _ </dev/tty
  fi
  exit "$status"
}
trap handle_exit EXIT

echo "Token Station 安装程序"
echo "------------------------"
echo "将检查同一 DMG 中的 token-station.app，并安装到："
echo "  $DEST_APP"
echo "脚本只会处理这个 App，不会关闭 Gatekeeper，也不会修改 SIP。"
echo

if [[ "$unsigned_test" == "true" ]]; then
  echo "重要提醒：这是未签名、未经 Apple 公证的测试 App。"
  echo "Apple 没有验证开发者身份，请只在已核对 GitHub Release 来源和 SHA-256 时继续。"
  echo
fi

verify_app "$SOURCE_APP" "DMG 中的 App"
if [[ "$unsigned_test" == "true" ]]; then
  if spctl --assess --type execute --verbose=2 "$SOURCE_APP" >/dev/null 2>&1; then
    echo "提醒：该测试 App 意外通过了本机 Gatekeeper，但它仍未完成 Apple 公证。" >&2
  else
    echo "已确认 Gatekeeper 会拒绝该未公证测试 App，安装脚本将只对 Token Station 进行放行。"
  fi
else
  if ! spctl --assess --type execute --verbose=2 "$SOURCE_APP" >/dev/null 2>&1; then
    echo "错误：DMG 中的 App 没有通过 macOS 安全检查，请重新从官方发布页面下载。" >&2
    exit 1
  fi
fi

read -r -p "确认安装 Token Station 吗？[yY] " answer
if [[ ! "$answer" =~ ^[yY]$ ]]; then
  echo "已取消，没有修改系统。"
  completed=true
  exit 0
fi

echo "接下来 macOS 会请求管理员密码。输入密码时不会显示字符，这是正常现象。"
sudo -v

if ! sudo /bin/mkdir "$LOCK_DIR"; then
  echo "错误：另一个 Token Station 安装程序正在运行，请关闭其他安装窗口后重试。" >&2
  exit 1
fi
lock_acquired=true

TEMP_ROOT=$(sudo /usr/bin/mktemp -d "/Applications/.token-station-install.XXXXXX")
BACKUP_ROOT=$(sudo /usr/bin/mktemp -d "/Applications/.token-station-backup.XXXXXX")
sudo /bin/chmod 755 "$TEMP_ROOT" "$BACKUP_ROOT"
TEMP_APP="$TEMP_ROOT/token-station.app"
BACKUP_APP="$BACKUP_ROOT/token-station.app"

if [[ -e "$DEST_APP" ]]; then
  verify_installed_app
fi

echo "正在复制并验证新版本……"
sudo /usr/bin/ditto "$SOURCE_APP" "$TEMP_APP"
verify_app "$TEMP_APP" "复制后的新 App"

if [[ -e "$DEST_APP" ]]; then
  sudo /bin/mv "$DEST_APP" "$BACKUP_APP"
  had_previous=true
fi

sudo /bin/mv "$TEMP_APP" "$DEST_APP"
installed_new=true
verify_installed_app

echo "正在只针对 Token Station 完成首次启动放行……"
sudo xattr -dr com.apple.quarantine "$DEST_APP"
verify_installed_app

if ! open "$DEST_APP"; then
  echo "错误：App 已安装，但启动失败。" >&2
  exit 1
fi

remove_managed_directory "$BACKUP_ROOT"
remove_managed_directory "$TEMP_ROOT"
completed=true
release_lock || echo "提醒：App 已安装，但安装锁未能清理；再次安装前请联系官方支持。" >&2
trap - EXIT
echo
echo "安装完成，Token Station 已启动。"

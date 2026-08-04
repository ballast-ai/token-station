#!/usr/bin/env bash
set -euo pipefail

# Cursor stores the OpenAI BYOK values in its globalStorage SQLite database.
# This helper follows the community-proven layout, but fails closed while
# Cursor is running and always writes a timestamped backup first.

DB_PATH="${CURSOR_USER_DB:-${HOME}/Library/Application Support/Cursor/User/globalStorage/state.vscdb}"
BACKUP_DIR="${TOKEN_STATION_CURSOR_BACKUP_DIR:-${HOME}/Library/Application Support/com.tokenstation.desktop/cursor-backups}"
APP_PROCESS_PATTERN="/Applications/Cursor.app/Contents/MacOS/Cursor"

die() { echo "cursor-provider: $*" >&2; exit 1; }

command -v sqlite3 >/dev/null 2>&1 || die "需要 sqlite3"
command -v python3 >/dev/null 2>&1 || die "需要 python3"

[[ -f "$DB_PATH" ]] || die "找不到 Cursor 数据库: $DB_PATH"
pgrep -f "$APP_PROCESS_PATTERN" >/dev/null 2>&1 && die "请先完全退出 Cursor"

usage() {
  echo "用法: $0 set <base-url> <api-key> | restore <backup-file>"
}

case "${1:-}" in
  set)
    [[ $# -eq 3 ]] || { usage; exit 2; }
    BASE_URL="${2%/}/v1"
    API_KEY="$3"
    mkdir -p "$BACKUP_DIR"
    BACKUP_PATH="$BACKUP_DIR/state.vscdb.$(date +%Y%m%d-%H%M%S).bak"
    cp -p "$DB_PATH" "$BACKUP_PATH"
    export DB_PATH BASE_URL API_KEY
    python3 - <<'PY'
import json, os, shutil, sqlite3, tempfile

db = os.environ["DB_PATH"]
base_url = os.environ["BASE_URL"]
api_key = os.environ["API_KEY"]
key_name = "src.vs.platform.reactivestorage.browser.reactiveStorageServiceImpl.persistentStorage.applicationUser"
secret_name = "secret://cursorAuth/openAIKey"

conn = sqlite3.connect(db, timeout=5.0)
try:
    conn.execute("BEGIN IMMEDIATE")
    row = conn.execute("SELECT value FROM ItemTable WHERE key = ?", (key_name,)).fetchone()
    if not row:
        raise RuntimeError("Cursor applicationUser 记录不存在")
    data = json.loads(row[0])
    data["openAIBaseUrl"] = base_url
    encoded = json.dumps(data, ensure_ascii=False, separators=(",", ":"))
    conn.execute("UPDATE ItemTable SET value = ? WHERE key = ?", (encoded, key_name))
    if conn.execute("SELECT changes()").fetchone()[0] != 1:
        raise RuntimeError("Base URL 写入失败")
    conn.execute("INSERT INTO ItemTable(key, value) VALUES(?, ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", (secret_name, api_key))
    conn.commit()
finally:
    conn.close()

# Verify without printing either secret.
check = sqlite3.connect(db)
try:
    row = check.execute("SELECT value FROM ItemTable WHERE key = ?", (key_name,)).fetchone()
    verified = bool(row) and json.loads(row[0]).get("openAIBaseUrl") == base_url
    secret = check.execute("SELECT value FROM ItemTable WHERE key = ?", (secret_name,)).fetchone()
    verified = verified and bool(secret) and secret[0] == api_key
finally:
    check.close()
if not verified:
    raise SystemExit("回读验证失败")
PY
    echo "已写入 Cursor 配置，备份: $BACKUP_PATH"
    ;;
  restore)
    [[ $# -eq 2 ]] || { usage; exit 2; }
    [[ -f "$2" ]] || die "找不到备份文件: $2"
    cp -p "$2" "$DB_PATH"
    echo "已恢复 Cursor 配置: $2"
    ;;
  *) usage; exit 2 ;;
esac

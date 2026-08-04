#!/usr/bin/env bash

set -u

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
desktop_dir="$repo_root/apps/desktop"
pid_file="$repo_root/.git/token-station-develop-watch.pid"
log_file="$repo_root/.git/token-station-develop-watch.log"
remote="${TOKEN_STATION_REMOTE:-origin}"
develop_branch="${TOKEN_STATION_DEVELOP_BRANCH:-develop}"
poll_seconds="${TOKEN_STATION_POLL_SECONDS:-60}"
app_pid=""

watcher_pid() {
  [[ -f "$pid_file" ]] || return 1
  local pid
  pid="$(<"$pid_file")"
  [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 1
  kill -0 "$pid" 2>/dev/null || return 1
  printf '%s' "$pid"
}

case "${1:---daemon}" in
  --daemon|--start)
    if pid="$(watcher_pid)"; then
      printf 'Watcher is already running (PID %s)\n' "$pid"
      exit 0
    fi
    rm -f "$pid_file"
    nohup "$0" --foreground >>"$log_file" 2>&1 </dev/null &
    pid=$!
    printf '%s\n' "$pid" >"$pid_file"
    printf 'Watcher started in background (PID %s)\nLog: %s\n' "$pid" "$log_file"
    exit 0
    ;;
  --stop)
    if pid="$(watcher_pid)"; then
      kill "$pid"
      printf 'Watcher stop requested (PID %s)\n' "$pid"
    else
      printf 'Watcher is not running\n'
      rm -f "$pid_file"
    fi
    exit 0
    ;;
  --status)
    if pid="$(watcher_pid)"; then
      printf 'Watcher is running (PID %s)\nLog: %s\n' "$pid" "$log_file"
      exit 0
    fi
    printf 'Watcher is not running\n'
    exit 1
    ;;
  --foreground)
    printf '%s\n' "$$" >"$pid_file"
    ;;
  *)
    printf 'Usage: %s [--start|--stop|--status|--foreground]\n' "$0" >&2
    exit 2
    ;;
esac

log() {
  printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*"
}

stop_app() {
  if [[ -n "$app_pid" ]] && kill -0 "$app_pid" 2>/dev/null; then
    log "Stopping desktop development process (PID $app_pid)"
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
  fi
  app_pid=""
}

start_app() {
  stop_app
  log "Starting Tauri desktop development version"
  (
    cd "$desktop_dir"
    exec npm run tauri dev
  ) &
  app_pid=$!
  log "Desktop development process started (PID $app_pid)"
}

cleanup() {
  stop_app
  if [[ -f "$pid_file" ]] && [[ "$(<"$pid_file")" == "$$" ]]; then
    rm -f "$pid_file"
  fi
  log "Watcher stopped"
}

trap cleanup EXIT INT TERM

if ! [[ "$poll_seconds" =~ ^[1-9][0-9]*$ ]]; then
  log "TOKEN_STATION_POLL_SECONDS must be a positive integer"
  exit 2
fi

cd "$repo_root"
log "Watching $remote/$develop_branch every ${poll_seconds}s"

if ! git fetch "$remote" "$develop_branch"; then
  log "Initial fetch failed; will retry during the next poll"
fi

last_remote="$(git rev-parse --verify "$remote/$develop_branch" 2>/dev/null || true)"
start_app

while true; do
  sleep "$poll_seconds"

  if [[ -z "$app_pid" ]] || ! kill -0 "$app_pid" 2>/dev/null; then
    log "Desktop development process is not running; restarting it"
    start_app
  fi

  if ! git fetch "$remote" "$develop_branch"; then
    log "Fetch failed; keeping the current version running"
    continue
  fi

  current_remote="$(git rev-parse --verify "$remote/$develop_branch" 2>/dev/null || true)"
  if [[ -z "$current_remote" || "$current_remote" == "$last_remote" ]]; then
    continue
  fi

  log "New develop revision detected: ${last_remote:0:7} -> ${current_remote:0:7}"

  if ! git diff --quiet || ! git diff --cached --quiet; then
    log "Tracked files have uncommitted changes; skipping automatic merge"
    continue
  fi

  if git merge-base --is-ancestor "$remote/$develop_branch" HEAD; then
    log "Current branch already contains the new develop revision"
    last_remote="$current_remote"
    start_app
    continue
  fi

  if git merge --no-edit "$remote/$develop_branch"; then
    last_remote="$current_remote"
    log "Merged $remote/$develop_branch successfully"
    start_app
  else
    log "Merge failed; aborting it and keeping the current version running"
    git merge --abort 2>/dev/null || true
  fi
done

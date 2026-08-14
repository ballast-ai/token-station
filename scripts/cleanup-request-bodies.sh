#!/usr/bin/env bash
set -euo pipefail

retention_days=7
request_data_dir="${HOME:?}/Library/Application Support/com.tokenstation.desktop/token-station-data"

usage() {
  echo "Usage: $0 [--data-dir PATH] [--days POSITIVE_INTEGER]"
}

while (($# > 0)); do
  case "$1" in
    --data-dir)
      (($# >= 2)) || { usage >&2; exit 2; }
      request_data_dir=$2
      shift 2
      ;;
    --days)
      (($# >= 2)) || { usage >&2; exit 2; }
      retention_days=$2
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
done

[[ "$retention_days" =~ ^[1-9][0-9]*$ ]] || {
  echo "--days must be a positive integer" >&2
  exit 2
}

request_body_dir="${request_data_dir%/}/request-bodies"
[[ "$request_body_dir" == */request-bodies ]] || {
  echo "refusing unsafe request body directory" >&2
  exit 2
}
if [[ ! -e "$request_body_dir" ]]; then
  echo "request body cleanup: directory does not exist: $request_body_dir"
  exit 0
fi
[[ -d "$request_body_dir" && ! -L "$request_body_dir" ]] || {
  echo "refusing request body path that is not a real directory: $request_body_dir" >&2
  exit 2
}

now_seconds=$(date +%s)
cutoff_seconds=$((now_seconds - retention_days * 24 * 60 * 60))
scanned=0
deleted=0

for request_body_file in "$request_body_dir"/req_*.json; do
  [[ -e "$request_body_file" ]] || continue
  [[ -f "$request_body_file" && ! -L "$request_body_file" ]] || continue
  request_body_name=${request_body_file##*/}
  [[ "$request_body_name" =~ ^req_[0-9a-f]{32}\.json$ ]] || continue
  scanned=$((scanned + 1))
  if modified_seconds=$(stat -f %m "$request_body_file" 2>/dev/null); then
    :
  else
    modified_seconds=$(stat -c %Y "$request_body_file")
  fi
  if ((modified_seconds < cutoff_seconds)); then
    rm -f -- "$request_body_file"
    deleted=$((deleted + 1))
  fi
done

echo "request body cleanup: directory=$request_body_dir scanned=$scanned deleted=$deleted retention_days=$retention_days"

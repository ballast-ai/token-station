#!/usr/bin/env bash
set -euo pipefail

readonly FROZEN_BASE='69eb9f65571147ff878d0c97b9aeb52d30d7ab32'
readonly PROTECTED_PATH='crates/router-core/'
readonly ZERO_SHA='0000000000000000000000000000000000000000'

usage() {
  echo "usage: $0 <range-base-commit> <head-commit>" >&2
}

if [[ $# -ne 2 ]]; then
  usage
  exit 2
fi

range_base_input="$1"
readonly head_input="$2"

if [[ "$head_input" == "$ZERO_SHA" ]]; then
  echo 'router-core redline: head commit must not be the all-zero SHA' >&2
  exit 2
fi
if [[ "$range_base_input" == "$ZERO_SHA" ]]; then
  range_base_input="$FROZEN_BASE"
fi

resolve_commit() {
  local label="$1"
  local revision="$2"
  local resolved

  if [[ -z "$revision" || "$revision" == -* ]]; then
    echo "router-core redline: invalid $label revision '$revision'" >&2
    return 2
  fi
  if ! resolved="$(git rev-parse --verify --end-of-options "${revision}^{commit}" 2>/dev/null)"; then
    echo "router-core redline: $label commit '$revision' is unavailable" >&2
    return 2
  fi
  printf '%s\n' "$resolved"
}

if ! range_base="$(resolve_commit 'range base' "$range_base_input")"; then
  exit 2
fi
if ! head_sha="$(resolve_commit 'head' "$head_input")"; then
  exit 2
fi
if ! frozen_sha="$(resolve_commit 'frozen baseline' "$FROZEN_BASE")"; then
  exit 2
fi
readonly range_base head_sha frozen_sha

set +e
git merge-base --is-ancestor "$frozen_sha" "$head_sha"
readonly ancestry_status=$?
set -e
case "$ancestry_status" in
  0) ;;
  1)
    echo 'router-core redline: frozen baseline is not an ancestor of head' >&2
    exit 2
    ;;
  *)
    echo "router-core redline: ancestry check failed with status $ancestry_status" >&2
    exit 2
    ;;
esac

check_gate() {
  local label="$1"
  local base="$2"
  local head="$3"
  local status

  set +e
  git diff --no-ext-diff --no-renames --quiet \
    "$base" "$head" -- "$PROTECTED_PATH"
  status=$?
  set -e

  case "$status" in
    0)
      echo "router-core redline: PASS $label ($base..$head)"
      return 0
      ;;
    1)
      echo "router-core redline: BLOCKED $label — '$PROTECTED_PATH' changed" >&2
      git diff --no-ext-diff --no-renames --name-status \
        "$base" "$head" -- "$PROTECTED_PATH" >&2
      return 1
      ;;
    *)
      echo "router-core redline: $label failed with git diff status $status" >&2
      return 2
      ;;
  esac
}

violation=0
set +e
check_gate 'event range' "$range_base" "$head_sha"
status=$?
set -e
case "$status" in
  0) ;;
  1) violation=1 ;;
  *) exit 2 ;;
esac

set +e
check_gate 'frozen baseline' "$frozen_sha" "$head_sha"
status=$?
set -e
case "$status" in
  0) ;;
  1) violation=1 ;;
  *) exit 2 ;;
esac

exit "$violation"

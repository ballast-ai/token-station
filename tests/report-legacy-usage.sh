#!/usr/bin/env bash
set -euo pipefail

readonly project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly test_root="$(mktemp -d "${TMPDIR:-/tmp}/token-station-legacy-usage.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT

readonly report_script="$project_root/scripts/report-legacy-usage.py"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

# Builds a metrics database with the real requests/attempts DDL (copied from
# apps/cli/src/store.rs, CHECK constraints included, so an enum typo in the
# fixture fails here instead of silently passing) plus the rows named on the
# command line: engine|reason|latency|status|error_code|age_days per attempt,
# one owning request per attempt.
make_db() {
  local db="$1"
  shift
  python3 - "$db" "$@" <<'PY'
import sqlite3
import sys
import time

SCHEMA = """
CREATE TABLE requests (
    id                  INTEGER PRIMARY KEY,
    request_id          TEXT    NOT NULL DEFAULT '',
    agent_id            TEXT,
    running_revision    INTEGER,
    started_at_ms       INTEGER NOT NULL,
    latency_ms          INTEGER NOT NULL,
    protocol            TEXT    NOT NULL,
    requested_model     TEXT    NOT NULL,
    stream              INTEGER NOT NULL,
    status              INTEGER NOT NULL,
    error_code          TEXT,
    attempts            INTEGER NOT NULL,
    upstream            TEXT,
    model               TEXT
);
CREATE UNIQUE INDEX requests_request_id
    ON requests (request_id) WHERE request_id <> '';

CREATE TABLE attempts (
    request_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    upstream TEXT NOT NULL,
    model TEXT NOT NULL,
    latency_ms INTEGER NOT NULL,
    http_status INTEGER,
    error_code TEXT,
    stream_outcome TEXT CHECK (
        stream_outcome IS NULL OR stream_outcome IN (
            'complete', 'failed_after_partial', 'failed_before_output', 'client_cancelled'
        )
    ),
    provider_call_engine TEXT NOT NULL DEFAULT 'unknown'
        CHECK (provider_call_engine IN (
            'legacy', 'south_v1_buffered', 'south_v1_streaming', 'native', 'unknown'
        )),
    south_fallback_reason TEXT
        CHECK (south_fallback_reason IS NULL OR south_fallback_reason IN (
            'configured_legacy', 'buffered_mode_cannot_stream', 'unauthenticated_upstream',
            'no_provider_runtime', 'credential_resolver', 'provider_dialect',
            'provider_package_unapproved', 'api_dialect', 'egress', 'streaming', 'method',
            'auth', 'body', 'secret_source', 'response_metadata', 'headers'
        )),
    fallback_allowed INTEGER NOT NULL,
    PRIMARY KEY (request_id, ordinal)
);
"""

db_path = sys.argv[1]
connection = sqlite3.connect(db_path)
connection.executescript(SCHEMA)
now_ms = int(time.time() * 1000)

for index, spec in enumerate(sys.argv[2:]):
    engine, reason, latency, status, error_code, age_days = spec.split("|")
    request_id = f"req-{index:05}"
    started_at = now_ms - int(age_days) * 86_400_000
    connection.execute(
        "INSERT INTO requests (request_id, started_at_ms, latency_ms, protocol,"
        " requested_model, stream, status, attempts)"
        " VALUES (?, ?, ?, 'openai-chat', 'test-model', 0, 200, 1)",
        (request_id, started_at, int(latency)),
    )
    connection.execute(
        "INSERT INTO attempts (request_id, ordinal, upstream, model, latency_ms,"
        " http_status, error_code, provider_call_engine, south_fallback_reason,"
        " fallback_allowed) VALUES (?, 0, 'up', 'test-model', ?, ?, ?, ?, ?, 1)",
        (
            request_id,
            int(latency),
            None if status == "null" else int(status),
            None if error_code == "null" else error_code,
            engine,
            None if reason == "null" else reason,
        ),
    )

connection.commit()
connection.close()
PY
}

test_mixed_window_is_counted_per_attempt_and_fails_the_gate() {
  local db="$test_root/mixed.sqlite"
  # In window: 3 legacy (one configured_legacy, one 500), 2 south, 1 native,
  # 1 unknown. Out of window (40 days): 1 legacy that must not be counted.
  make_db "$db" \
    'legacy|configured_legacy|100|200|null|0' \
    'legacy|egress|150|500|upstream_5xx|1' \
    'legacy|streaming|120|200|null|2' \
    'south_v1_streaming|null|80|200|null|3' \
    'south_v1_buffered|null|90|200|null|4' \
    'native|null|70|200|null|5' \
    'unknown|null|60|200|null|6' \
    'legacy|configured_legacy|100|200|null|40'

  cat >"$test_root/mixed-config.json" <<'JSON'
{
  "upstreams": {
    "a": { "provider": "openai-compatible", "provider_call": "legacy" },
    "b": { "provider": "openai-compatible", "provider_call": "south_v1_buffered" },
    "c": { "provider": "openai-compatible" },
    "d": {
      "provider": "openai-compatible",
      "provider_call": "south_v1_buffered_streaming_header_auth"
    }
  }
}
JSON

  python3 "$report_script" --db "$db" --config "$test_root/mixed-config.json" \
    --json >"$test_root/mixed-report.json" \
    || fail "report script failed on the mixed fixture"

  python3 - "$test_root/mixed-report.json" <<'PY'
import json
import sys

report = json.load(open(sys.argv[1]))
check = lambda ok, what: ok or sys.exit(f"FAIL: mixed fixture: {what}: {report}")

# The spelled-out default on "d" is still the default engine.
check(report["config"]["explicit_legacy_upstreams"] == ["a"], "explicit legacy list")
check(
    report["config"]["explicit_non_default_provider_call"]
    == {"a": "legacy", "b": "south_v1_buffered"},
    "explicit non-default map",
)

attempts = report["attempts"]
check(attempts["total"] == 7, "window total must exclude the 40-day-old attempt")
check(attempts["by_engine"]["legacy"]["count"] == 3, "legacy attempt count")
check(attempts["by_engine"]["legacy"]["error_count"] == 1, "legacy error count")
check(attempts["by_engine"]["unknown"]["count"] == 1, "unknown attempt count")
# 3 legacy / (7 - 1 unknown); native stays in the denominator.
check(attempts["legacy_attempt_ratio"] == 0.5, "legacy ratio")
check(
    attempts["by_south_fallback_reason"]["configured_legacy"] == 1,
    "configured_legacy reason count",
)

gate = report["gate"]
check(gate["zero_explicit_use"]["status"] == "fail", "zero_explicit_use verdict")
check(
    gate["fallback_below_threshold"]["status"] == "fail",
    "fallback_below_threshold verdict",
)
check(
    gate["south_not_degraded"]["status"] == "insufficient_data",
    "south_not_degraded needs 100 legacy attempts",
)
check(report["verdict"] == "not_ready", "overall verdict")
PY
}

test_clean_config_and_small_fallback_share_is_ready() {
  local db="$test_root/ready.sqlite"
  local rows=()
  # 200 legacy fallback attempts with latencies 1..200 ms (p50 100, p95 190)
  # and 800 south attempts at a flat 150 ms, all successful.
  local i
  for ((i = 1; i <= 200; i++)); do
    rows+=("legacy|streaming|$i|200|null|0")
  done
  for ((i = 1; i <= 800; i++)); do
    rows+=("south_v1_streaming|null|150|200|null|0")
  done
  make_db "$db" "${rows[@]}"

  printf '{ "upstreams": { "c": { "provider": "openai-compatible" } } }\n' \
    >"$test_root/ready-config.json"

  python3 "$report_script" --db "$db" --config "$test_root/ready-config.json" \
    --fallback-threshold 0.5 >"$test_root/ready-report.json" \
    || fail "report script failed on the ready fixture"

  python3 - "$test_root/ready-report.json" <<'PY'
import json
import sys

report = json.load(open(sys.argv[1]))
check = lambda ok, what: ok or sys.exit(f"FAIL: ready fixture: {what}: {report}")

legacy = report["attempts"]["by_engine"]["legacy"]
check(legacy["count"] == 200, "legacy attempt count")
check(legacy["p50_latency_ms"] == 100, "legacy p50")
check(legacy["p95_latency_ms"] == 190, "legacy p95")
check(report["attempts"]["legacy_attempt_ratio"] == 0.2, "legacy ratio")

gate = report["gate"]
check(gate["zero_explicit_use"]["status"] == "pass", "zero_explicit_use verdict")
check(
    gate["fallback_below_threshold"]["status"] == "pass",
    "fallback_below_threshold under the overridden threshold",
)
check(gate["south_not_degraded"]["status"] == "pass", "south_not_degraded verdict")
check(gate["south_not_degraded"]["south_p95_latency_ms"] == 150, "south p95")
check(report["verdict"] == "ready", "overall verdict")
PY
}

test_pre_south_database_gets_a_clear_error() {
  local db="$test_root/old.sqlite"
  python3 - "$db" <<'PY'
import sqlite3
import sys

connection = sqlite3.connect(sys.argv[1])
connection.execute("CREATE TABLE requests (request_id TEXT, started_at_ms INTEGER)")
connection.commit()
connection.close()
PY
  printf '{ "upstreams": {} }\n' >"$test_root/old-config.json"

  if python3 "$report_script" --db "$db" --config "$test_root/old-config.json" \
    >"$test_root/old-stdout" 2>"$test_root/old-stderr"; then
    fail "report script accepted a database without an attempts table"
  fi
  grep -Fq "no attempts table" "$test_root/old-stderr" \
    || fail "missing attempts table error was not explicit"
  if grep -Fq "Traceback" "$test_root/old-stderr"; then
    fail "missing attempts table produced a traceback instead of an error"
  fi
}

test_mixed_window_is_counted_per_attempt_and_fails_the_gate
test_clean_config_and_small_fallback_share_is_ready
test_pre_south_database_gets_a_clear_error

echo "report-legacy-usage tests: PASS"

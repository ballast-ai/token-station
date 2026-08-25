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
    parts = spec.split("|")
    engine, reason, latency, status, error_code, age_days = parts[:6]
    # An optional seventh field names the upstream, so a fixture can build more
    # than one cohort. Without it every row lands in the same one.
    upstream = parts[6] if len(parts) > 6 else "up"
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
        " fallback_allowed) VALUES (?, 0, ?, 'test-model', ?, ?, ?, ?, ?, 1)",
        (
            request_id,
            upstream,
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

usage = report["usage_gate"]
check(usage["zero_explicit_use"]["status"] == "fail", "zero_explicit_use verdict")
check(
    usage["fallback_below_threshold"]["status"] == "fail",
    "fallback_below_threshold verdict",
)
performance = report["performance_gate"]
check(
    performance["status"] == "insufficient_data",
    "performance needs a real sample on both sides",
)
check(performance["source"] == "live", "performance source")
check(report["thresholds_approved"] is False, "draft thresholds are marked unapproved")
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

usage = report["usage_gate"]
check(usage["zero_explicit_use"]["status"] == "pass", "zero_explicit_use verdict")
check(
    usage["fallback_below_threshold"]["status"] == "pass",
    "fallback_below_threshold under the overridden threshold",
)

performance = report["performance_gate"]
check(performance["status"] == "pass", "performance verdict")
check(performance["source"] == "live", "measured live, not carried from a baseline")
# Every attempt in this fixture is the same upstream, model and transport, so
# it forms one cohort with both sides well over the minimum. That is the point:
# the comparison is within a cohort, never across the whole window.
compared = performance["cohorts"]["compared"]
check(len(compared) == 1, f"one cohort compared, got {len(compared)}")
cohort = compared[0]
check(cohort["transport"] == "buffered", "transport shape")
check(cohort["south_count"] == 800 and cohort["legacy_count"] == 200, "cohort sides")
check(cohort["south_p95_latency_ms"] == 150, "south p95 within the cohort")
check(cohort["legacy_p95_latency_ms"] == 190, "legacy p95 within the cohort")
check(cohort["status"] == "pass", "cohort verdict")
check(report["verdict"] == "ready", "overall verdict")
PY
}

test_enforce_turns_a_report_into_a_gate() {
  local db="$test_root/enforce.sqlite"
  make_db "$db" "legacy|configured_legacy|10|200|null|0"
  printf '{ "upstreams": { "a": { "provider_call": "legacy" } } }\n' \
    >"$test_root/enforce-config.json"

  # Without --enforce the tool reports and exits zero: a zero here means "the
  # report was produced", not "the gate passed".
  python3 "$report_script" --db "$db" --config "$test_root/enforce-config.json" \
    >/dev/null || fail "reporting must not fail just because the verdict is bad"

  if python3 "$report_script" --db "$db" --config "$test_root/enforce-config.json" \
    --enforce >/dev/null 2>"$test_root/enforce.err"; then
    fail "--enforce must exit non-zero when the verdict is not ready"
  fi
  grep -q "not_ready" "$test_root/enforce.err" \
    || fail "--enforce must say which verdict it refused"
}

test_insufficient_data_is_refused_under_enforce_too() {
  # The dangerous confusion is treating "we could not measure" as "nothing is
  # wrong". Under --enforce they must be equally fatal.
  local db="$test_root/thin.sqlite"
  make_db "$db" "south_v1_streaming|null|150|200|null|0"
  printf '{ "upstreams": { "c": { "provider": "openai-compatible" } } }\n' \
    >"$test_root/thin-config.json"

  local verdict
  verdict="$(python3 "$report_script" --db "$db" --config "$test_root/thin-config.json" \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["verdict"])')"
  [[ "$verdict" == "insufficient_data" ]] \
    || fail "a one-attempt window must be insufficient_data, got $verdict"

  if python3 "$report_script" --db "$db" --config "$test_root/thin-config.json" \
    --enforce >/dev/null 2>&1; then
    fail "--enforce must refuse insufficient_data, not only fail"
  fi
}

test_zero_legacy_uses_the_sealed_baseline_instead_of_stalling() {
  # Zero legacy traffic is the migration working. If that left the performance
  # gate permanently unfinishable, success would be indistinguishable from
  # never having measured — so a sealed pre-cutover report stands in.
  local db="$test_root/migrated.sqlite"
  local rows=()
  local i
  for ((i = 1; i <= 300; i++)); do
    rows+=("south_v1_streaming|null|150|200|null|0")
  done
  make_db "$db" "${rows[@]}"
  printf '{ "upstreams": { "c": { "provider": "openai-compatible" } } }\n' \
    >"$test_root/migrated-config.json"

  # No baseline: honest about not having a Legacy side to compare against.
  python3 "$report_script" --db "$db" --config "$test_root/migrated-config.json" \
    >"$test_root/migrated.json" || fail "report failed on the migrated fixture"
  python3 - "$test_root/migrated.json" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
gate = report["performance_gate"]
assert gate["status"] == "insufficient_data", gate
assert gate["source"] == "live", gate
assert "--baseline" in gate["reason"], gate
assert report["usage_gate"]["zero_explicit_use"]["status"] == "pass", report
PY

  # With a sealed baseline that passed, the verdict carries.
  cat >"$test_root/sealed.json" <<'JSON'
{
  "window": { "until_ms": 1750000000000 },
  "performance_gate": {
    "status": "pass",
    "source": "live",
    "cohorts": { "compared": [{ "upstream": "up", "status": "pass" }], "skipped": [] }
  }
}
JSON
  python3 "$report_script" --db "$db" --config "$test_root/migrated-config.json" \
    --baseline "$test_root/sealed.json" --enforce >"$test_root/carried.json" \
    || fail "a sealed passing baseline must let the gate complete"
  python3 - "$test_root/carried.json" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
gate = report["performance_gate"]
assert gate["status"] == "pass", gate
assert gate["source"] == "sealed_baseline", gate
assert gate["sealed_window_end_ms"] == 1750000000000, gate
assert report["verdict"] == "ready", report
PY
}

test_a_cohort_too_thin_to_compare_is_reported_not_counted() {
  # The failure this whole gate reshaping exists to prevent: Legacy carries the
  # traffic South cannot, so a window-wide p95 compares two populations and
  # reads the difference as a transport regression. Here one upstream has a
  # real sample on both sides and another has almost none. The thin one must
  # be reported as unmeasured — not averaged into the verdict, and not
  # silently dropped, which would make "we did not measure this" look exactly
  # like "this is fine".
  local db="$test_root/cohorts.sqlite"
  local rows=()
  local i
  for ((i = 1; i <= 120; i++)); do rows+=("legacy|null|100|200|null|0|busy"); done
  for ((i = 1; i <= 120; i++)); do rows+=("south_v1_streaming|null|110|200|null|0|busy"); done
  # `thin` is where Legacy still handles something South barely sees.
  for ((i = 1; i <= 5; i++)); do rows+=("legacy|null|900|200|null|0|thin"); done
  for ((i = 1; i <= 2; i++)); do rows+=("south_v1_streaming|null|50|200|null|0|thin"); done
  make_db "$db" "${rows[@]}"
  printf '{ "upstreams": { "c": { "provider": "openai-compatible" } } }\n' \
    >"$test_root/cohorts-config.json"

  python3 "$report_script" --db "$db" --config "$test_root/cohorts-config.json" \
    --fallback-threshold 0.9 >"$test_root/cohorts.json" \
    || fail "report failed on the cohort fixture"

  python3 - "$test_root/cohorts.json" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
cohorts = report["performance_gate"]["cohorts"]
compared = {entry["upstream"]: entry for entry in cohorts["compared"]}
skipped = {entry["upstream"]: entry for entry in cohorts["skipped"]}
assert set(compared) == {"busy"}, compared
assert set(skipped) == {"thin"}, skipped
assert skipped["thin"]["status"] == "insufficient_data", skipped
assert skipped["thin"]["legacy_count"] == 5 and skipped["thin"]["south_count"] == 2, skipped
# The thin cohort's 900ms legacy attempts must not have been folded into the
# comparison; the busy cohort's own numbers decide it.
assert compared["busy"]["legacy_p95_latency_ms"] == 100, compared
assert compared["busy"]["status"] == "pass", compared
assert report["performance_gate"]["status"] == "pass", report["performance_gate"]
PY
}

test_ci_never_uploads_a_real_report() {
  # A real report names upstreams and carries usage shape. It is fine on the
  # maintainer's machine and not fine in a public artifact, and the difference
  # is one `upload-artifact` step away — so the boundary is a gate, not a
  # convention.
  local workflows="$project_root/.github/workflows"
  local offenders
  offenders="$(grep -rln "report-legacy-usage" "$workflows" || true)"
  local file
  for file in $offenders; do
    grep -q "upload-artifact" "$file" && {
      grep -A6 "report-legacy-usage" "$file" | grep -q "upload-artifact" \
        && fail "$file uploads a legacy usage report"
    }
    # CI may only run the synthetic-fixture test, never the reporter against a
    # real database.
    grep -qE "report-legacy-usage\.py" "$file" \
      && fail "$file runs the reporter directly; CI may only run its test"
  done
  [[ -n "$offenders" ]] || fail "no workflow runs the legacy usage test at all"
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
test_enforce_turns_a_report_into_a_gate
test_insufficient_data_is_refused_under_enforce_too
test_zero_legacy_uses_the_sealed_baseline_instead_of_stalling
test_a_cohort_too_thin_to_compare_is_reported_not_counted
test_ci_never_uploads_a_real_report
test_clean_config_and_small_fallback_share_is_ready
test_pre_south_database_gets_a_clear_error

echo "report-legacy-usage tests: PASS"

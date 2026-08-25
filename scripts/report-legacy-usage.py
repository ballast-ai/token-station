#!/usr/bin/env python3
"""Inventory legacy provider_call usage for the deletion gate.

Reads the metrics database and the client configuration, aggregates at the
attempt level (never by a request's last attempt, which systematically hides
legacy attempts that were retried onto South), and emits one JSON report with
a three-part gate verdict. Measurement only: this script deletes nothing.
"""

import argparse
import json
import math
import sqlite3
import sys
import time
from pathlib import Path

# The serialized default is omitted from configs; an upstream that spells it
# out is still on the default engine.
DEFAULT_PROVIDER_CALL = "south_v1_buffered_streaming_header_auth"
# Metrics vocabulary (attempts.provider_call_engine CHECK constraint).
SOUTH_ENGINES = ("south_v1_buffered", "south_v1_streaming")
LEGACY_ENGINE = "legacy"
UNKNOWN_ENGINE = "unknown"
# south_not_degraded needs a real legacy sample to compare against.
MIN_LEGACY_SAMPLE = 100
DEGRADATION_FACTOR = 1.2

REQUIRED_COLUMNS = {
    "requests": ("request_id", "started_at_ms"),
    "attempts": (
        "request_id",
        "latency_ms",
        "http_status",
        "error_code",
        "provider_call_engine",
        "south_fallback_reason",
    ),
}


def read_config(path: str) -> dict:
    try:
        document = json.loads(Path(path).read_text(encoding="utf-8"))
    except OSError as error:
        raise SystemExit(f"cannot read config {path}: {error}")
    except json.JSONDecodeError as error:
        raise SystemExit(f"config {path} is not valid JSON: {error}")

    upstreams = document.get("upstreams")
    if not isinstance(upstreams, dict):
        raise SystemExit(f"config {path} has no upstreams object")

    explicit_legacy = []
    explicit_non_default = {}
    for name, upstream in sorted(upstreams.items()):
        if not isinstance(upstream, dict):
            raise SystemExit(f"upstream {name} in {path} is not an object")
        value = upstream.get("provider_call")
        if value is None or value == DEFAULT_PROVIDER_CALL:
            continue
        explicit_non_default[name] = value
        if value == LEGACY_ENGINE:
            explicit_legacy.append(name)
    return {
        "explicit_legacy_upstreams": explicit_legacy,
        "explicit_non_default_provider_call": explicit_non_default,
    }


def require_schema(connection: sqlite3.Connection, path: str) -> None:
    for table, needed in REQUIRED_COLUMNS.items():
        rows = connection.execute(f"PRAGMA table_info({table})").fetchall()
        if not rows:
            raise SystemExit(
                f"metrics database {path} has no {table} table; "
                "it predates the current schema — run the gateway once to migrate"
            )
        have = {row[1] for row in rows}
        missing = sorted(set(needed) - have)
        if missing:
            raise SystemExit(
                f"metrics database {path}: table {table} lacks columns "
                f"{', '.join(missing)}; it predates the current schema — "
                "run the gateway once to migrate"
            )


def percentile(sorted_values: list, quantile: float):
    """Nearest-rank percentile over an already sorted list; None when empty."""
    if not sorted_values:
        return None
    rank = max(1, math.ceil(quantile * len(sorted_values)))
    return sorted_values[rank - 1]


def read_attempts(connection: sqlite3.Connection, since_ms: int) -> dict:
    # Attempts carry no timestamp of their own; the window comes from the
    # owning request. Rows with an empty request_id are exempt from the
    # requests unique index (pre-accounting-id rows) and would multiply the
    # join, so they are excluded from the window.
    cursor = connection.execute(
        """
        SELECT a.provider_call_engine, a.south_fallback_reason,
               a.latency_ms, a.http_status, a.error_code
        FROM attempts AS a
        JOIN requests AS r ON r.request_id = a.request_id
        WHERE r.request_id <> '' AND r.started_at_ms >= ?
        """,
        (since_ms,),
    )

    total = 0
    counts = {}
    error_counts = {}
    latencies = {}
    reasons = {}
    for engine, reason, latency_ms, http_status, error_code in cursor:
        total += 1
        counts[engine] = counts.get(engine, 0) + 1
        latencies.setdefault(engine, []).append(latency_ms)
        is_error = (
            error_code is not None or http_status is None or http_status >= 400
        )
        if is_error:
            error_counts[engine] = error_counts.get(engine, 0) + 1
        if reason is not None:
            reasons[reason] = reasons.get(reason, 0) + 1

    by_engine = {}
    for engine in sorted(counts):
        values = sorted(latencies[engine])
        by_engine[engine] = {
            "count": counts[engine],
            "error_count": error_counts.get(engine, 0),
            "p50_latency_ms": percentile(values, 0.50),
            "p95_latency_ms": percentile(values, 0.95),
        }

    # unknown attempts cannot be attributed to either path, so they stay out
    # of the denominator; native is a real (non-legacy) path and stays in.
    denominator = total - counts.get(UNKNOWN_ENGINE, 0)
    legacy_count = counts.get(LEGACY_ENGINE, 0)
    ratio = (legacy_count / denominator) if denominator > 0 else None

    south_latencies = sorted(
        value
        for engine in SOUTH_ENGINES
        for value in latencies.get(engine, [])
    )
    return {
        "total": total,
        "by_engine": by_engine,
        "by_south_fallback_reason": {key: reasons[key] for key in sorted(reasons)},
        "legacy_attempt_ratio": ratio,
        "_legacy_count": legacy_count,
        "_legacy_p95": percentile(sorted(latencies.get(LEGACY_ENGINE, [])), 0.95),
        "_south_p95": percentile(south_latencies, 0.95),
        "_south_count": len(south_latencies),
    }


def evaluate_gate(config: dict, attempts: dict, fallback_threshold: float) -> dict:
    configured_legacy_attempts = attempts["by_south_fallback_reason"].get(
        "configured_legacy", 0
    )
    zero_explicit_use = {
        "status": "pass"
        if not config["explicit_legacy_upstreams"] and configured_legacy_attempts == 0
        else "fail",
        "explicit_legacy_upstreams": config["explicit_legacy_upstreams"],
        "configured_legacy_attempts": configured_legacy_attempts,
    }

    ratio = attempts["legacy_attempt_ratio"]
    if ratio is None:
        fallback_below_threshold = {
            "status": "insufficient_data",
            "reason": "no attributable attempts in the window",
            "threshold": fallback_threshold,
        }
    else:
        fallback_below_threshold = {
            "status": "pass" if ratio <= fallback_threshold else "fail",
            "legacy_attempt_ratio": ratio,
            "threshold": fallback_threshold,
        }

    legacy_count = attempts["_legacy_count"]
    legacy_p95 = attempts["_legacy_p95"]
    south_p95 = attempts["_south_p95"]
    if legacy_count < MIN_LEGACY_SAMPLE:
        south_not_degraded = {
            "status": "insufficient_data",
            "reason": (
                f"only {legacy_count} legacy attempts in the window; "
                f"{MIN_LEGACY_SAMPLE} needed for a p95 comparison"
            ),
        }
    elif attempts["_south_count"] == 0:
        south_not_degraded = {
            "status": "insufficient_data",
            "reason": "no south attempts in the window to compare against",
        }
    else:
        limit = legacy_p95 * DEGRADATION_FACTOR
        south_not_degraded = {
            "status": "pass" if south_p95 <= limit else "fail",
            "south_p95_latency_ms": south_p95,
            "legacy_p95_latency_ms": legacy_p95,
            "limit_ms": limit,
            "factor": DEGRADATION_FACTOR,
        }

    return {
        "zero_explicit_use": zero_explicit_use,
        "fallback_below_threshold": fallback_below_threshold,
        "south_not_degraded": south_not_degraded,
    }


def verdict_of(gate: dict) -> str:
    statuses = [item["status"] for item in gate.values()]
    if "fail" in statuses:
        return "not_ready"
    if "insufficient_data" in statuses:
        return "insufficient_data"
    return "ready"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", required=True, help="path to metrics.sqlite")
    parser.add_argument("--config", required=True, help="path to token-station.json")
    parser.add_argument(
        "--since-days", type=int, default=30, help="measurement window (default 30)"
    )
    parser.add_argument(
        "--fallback-threshold",
        type=float,
        default=0.005,
        help="maximum legacy_attempt_ratio to pass (default 0.005)",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="emit JSON to stdout (the default and only output format)",
    )
    args = parser.parse_args()
    if args.since_days <= 0:
        raise SystemExit("--since-days must be positive")

    config = read_config(args.config)

    if not Path(args.db).is_file():
        raise SystemExit(f"metrics database not found: {args.db}")
    connection = sqlite3.connect(f"file:{args.db}?mode=ro", uri=True)
    try:
        require_schema(connection, args.db)
        now_ms = int(time.time() * 1000)
        since_ms = now_ms - args.since_days * 86_400_000
        attempts = read_attempts(connection, since_ms)
    except sqlite3.Error as error:
        raise SystemExit(f"metrics database {args.db}: {error}")
    finally:
        connection.close()

    gate = evaluate_gate(config, attempts, args.fallback_threshold)
    report = {
        "generated_at_ms": now_ms,
        "window": {"since_days": args.since_days, "since_ms": since_ms},
        "config": config,
        "attempts": {
            key: value for key, value in attempts.items() if not key.startswith("_")
        },
        "gate": gate,
        "verdict": verdict_of(gate),
    }
    json.dump(report, sys.stdout, indent=2)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()

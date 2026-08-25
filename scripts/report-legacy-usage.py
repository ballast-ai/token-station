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
# Draft thresholds. NONE OF THESE ARE APPROVED — the handoff plan reserves all
# four for lv, and until they are ruled on no Legacy removal plan may be
# written on the strength of a `ready` verdict from this tool. They are here so
# the mechanism can be built and tested, not so the numbers can be relied on.
MIN_LEGACY_SAMPLE = 100
# A one-sided minimum was the old shape: it asked for 100 legacy attempts and
# accepted a single South attempt as the thing to compare them against. Both
# sides of a comparison need enough samples for a p95 to mean anything.
MIN_SOUTH_SAMPLE = 100
DEGRADATION_FACTOR = 1.2
ERROR_RATE_MARGIN = 0.01

# A cohort is one (upstream, model, transport shape). Legacy carries the
# traffic South cannot — unusual dialects, credential shapes, media the
# component refuses — so a repository-wide p95 for each engine compares two
# different populations and calls the difference a regression in the
# transport. Only within a cohort are the two doing the same work.
COHORT_MIN_PER_SIDE = 30

REQUIRED_COLUMNS = {
    "requests": ("request_id", "started_at_ms"),
    "attempts": (
        "request_id",
        "upstream",
        "model",
        "latency_ms",
        "http_status",
        "error_code",
        "stream_outcome",
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
               a.latency_ms, a.http_status, a.error_code,
               a.upstream, a.model, a.stream_outcome
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
    cohorts = {}
    for (
        engine,
        reason,
        latency_ms,
        http_status,
        error_code,
        upstream,
        model,
        stream_outcome,
    ) in cursor:
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

        # The transport shape matters as much as the model: a streamed attempt
        # and a buffered one to the same upstream are not the same work.
        side = side_of(engine)
        if side is not None:
            key = (upstream, model, "streaming" if stream_outcome is not None else "buffered")
            bucket = cohorts.setdefault(key, {"south": [], "legacy": [], "errors": {"south": 0, "legacy": 0}})
            bucket[side].append(latency_ms)
            if is_error:
                bucket["errors"][side] += 1

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
        "_cohorts": cohorts,
    }


def side_of(engine: str):
    """Which side of the comparison an engine belongs to, or None.

    `unknown` is unattributable and `native` is neither transport, so neither
    belongs in a South-versus-Legacy latency comparison — but `native` still
    counts in the usage denominator, which is a different question.
    """
    if engine in SOUTH_ENGINES:
        return "south"
    if engine == LEGACY_ENGINE:
        return "legacy"
    return None


def compare_cohorts(cohorts: dict) -> dict:
    """Per-cohort South-versus-Legacy comparison, and what it adds up to.

    A cohort with too few attempts on either side is reported, not silently
    dropped: "we did not measure this" and "this is fine" must not look alike.
    """
    compared = []
    skipped = []
    for key in sorted(cohorts):
        upstream, model, shape = key
        bucket = cohorts[key]
        south = sorted(bucket["south"])
        legacy = sorted(bucket["legacy"])
        entry = {
            "upstream": upstream,
            "model": model,
            "transport": shape,
            "south_count": len(south),
            "legacy_count": len(legacy),
        }
        if len(south) < COHORT_MIN_PER_SIDE or len(legacy) < COHORT_MIN_PER_SIDE:
            entry["status"] = "insufficient_data"
            entry["minimum_per_side"] = COHORT_MIN_PER_SIDE
            skipped.append(entry)
            continue
        south_p95 = percentile(south, 0.95)
        legacy_p95 = percentile(legacy, 0.95)
        south_errors = bucket["errors"]["south"] / len(south)
        legacy_errors = bucket["errors"]["legacy"] / len(legacy)
        entry.update(
            {
                "south_p50_latency_ms": percentile(south, 0.50),
                "legacy_p50_latency_ms": percentile(legacy, 0.50),
                "south_p95_latency_ms": south_p95,
                "legacy_p95_latency_ms": legacy_p95,
                "south_error_rate": south_errors,
                "legacy_error_rate": legacy_errors,
                "p95_limit_ms": legacy_p95 * DEGRADATION_FACTOR,
                "error_rate_limit": legacy_errors + ERROR_RATE_MARGIN,
            }
        )
        entry["status"] = (
            "pass"
            if south_p95 <= entry["p95_limit_ms"]
            and south_errors <= entry["error_rate_limit"]
            else "fail"
        )
        compared.append(entry)
    return {"compared": compared, "skipped": skipped}


def evaluate_gates(
    config: dict, attempts: dict, fallback_threshold: float, baseline: dict | None
) -> dict:
    """The two gates, kept apart because they answer different questions.

    The usage gate asks whether anything still needs Legacy. The performance
    gate asks whether removing it would cost anything. A single verdict over
    both hid which one was blocking, and they fail for entirely different
    reasons: one is a migration fact, the other a measurement.
    """
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

    usage_gate = {
        "zero_explicit_use": zero_explicit_use,
        "fallback_below_threshold": fallback_below_threshold,
    }

    performance_gate = evaluate_performance(attempts, baseline)
    return {"usage_gate": usage_gate, "performance_gate": performance_gate}


def evaluate_performance(attempts: dict, baseline: dict | None) -> dict:
    """South against Legacy, within comparable cohorts.

    Zero Legacy traffic is the migration succeeding, so it must not leave this
    gate permanently unfinishable — that would make success indistinguishable
    from never having measured. A sealed pre-migration report stands in: it
    holds cohorts measured when both sides still carried real traffic.
    """
    legacy_count = attempts["_legacy_count"]
    south_count = attempts["_south_count"]

    if legacy_count == 0:
        if baseline is None:
            return {
                "status": "insufficient_data",
                "source": "live",
                "reason": (
                    "no legacy attempts in the window, which is the migration "
                    "working; supply --baseline with a report sealed before the "
                    "cutover so the comparison has a Legacy side"
                ),
                "south_attempts": south_count,
            }
        sealed = baseline.get("performance_gate", {})
        return {
            "status": sealed.get("status", "insufficient_data"),
            "source": "sealed_baseline",
            "sealed_window_end_ms": baseline.get("window", {}).get("until_ms"),
            "reason": (
                "no legacy attempts in the window; verdict carried from the "
                "sealed pre-migration baseline"
            ),
            "cohorts": sealed.get("cohorts", {}),
        }

    if legacy_count < MIN_LEGACY_SAMPLE or south_count < MIN_SOUTH_SAMPLE:
        return {
            "status": "insufficient_data",
            "source": "live",
            "reason": (
                f"{legacy_count} legacy and {south_count} south attempts in the "
                f"window; both sides need at least {MIN_LEGACY_SAMPLE} and "
                f"{MIN_SOUTH_SAMPLE}"
            ),
            "legacy_attempts": legacy_count,
            "south_attempts": south_count,
        }

    cohorts = compare_cohorts(attempts["_cohorts"])
    if not cohorts["compared"]:
        return {
            "status": "insufficient_data",
            "source": "live",
            "reason": (
                "no cohort had enough attempts on both sides; an overall p95 is "
                "not a substitute, because Legacy carries the traffic South "
                "cannot and the two populations differ"
            ),
            "cohorts": cohorts,
        }
    return {
        "status": "pass"
        if all(entry["status"] == "pass" for entry in cohorts["compared"])
        else "fail",
        "source": "live",
        "cohorts": cohorts,
        "factor": DEGRADATION_FACTOR,
        "error_rate_margin": ERROR_RATE_MARGIN,
    }


def verdict_of(gates: dict) -> str:
    statuses = [item["status"] for item in gates["usage_gate"].values()]
    statuses.append(gates["performance_gate"]["status"])
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
    parser.add_argument(
        "--baseline",
        default=None,
        help=(
            "a report sealed before the cutover, used as the Legacy side once "
            "live legacy traffic reaches zero"
        ),
    )
    parser.add_argument(
        "--enforce",
        action="store_true",
        help=(
            "exit non-zero unless the verdict is `ready`; without it the tool "
            "reports and says nothing about what to do"
        ),
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

    baseline = None
    if args.baseline is not None:
        try:
            baseline = json.loads(Path(args.baseline).read_text(encoding="utf-8"))
        except OSError as error:
            raise SystemExit(f"baseline {args.baseline}: {error}")
        except json.JSONDecodeError as error:
            raise SystemExit(f"baseline {args.baseline} is not JSON: {error}")

    gates = evaluate_gates(config, attempts, args.fallback_threshold, baseline)
    verdict = verdict_of(gates)
    report = {
        "generated_at_ms": now_ms,
        "window": {
            "since_days": args.since_days,
            "since_ms": since_ms,
            "until_ms": now_ms,
        },
        "config": config,
        "attempts": {
            key: value for key, value in attempts.items() if not key.startswith("_")
        },
        "usage_gate": gates["usage_gate"],
        "performance_gate": gates["performance_gate"],
        "thresholds_approved": False,
        "verdict": verdict,
    }
    json.dump(report, sys.stdout, indent=2)
    sys.stdout.write("\n")

    # `--enforce` is what a pipeline uses. Without it the tool is a report and
    # a zero exit means "the report was produced", not "the gate passed" —
    # which is exactly the confusion that let a skipped release job read as a
    # successful one.
    if args.enforce and verdict != "ready":
        print(
            f"legacy removal gate: {verdict}",
            file=sys.stderr,
        )
        raise SystemExit(1)


if __name__ == "__main__":
    main()

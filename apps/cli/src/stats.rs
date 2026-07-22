//! `stats`: read-only aggregation over the metrics store.
//!
//! Opens the same `SQLite` file [`crate::store::SqliteStore`] writes — with
//! `SQLITE_OPEN_READ_ONLY`, so a running `serve` is never contended with and a
//! typo in a query can never mutate history. Aggregation happens here in Rust
//! rather than in SQL: the row counts are personal-use sized, and a percentile
//! computed in one visible place is a percentile a test can pin down.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use token_station_metrics::SCHEMA_VERSION;

/// What `--by` groups on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Upstream,
    Model,
    Pool,
    Status,
}

impl GroupBy {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Upstream => "upstream",
            Self::Model => "model",
            Self::Pool => "pool",
            Self::Status => "status",
        }
    }
}

/// One bucket's numbers. `requests` counts every exchange, including the ones
/// that failed before a routing decision existed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aggregate {
    pub requests: u64,
    /// Failed exchanges: an error code was recorded *or* the caller saw >= 400.
    /// Both terms matter — a mid-stream failure keeps its committed 200.
    pub errors: u64,
    pub p50_latency_ms: u64,
    pub p95_latency_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// `None` until any row carries a cost (the pricing table is C2#4).
    pub cost_micros: Option<i64>,
}

/// The whole answer: totals, plus one bucket per group when `--by` was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub total: Aggregate,
    pub groups: Vec<(String, Aggregate)>,
}

/// One request's routing receipt: who asked, what actually served, why, and how
/// it ended — content-free, straight from the same store `collect` aggregates.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Receipt {
    pub request_id: String,
    pub started_at_ms: u64,
    pub latency_ms: u64,
    pub status: u16,
    pub error_code: Option<String>,
    /// A short, content-free reason refining `error_code` — most usefully the
    /// specific unmet capability behind a `capability` refusal. `None` when the
    /// code alone said everything.
    pub error_detail: Option<String>,
    pub requested_model: String,
    /// The upstream and model that actually served (or last refused).
    pub upstream: Option<String>,
    pub model: Option<String>,
    pub pool: Option<String>,
    /// How the pool was chosen: rule / hint / heuristic / default / `exact_model`.
    pub tier: Option<String>,
    pub attempts: u32,
    pub cost_micros: Option<i64>,
}

/// Parses `--since`: `all`, `<N>h`, or `<N>d` — returned as a window in
/// milliseconds, `None` meaning all history.
///
/// # Errors
///
/// Names the spec and the accepted shapes.
pub fn parse_since(spec: &str) -> Result<Option<u64>, String> {
    if spec == "all" {
        return Ok(None);
    }
    let refused = || format!("`{spec}` is not a window; use all, <N>h or <N>d");
    let (number, unit) = spec.split_at(spec.len().saturating_sub(1));
    let count: u64 = number.parse().map_err(|_| refused())?;
    let hours = match unit {
        "h" => count,
        "d" => count.saturating_mul(24),
        _ => return Err(refused()),
    };
    Ok(Some(hours.saturating_mul(60 * 60 * 1000)))
}

/// Aggregates the store at `db_path`, keeping rows with
/// `started_at_ms >= cutoff_ms`.
///
/// # Errors
///
/// A missing store (nothing has ever been recorded), an unreadable file, or a
/// schema version this build does not know — mirroring the write side's
/// refusal, because summing columns that changed meaning is worse than an
/// error.
pub fn collect(
    db_path: &Path,
    cutoff_ms: Option<u64>,
    group_by: Option<GroupBy>,
) -> Result<Report, String> {
    if !db_path.exists() {
        return Err(format!(
            "no metrics store at `{}` — it is created when `serve` first runs with data.metrics \
             on",
            db_path.display()
        ));
    }
    let connection = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("metrics store `{}`: {error}", db_path.display()))?;

    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| format!("metrics store version: {error}"))?;
    if version != SCHEMA_VERSION {
        return Err(format!(
            "metrics store `{}` has schema version {version}, this build knows {SCHEMA_VERSION}",
            db_path.display()
        ));
    }

    let mut statement = connection
        .prepare(
            "SELECT latency_ms, status, error_code, upstream, model, pool,
                    input_tokens, output_tokens, cost_micros
             FROM requests WHERE started_at_ms >= ?1",
        )
        .map_err(|error| format!("metrics query: {error}"))?;
    let rows = statement
        .query_map(
            [i64::try_from(cutoff_ms.unwrap_or(0)).unwrap_or(i64::MAX)],
            |row| {
                // SQLite integers are i64; the store wrote these as saturating
                // non-negatives, so the narrowing back is total.
                let narrow = |value: i64| u64::try_from(value).unwrap_or(0);
                Ok(Row {
                    latency_ms: narrow(row.get::<_, i64>(0)?),
                    status: row.get::<_, u16>(1)?,
                    error_code: row.get::<_, Option<String>>(2)?,
                    upstream: row.get::<_, Option<String>>(3)?,
                    model: row.get::<_, Option<String>>(4)?,
                    pool: row.get::<_, Option<String>>(5)?,
                    input_tokens: row.get::<_, Option<i64>>(6)?.map(narrow),
                    output_tokens: row.get::<_, Option<i64>>(7)?.map(narrow),
                    cost_micros: row.get::<_, Option<i64>>(8)?,
                })
            },
        )
        .and_then(Iterator::collect::<Result<Vec<Row>, _>>)
        .map_err(|error| format!("metrics query: {error}"))?;

    let total = aggregate(rows.iter());
    let groups = match group_by {
        None => Vec::new(),
        Some(by) => {
            let mut buckets: BTreeMap<String, Vec<&Row>> = BTreeMap::new();
            for row in &rows {
                buckets.entry(row.key(by)).or_default().push(row);
            }
            buckets
                .into_iter()
                .map(|(key, rows)| (key, aggregate(rows.into_iter())))
                .collect()
        }
    };

    Ok(Report { total, groups })
}

/// The most recent `limit` requests, newest first — the per-request receipts.
///
/// # Errors
///
/// The store is missing, at a schema version this build does not know, or the
/// query fails.
pub fn recent(db_path: &Path, limit: u32) -> Result<Vec<Receipt>, String> {
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let connection = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("metrics store `{}`: {error}", db_path.display()))?;

    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| format!("metrics store version: {error}"))?;
    if version != SCHEMA_VERSION {
        return Err(format!(
            "metrics store `{}` has schema version {version}, this build knows {SCHEMA_VERSION}",
            db_path.display()
        ));
    }

    let mut statement = connection
        .prepare(
            "SELECT request_id, started_at_ms, latency_ms, status, error_code,
                    requested_model, upstream, model, pool, tier, attempts, cost_micros,
                    error_detail
             FROM requests ORDER BY id DESC LIMIT ?1",
        )
        .map_err(|error| format!("metrics query: {error}"))?;
    let narrow = |value: i64| u64::try_from(value).unwrap_or(0);
    let receipts = statement
        .query_map([i64::from(limit)], |row| {
            Ok(Receipt {
                request_id: row.get::<_, String>(0)?,
                started_at_ms: narrow(row.get::<_, i64>(1)?),
                latency_ms: narrow(row.get::<_, i64>(2)?),
                status: row.get::<_, u16>(3)?,
                error_code: row.get::<_, Option<String>>(4)?,
                requested_model: row.get::<_, String>(5)?,
                upstream: row.get::<_, Option<String>>(6)?,
                model: row.get::<_, Option<String>>(7)?,
                pool: row.get::<_, Option<String>>(8)?,
                tier: row.get::<_, Option<String>>(9)?,
                attempts: row.get::<_, i64>(10)?.try_into().unwrap_or(0),
                cost_micros: row.get::<_, Option<i64>>(11)?,
                error_detail: row.get::<_, Option<String>>(12)?,
            })
        })
        .map_err(|error| format!("metrics query: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("metrics row: {error}"))?;
    Ok(receipts)
}

/// Renders a report as an aligned table, totals last.
#[must_use]
pub fn render(report: &Report, group_by: Option<GroupBy>) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut header = vec![
        group_by.map_or_else(String::new, |by| by.label().to_uppercase()),
        "REQUESTS".to_owned(),
        "ERRORS".to_owned(),
        "P50 MS".to_owned(),
        "P95 MS".to_owned(),
        "TOKENS IN".to_owned(),
        "TOKENS OUT".to_owned(),
        "COST".to_owned(),
    ];
    if group_by.is_none() {
        header.remove(0);
    }
    rows.push(header);

    let line = |label: Option<&str>, bucket: &Aggregate| {
        let mut row = vec![
            bucket.requests.to_string(),
            format!(
                "{} ({}%)",
                bucket.errors,
                percentage(bucket.errors, bucket.requests)
            ),
            bucket.p50_latency_ms.to_string(),
            bucket.p95_latency_ms.to_string(),
            bucket.input_tokens.to_string(),
            bucket.output_tokens.to_string(),
            bucket
                .cost_micros
                .map_or_else(|| "—".to_owned(), |micros| format!("{micros}µ")),
        ];
        if let Some(label) = label {
            row.insert(0, label.to_owned());
        }
        row
    };

    for (key, bucket) in &report.groups {
        rows.push(line(Some(key), bucket));
    }
    rows.push(line(group_by.map(|_| "(total)"), &report.total));

    let columns = rows[0].len();
    let mut widths = vec![0usize; columns];
    for row in &rows {
        for (column, cell) in row.iter().enumerate() {
            widths[column] = widths[column].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in &rows {
        for (column, cell) in row.iter().enumerate() {
            if column + 1 == columns {
                out.push_str(cell);
                out.push('\n');
            } else {
                let _ = write!(out, "{cell:width$}  ", width = widths[column]);
            }
        }
    }
    if report.total.cost_micros.is_none() {
        out.push_str("cost: no priced rows yet — the pricing table arrives with account binding\n");
    }
    out
}

struct Row {
    latency_ms: u64,
    status: u16,
    error_code: Option<String>,
    upstream: Option<String>,
    model: Option<String>,
    pool: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cost_micros: Option<i64>,
}

impl Row {
    fn is_error(&self) -> bool {
        self.error_code.is_some() || self.status >= 400
    }

    /// The bucket this row lands in; requests that failed before a routing
    /// decision existed have no upstream/model/pool and bucket together.
    fn key(&self, by: GroupBy) -> String {
        let unrouted = || "(unrouted)".to_owned();
        match by {
            GroupBy::Upstream => self.upstream.clone().unwrap_or_else(unrouted),
            GroupBy::Model => self.model.clone().unwrap_or_else(unrouted),
            GroupBy::Pool => self.pool.clone().unwrap_or_else(unrouted),
            GroupBy::Status => self.status.to_string(),
        }
    }
}

fn aggregate<'a>(rows: impl Iterator<Item = &'a Row>) -> Aggregate {
    let mut latencies = Vec::new();
    let mut bucket = Aggregate {
        requests: 0,
        errors: 0,
        p50_latency_ms: 0,
        p95_latency_ms: 0,
        input_tokens: 0,
        output_tokens: 0,
        cost_micros: None,
    };
    for row in rows {
        bucket.requests += 1;
        bucket.errors += u64::from(row.is_error());
        latencies.push(row.latency_ms);
        bucket.input_tokens = bucket
            .input_tokens
            .saturating_add(row.input_tokens.unwrap_or(0));
        bucket.output_tokens = bucket
            .output_tokens
            .saturating_add(row.output_tokens.unwrap_or(0));
        if let Some(cost) = row.cost_micros {
            bucket.cost_micros = Some(bucket.cost_micros.unwrap_or(0).saturating_add(cost));
        }
    }
    latencies.sort_unstable();
    bucket.p50_latency_ms = percentile(&latencies, 50);
    bucket.p95_latency_ms = percentile(&latencies, 95);
    bucket
}

/// Nearest-rank percentile over an already-sorted slice; 0 for no data.
fn percentile(sorted: &[u64], rank: u64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let count = sorted.len() as u64;
    let position = (rank * count).div_ceil(100).max(1) - 1;
    sorted[usize::try_from(position)
        .unwrap_or(sorted.len() - 1)
        .min(sorted.len() - 1)]
}

fn percentage(part: u64, whole: u64) -> u64 {
    (part * 100).checked_div(whole).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{GroupBy, collect, parse_since};
    use crate::store::SqliteStore;
    use std::path::PathBuf;
    use token_station_metrics::{Recorder, RequestRecord, RoutingRecord};
    use token_station_router_core::{DecidedBy, RequestFeatures};

    fn record(
        started_at_ms: u64,
        latency_ms: u64,
        status: u16,
        upstream: Option<&str>,
        tokens: Option<(u64, u64)>,
    ) -> RequestRecord {
        let mut record = RequestRecord::begin(started_at_ms, "openai-chat-completions");
        record.latency_ms = latency_ms;
        record.status = status;
        record.requested_model = "auto".to_owned();
        record.attempts = u32::from(upstream.is_some());
        record.routing = upstream.map(|name| RoutingRecord {
            upstream: name.to_owned(),
            model: "m1".to_owned(),
            pool: "main".to_owned(),
            decided_by: DecidedBy::Default,
            fallbacks: 0,
            features: RequestFeatures::default(),
        });
        record.usage = tokens.map(|(input, output)| token_station_protocol::Usage {
            input_tokens: input,
            output_tokens: output,
            ..token_station_protocol::Usage::default()
        });
        if status >= 400 {
            record.error_code = Some(token_station_protocol::ErrorCode::UpstreamUnavailable);
        }
        record
    }

    /// A store with a known population: 19 fast successes 10..=190ms, one slow
    /// 1000ms failure, and one pre-routing refusal from before the window.
    fn fixture(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("ts-stats-{}-{name}.sqlite", std::process::id()));
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        for step in 1..=19u64 {
            store.record(&record(
                2_000_000,
                step * 10,
                200,
                Some("mock_primary"),
                Some((100, 10)),
            ));
        }
        store.record(&record(2_000_000, 1000, 502, Some("mock_backup"), None));
        store.record(&record(1_000, 5, 400, None, None));
        path
    }

    #[test]
    fn totals_percentiles_and_the_error_definition_come_out_right() {
        let path = fixture("totals");

        let report = collect(&path, None, None).expect("collects");
        assert_eq!(report.total.requests, 21);
        assert_eq!(report.total.errors, 2, "the 502 and the pre-routing 400");
        assert_eq!(report.total.input_tokens, 1900);
        assert_eq!(report.total.output_tokens, 190);
        // 21 sorted latencies: [5, 10..=190 by 10, 1000]. Nearest rank:
        // p50 -> 11th value = 100, p95 -> 20th value = 190.
        assert_eq!(report.total.p50_latency_ms, 100);
        assert_eq!(report.total.p95_latency_ms, 190);
        assert_eq!(report.total.cost_micros, None, "no pricing table yet");

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn a_mid_stream_failure_with_a_committed_200_counts_as_an_error() {
        let path =
            std::env::temp_dir().join(format!("ts-stats-{}-midstream.sqlite", std::process::id()));
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        let mut broken = record(1, 50, 200, Some("mock_primary"), None);
        broken.error_code = Some(token_station_protocol::ErrorCode::UpstreamUnavailable);
        store.record(&broken);

        let report = collect(&path, None, None).expect("collects");
        assert_eq!(report.total.errors, 1, "status alone would lie");

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn the_window_cutoff_excludes_older_rows() {
        let path = fixture("window");

        let report = collect(&path, Some(2_000_000), None).expect("collects");
        assert_eq!(report.total.requests, 20, "the 400 predates the window");
        assert_eq!(report.total.errors, 1);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn grouping_buckets_unrouted_requests_visibly() {
        let path = fixture("groups");

        let report = collect(&path, None, Some(GroupBy::Upstream)).expect("collects");
        let keys: Vec<&str> = report.groups.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, ["(unrouted)", "mock_backup", "mock_primary"]);
        let primary = &report.groups[2].1;
        assert_eq!(primary.requests, 19);
        assert_eq!(primary.errors, 0);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn a_missing_store_is_a_clear_error_not_an_empty_report() {
        let missing = std::env::temp_dir().join("ts-stats-never-created.sqlite");

        let error = collect(&missing, None, None).expect_err("nothing to read");
        assert!(error.contains("data.metrics"), "{error}");
    }

    #[test]
    fn a_newer_schema_is_refused_not_misread() {
        let path =
            std::env::temp_dir().join(format!("ts-stats-{}-newer.sqlite", std::process::id()));
        std::fs::remove_file(&path).ok();
        {
            let connection = rusqlite::Connection::open(&path).expect("opens");
            connection
                .pragma_update(None, "user_version", 99)
                .expect("stamps");
        }

        let error = collect(&path, None, None).expect_err("version 99 is not ours");
        assert!(error.contains("99"), "{error}");

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn since_windows_parse_and_garbage_is_refused() {
        assert_eq!(parse_since("all"), Ok(None));
        assert_eq!(parse_since("24h"), Ok(Some(24 * 60 * 60 * 1000)));
        assert_eq!(parse_since("7d"), Ok(Some(7 * 24 * 60 * 60 * 1000)));
        assert!(parse_since("soon").is_err());
        assert!(parse_since("h").is_err());
        assert!(parse_since("").is_err());
    }

    #[test]
    fn rendering_smoke_test_marks_unpriced_cost() {
        let path = fixture("render");

        let report = collect(&path, None, Some(GroupBy::Upstream)).expect("collects");
        let rendered = super::render(&report, Some(GroupBy::Upstream));
        assert!(rendered.contains("UPSTREAM"), "{rendered}");
        assert!(rendered.contains("(total)"), "{rendered}");
        assert!(rendered.contains("no priced rows yet"), "{rendered}");

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn recent_returns_the_newest_requests_first_with_routing() {
        let path =
            std::env::temp_dir().join(format!("ts-stats-recent-{}.sqlite", std::process::id()));
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        for step in 1..=3u64 {
            let mut record = record(
                2_000_000,
                step * 10,
                200,
                Some("openai_personal"),
                Some((10, 5)),
            );
            record.request_id = format!("req_{step}");
            store.record(&record);
        }
        drop(store);

        let receipts = super::recent(&path, 2).expect("reads");
        assert_eq!(receipts.len(), 2, "limit honored");
        assert_eq!(receipts[0].request_id, "req_3", "newest first");
        assert_eq!(receipts[1].request_id, "req_2");
        assert_eq!(receipts[0].upstream.as_deref(), Some("openai_personal"));
        assert_eq!(receipts[0].tier.as_deref(), Some("default"));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn recent_on_a_missing_store_is_empty_not_an_error() {
        let path = std::env::temp_dir().join("ts-stats-recent-absent.sqlite");
        std::fs::remove_file(&path).ok();
        assert!(
            super::recent(&path, 5)
                .expect("no store is empty")
                .is_empty()
        );
    }
}

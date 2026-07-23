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
    Agent,
    Upstream,
    Model,
    Pool,
    Status,
    Hour,
    Day,
}

impl GroupBy {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Upstream => "upstream",
            Self::Model => "model",
            Self::Pool => "pool",
            Self::Status => "status",
            Self::Hour => "hour",
            Self::Day => "day",
        }
    }
}

/// Optional exact-match dimensions applied before aggregation. `source` is
/// the inbound adapter protocol stored on the Receipt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatsFilter<'a> {
    pub agent_id: Option<&'a str>,
    pub source: Option<&'a str>,
    pub upstream: Option<&'a str>,
    pub model: Option<&'a str>,
}

/// One bucket's numbers. `requests` counts every exchange, including the ones
/// that failed before a routing decision existed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Aggregate {
    pub requests: u64,
    /// Failed exchanges: an error code was recorded *or* the caller saw >= 400.
    /// Both terms matter — a mid-stream failure keeps its committed 200.
    pub errors: u64,
    pub p50_latency_ms: u64,
    pub p95_latency_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Provider-side cache reads and writes partition `input_tokens`; they are
    /// exposed for efficiency analysis and must not be added to total tokens.
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// Hidden reasoning is a subset of `output_tokens` for supported providers.
    pub reasoning_tokens: u64,
    /// `None` until any row carries a cost (the pricing table is C2#4).
    pub cost_micros: Option<i64>,
    /// Requests carrying a stable numeric cost versus requests whose model
    /// was not covered by the price table.
    pub priced_requests: u64,
    pub unpriced_requests: u64,
}

/// The whole answer: totals, plus one bucket per group when `--by` was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub total: Aggregate,
    pub groups: Vec<(String, Aggregate)>,
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

/// Resolve a relative window into an absolute inclusive cutoff.
///
/// Keeping this conversion beside [`parse_since`] prevents UI/admin callers
/// from accidentally treating a duration such as `86_400_000` as a Unix
/// timestamp near the epoch.
pub fn cutoff_from_since(spec: &str, now_ms: u64) -> Result<Option<u64>, String> {
    parse_since(spec).map(|window| window.map(|duration| now_ms.saturating_sub(duration)))
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
    collect_range(db_path, cutoff_ms, None, group_by)
}

/// Aggregates the half-open time range `[start_ms, end_ms)`. This is used by
/// fixed budget periods so receipts from a later period cannot inflate an
/// expired period's usage.
///
/// # Errors
///
/// Returns the same read/schema errors as [`collect`], and rejects a reversed
/// range before opening the store.
pub fn collect_range(
    db_path: &Path,
    start_ms: Option<u64>,
    end_ms: Option<u64>,
    group_by: Option<GroupBy>,
) -> Result<Report, String> {
    collect_filtered(db_path, start_ms, end_ms, group_by, StatsFilter::default())
}

/// Aggregates a half-open time range after exact Agent/source filters.
///
/// # Errors
///
/// Returns the same range, store, and schema errors as [`collect_range`].
pub fn collect_filtered(
    db_path: &Path,
    start_ms: Option<u64>,
    end_ms: Option<u64>,
    group_by: Option<GroupBy>,
    filter: StatsFilter<'_>,
) -> Result<Report, String> {
    if start_ms
        .zip(end_ms)
        .is_some_and(|(start, end)| start >= end)
    {
        return Err("stats range end_ms must be after start_ms".to_string());
    }
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
            "SELECT latency_ms, status, error_code, agent_id, upstream, model, pool,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                    reasoning_tokens, cost_micros,
                    CAST(strftime('%s',
                        strftime('%Y-%m-%d %H:00:00', started_at_ms / 1000, 'unixepoch', 'localtime'),
                        'utc') AS INTEGER) * 1000,
                    CAST(strftime('%s',
                        date(started_at_ms / 1000, 'unixepoch', 'localtime'),
                        'utc') AS INTEGER) * 1000
             FROM requests
             WHERE started_at_ms >= ?1
               AND (?2 IS NULL OR started_at_ms < ?2)
               AND (?3 IS NULL OR agent_id = ?3)
               AND (?4 IS NULL OR protocol = ?4)
               AND (?5 IS NULL OR upstream = ?5)
               AND (?6 IS NULL OR model = ?6)",
        )
        .map_err(|error| format!("metrics query: {error}"))?;
    let rows = statement
        .query_map(
            rusqlite::params![
                i64::try_from(start_ms.unwrap_or(0)).unwrap_or(i64::MAX),
                end_ms.map(|value| i64::try_from(value).unwrap_or(i64::MAX)),
                filter.agent_id,
                filter.source,
                filter.upstream,
                filter.model,
            ],
            |row| {
                // SQLite integers are i64; the store wrote these as saturating
                // non-negatives, so the narrowing back is total.
                let narrow = |value: i64| u64::try_from(value).unwrap_or(0);
                Ok(Row {
                    latency_ms: narrow(row.get::<_, i64>(0)?),
                    status: row.get::<_, u16>(1)?,
                    error_code: row.get::<_, Option<String>>(2)?,
                    agent_id: row.get::<_, Option<String>>(3)?,
                    upstream: row.get::<_, Option<String>>(4)?,
                    model: row.get::<_, Option<String>>(5)?,
                    pool: row.get::<_, Option<String>>(6)?,
                    input_tokens: row.get::<_, Option<i64>>(7)?.map(narrow),
                    output_tokens: row.get::<_, Option<i64>>(8)?.map(narrow),
                    cache_read_tokens: row.get::<_, Option<i64>>(9)?.map(narrow),
                    cache_write_tokens: row.get::<_, Option<i64>>(10)?.map(narrow),
                    reasoning_tokens: row.get::<_, Option<i64>>(11)?.map(narrow),
                    cost_micros: row.get::<_, Option<i64>>(12)?,
                    hour_bucket_ms: narrow(row.get::<_, i64>(13)?),
                    day_bucket_ms: narrow(row.get::<_, i64>(14)?),
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
    agent_id: Option<String>,
    upstream: Option<String>,
    model: Option<String>,
    pool: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
    cost_micros: Option<i64>,
    hour_bucket_ms: u64,
    day_bucket_ms: u64,
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
            GroupBy::Agent => self.agent_id.clone().unwrap_or_else(unrouted),
            GroupBy::Upstream => self.upstream.clone().unwrap_or_else(unrouted),
            GroupBy::Model => self.model.clone().unwrap_or_else(unrouted),
            GroupBy::Pool => self.pool.clone().unwrap_or_else(unrouted),
            GroupBy::Status => self.status.to_string(),
            GroupBy::Hour => self.hour_bucket_ms.to_string(),
            GroupBy::Day => self.day_bucket_ms.to_string(),
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
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        cost_micros: None,
        priced_requests: 0,
        unpriced_requests: 0,
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
        bucket.cache_read_tokens = bucket
            .cache_read_tokens
            .saturating_add(row.cache_read_tokens.unwrap_or(0));
        bucket.cache_write_tokens = bucket
            .cache_write_tokens
            .saturating_add(row.cache_write_tokens.unwrap_or(0));
        bucket.reasoning_tokens = bucket
            .reasoning_tokens
            .saturating_add(row.reasoning_tokens.unwrap_or(0));
        if let Some(cost) = row.cost_micros {
            bucket.cost_micros = Some(bucket.cost_micros.unwrap_or(0).saturating_add(cost));
            bucket.priced_requests = bucket.priced_requests.saturating_add(1);
        } else {
            bucket.unpriced_requests = bucket.unpriced_requests.saturating_add(1);
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
    use super::{GroupBy, StatsFilter, collect, parse_since};
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
    fn cache_and_reasoning_are_aggregated_without_changing_total_token_semantics() {
        let path = std::env::temp_dir().join(format!(
            "ts-stats-{}-usage-breakdown.sqlite",
            std::process::id()
        ));
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        let mut value = record(
            1_800_000_000_000,
            20,
            200,
            Some("provider"),
            Some((1_000, 300)),
        );
        value.request_id = "usage-breakdown".to_string();
        value.usage = Some(token_station_protocol::Usage {
            input_tokens: 1_000,
            output_tokens: 300,
            cache_read_tokens: 400,
            cache_write_tokens: 120,
            reasoning_tokens: 80,
        });
        store.record(&value);

        let report = collect(&path, None, None).expect("collects");
        assert_eq!(
            report.total.input_tokens + report.total.output_tokens,
            1_300
        );
        assert_eq!(report.total.cache_read_tokens, 400);
        assert_eq!(report.total.cache_write_tokens, 120);
        assert_eq!(report.total.reasoning_tokens, 80);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn relative_windows_resolve_to_absolute_cutoffs() {
        assert_eq!(
            super::cutoff_from_since("24h", 2 * 86_400_000).unwrap(),
            Some(86_400_000)
        );
        assert_eq!(
            super::cutoff_from_since("all", 2 * 86_400_000).unwrap(),
            None
        );
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
    fn fixed_budget_window_excludes_receipts_at_or_after_its_end() {
        let path = std::env::temp_dir().join(format!(
            "ts-stats-{}-fixed-budget-window.sqlite",
            std::process::id()
        ));
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        for started_at_ms in [99, 100, 199, 200] {
            store.record(&record(started_at_ms, 1, 200, Some("provider"), None));
        }

        let report = super::collect_range(&path, Some(100), Some(200), None)
            .expect("collects a half-open fixed period");
        assert_eq!(report.total.requests, 2);
        assert!(super::collect_range(&path, Some(200), Some(100), None).is_err());
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
    fn agent_groups_keep_priced_and_unpriced_request_counts_distinct() {
        let path = std::env::temp_dir().join(format!(
            "ts-stats-{}-agent-budgets.sqlite",
            std::process::id()
        ));
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        let mut priced = record(10, 1, 200, Some("provider"), None);
        priced.agent_id = Some("codex".to_string());
        priced.cost_kind = token_station_metrics::CostKind::Estimated;
        priced.cost_micros = Some(750_000);
        priced.price_version = Some(1);
        store.record(&priced);
        let mut unknown = record(10, 1, 200, Some("provider"), None);
        unknown.agent_id = Some("codex".to_string());
        store.record(&unknown);
        let mut other = priced.clone();
        other.request_id = "other-agent".to_string();
        other.agent_id = Some("opencode".to_string());
        other.cost_micros = Some(5);
        store.record(&other);

        let report = collect(&path, None, Some(GroupBy::Agent)).expect("groups by Agent");
        let codex = report
            .groups
            .iter()
            .find(|(agent, _)| agent == "codex")
            .map(|(_, aggregate)| aggregate)
            .unwrap();
        assert_eq!(codex.cost_micros, Some(750_000));
        assert_eq!(codex.priced_requests, 1);
        assert_eq!(codex.unpriced_requests, 1);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn agent_and_inbound_source_filters_are_exact_and_composable() {
        let path = std::env::temp_dir().join(format!(
            "ts-stats-{}-agent-source-filters.sqlite",
            std::process::id()
        ));
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        for (request_id, agent, protocol) in [
            ("codex-responses", "codex", "openai-responses"),
            ("codex-chat", "codex", "openai-chat-completions"),
            ("opencode-chat", "opencode", "openai-chat-completions"),
        ] {
            let mut value = record(10, 1, 200, Some("provider"), None);
            value.request_id = request_id.to_string();
            value.agent_id = Some(agent.to_string());
            value.protocol = protocol.to_string();
            store.record(&value);
        }

        let report = super::collect_filtered(
            &path,
            None,
            None,
            None,
            StatsFilter {
                agent_id: Some("codex"),
                source: Some("openai-chat-completions"),
                ..StatsFilter::default()
            },
        )
        .expect("filters exact Agent and inbound protocol source");
        assert_eq!(report.total.requests, 1);

        let source_only = super::collect_filtered(
            &path,
            None,
            None,
            Some(GroupBy::Agent),
            StatsFilter {
                agent_id: None,
                source: Some("openai-chat-completions"),
                ..StatsFilter::default()
            },
        )
        .unwrap();
        assert_eq!(source_only.total.requests, 2);
        assert_eq!(source_only.groups.len(), 2);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn upstream_and_model_filters_compose_and_time_groups_are_ordered() {
        let path = std::env::temp_dir().join(format!(
            "ts-stats-{}-dashboard-filters.sqlite",
            std::process::id()
        ));
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        for (request_id, timestamp, upstream, model) in [
            ("wanted-1", 1_800_000_000_000, "openai", "gpt-5"),
            ("wanted-2", 1_800_090_000_000, "openai", "gpt-5"),
            ("other-model", 1_800_180_000_000, "openai", "gpt-4"),
            ("other-upstream", 1_800_270_000_000, "backup", "gpt-5"),
        ] {
            let mut value = record(timestamp, 1, 200, Some(upstream), Some((10, 2)));
            value.request_id = request_id.to_string();
            if let Some(routing) = &mut value.routing {
                routing.model = model.to_string();
            }
            store.record(&value);
        }

        let filtered = super::collect_filtered(
            &path,
            None,
            None,
            Some(GroupBy::Hour),
            StatsFilter {
                upstream: Some("openai"),
                model: Some("gpt-5"),
                ..StatsFilter::default()
            },
        )
        .expect("filters exact upstream and model");
        assert_eq!(filtered.total.requests, 2);
        assert_eq!(filtered.groups.len(), 2);
        assert!(filtered.groups[0].0 < filtered.groups[1].0);

        let days = collect(&path, None, Some(GroupBy::Day)).expect("groups by local day");
        assert!(days.groups.len() >= 2);
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
}

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

/// Exact percentiles require retaining the selected latencies. Refuse an
/// unbounded historical scan instead of allowing a local admin request to
/// consume memory proportional to an operator-controlled database.
pub(crate) const MAX_STATS_ROWS: usize = 100_000;

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
    /// The transport engine of the attempt that settled the request.
    Engine,
    /// Why that attempt ran on legacy instead of South (`(south)` when it
    /// did not fall back).
    Fallback,
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
            Self::Engine => "engine",
            Self::Fallback => "fallback",
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
    /// Usage-bearing rows whose input value predates canonical versioning.
    pub legacy_input_requests: u64,
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
    pub missing_price_requests: u64,
    pub missing_usage_requests: u64,
    pub actual_cost_requests: u64,
    pub estimated_cost_requests: u64,
    pub cache_read_reported_requests: u64,
    pub cache_write_reported_requests: u64,
    pub input_reported_requests: u64,
    pub output_reported_requests: u64,
    /// Requests with both input and output reported, including explicit zeros.
    pub total_reported_requests: u64,
    pub incomplete_usage_requests: u64,
    /// Successful exchanges or exchanges with at least one observed usage field.
    pub usage_expected_requests: u64,
    /// Failed or cancelled exchanges without any observed usage field.
    pub failed_without_usage_requests: u64,
    /// The subset without a recorded cost. Missing usage does not mean free.
    pub unpriced_failed_without_usage_requests: u64,
    /// Usage population rows with no field-presence metadata or positive cache value.
    pub cache_read_unrecorded_requests: u64,
    pub cache_write_unrecorded_requests: u64,
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
///
/// # Errors
///
/// Returns the same validation error as [`parse_since`] when `spec` is not
/// `all`, `<N>h`, or `<N>d`.
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

    let rows = select_rows(&connection, start_ms, end_ms, filter)?;

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

/// Reads the receipts in `[start_ms, end_ms)` that pass `filter`, with the
/// engine and fallback reason of each request's last attempt.
fn select_rows(
    connection: &Connection,
    start_ms: Option<u64>,
    end_ms: Option<u64>,
    filter: StatsFilter<'_>,
) -> Result<Vec<Row>, String> {
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
                        'utc') AS INTEGER) * 1000,
                    (SELECT provider_call_engine FROM attempts
                      WHERE attempts.request_id = requests.request_id
                      ORDER BY ordinal DESC LIMIT 1),
                    (SELECT south_fallback_reason FROM attempts
                      WHERE attempts.request_id = requests.request_id
                      ORDER BY ordinal DESC LIMIT 1),
                    usage_semantics, usage_observation, cost_kind
             FROM requests
             WHERE started_at_ms >= ?1
               AND (?2 IS NULL OR started_at_ms < ?2)
               AND (?3 IS NULL OR agent_id = ?3)
               AND (?4 IS NULL OR protocol = ?4)
               AND (?5 IS NULL OR upstream = ?5)
               AND (?6 IS NULL OR model = ?6)",
        )
        .map_err(|error| format!("metrics query: {error}"))?;
    let mapped = statement
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
                    engine: row.get::<_, Option<String>>(15)?,
                    fallback_reason: row.get::<_, Option<String>>(16)?,
                    legacy_input: row.get::<_, String>(17)? == "provider_reported_v1",
                    observation: crate::store::read_usage_observation(row, 18)?,
                    cost_kind: row.get(19)?,
                })
            },
        )
        .map_err(|error| format!("metrics query: {error}"))?;
    let mut rows = Vec::new();
    for row in mapped {
        enforce_stats_row_limit(rows.len().saturating_add(1))?;
        rows.push(row.map_err(|error| format!("metrics query: {error}"))?);
    }
    Ok(rows)
}

fn enforce_stats_row_limit(selected_rows: usize) -> Result<(), String> {
    if selected_rows > MAX_STATS_ROWS {
        return Err(format!(
            "stats query selects more than {MAX_STATS_ROWS} receipts; narrow the time window or \
             add an exact filter"
        ));
    }
    Ok(())
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
    if report.total.legacy_input_requests > 0 {
        let _ = writeln!(
            out,
            "input: {} historical request(s) use provider-reported semantics; their input totals may exclude cache tokens",
            report.total.legacy_input_requests
        );
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
    /// From the last attempt; `None` when no upstream was ever tried.
    engine: Option<String>,
    fallback_reason: Option<String>,
    legacy_input: bool,
    observation: Option<token_station_metrics::UsageObservation>,
    cost_kind: String,
}

impl Row {
    fn is_error(&self) -> bool {
        self.status != 499 && (self.error_code.is_some() || self.status >= 400)
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
            GroupBy::Engine => self.engine.clone().unwrap_or_else(unrouted),
            GroupBy::Fallback => match (&self.fallback_reason, self.engine.as_deref()) {
                (Some(reason), _) => reason.clone(),
                (None, Some(engine)) if engine.starts_with("south") => "(south)".to_owned(),
                // Legacy without a reason: the attempt predates the receipt
                // field, or ran before South was the default.
                (None, Some(_)) => "(unrecorded)".to_owned(),
                (None, None) => unrouted(),
            },
        }
    }
}

fn has_observed_usage(row: &Row) -> bool {
    row.observation.map_or_else(
        || {
            [
                row.input_tokens,
                row.output_tokens,
                row.cache_read_tokens,
                row.cache_write_tokens,
                row.reasoning_tokens,
            ]
            .iter()
            .any(Option::is_some)
        },
        |value| {
            [
                value.input_tokens,
                value.output_tokens,
                value.cache_read_tokens,
                value.cache_write_tokens,
                value.cache_write_5m_tokens,
                value.cache_write_1h_tokens,
                value.reasoning_tokens,
            ]
            .iter()
            .any(Option::is_some)
        },
    )
}

fn record_usage_population(bucket: &mut Aggregate, row: &Row) -> bool {
    // Cancellation remains outside the existing error count, but not a success.
    let successful = (200..300).contains(&row.status) && row.error_code.is_none();
    let usage_expected = successful || has_observed_usage(row);
    bucket.usage_expected_requests += u64::from(usage_expected);
    bucket.failed_without_usage_requests += u64::from(!usage_expected);
    bucket.unpriced_failed_without_usage_requests +=
        u64::from(!usage_expected && row.cost_micros.is_none());
    bucket.cache_read_unrecorded_requests += u64::from(
        usage_expected
            && row.observation.is_none()
            && row.cache_read_tokens.is_none_or(|value| value == 0),
    );
    bucket.cache_write_unrecorded_requests += u64::from(
        usage_expected
            && row.observation.is_none()
            && row.cache_write_tokens.is_none_or(|value| value == 0),
    );
    usage_expected
}

fn aggregate<'a>(rows: impl Iterator<Item = &'a Row>) -> Aggregate {
    let mut latencies = Vec::new();
    let mut bucket = Aggregate {
        requests: 0,
        errors: 0,
        p50_latency_ms: 0,
        p95_latency_ms: 0,
        input_tokens: 0,
        legacy_input_requests: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        cost_micros: None,
        priced_requests: 0,
        unpriced_requests: 0,
        ..Aggregate::default()
    };
    for row in rows {
        bucket.requests += 1;
        bucket.errors += u64::from(row.is_error());
        latencies.push(row.latency_ms);
        let input = row
            .observation
            .map_or(row.input_tokens, |value| value.input_tokens);
        let output = row
            .observation
            .map_or(row.output_tokens, |value| value.output_tokens);
        let usage_expected = record_usage_population(&mut bucket, row);
        bucket.input_reported_requests += u64::from(input.is_some());
        bucket.output_reported_requests += u64::from(output.is_some());
        bucket.total_reported_requests += u64::from(input.is_some() && output.is_some());
        bucket.incomplete_usage_requests +=
            u64::from(usage_expected && row.observation.is_some_and(|value| value.incomplete));
        // Historical positive values prove presence. Historical zeros do not.
        bucket.cache_read_reported_requests += u64::from(
            row.observation
                .is_some_and(|value| value.cache_read_tokens.is_some())
                || row.cache_read_tokens.is_some_and(|value| value > 0),
        );
        bucket.cache_write_reported_requests += u64::from(
            row.observation
                .is_some_and(|value| value.cache_write_tokens.is_some())
                || row.cache_write_tokens.is_some_and(|value| value > 0),
        );
        bucket.input_tokens = bucket.input_tokens.saturating_add(input.unwrap_or(0));
        if row.input_tokens.is_some() && row.legacy_input {
            bucket.legacy_input_requests = bucket.legacy_input_requests.saturating_add(1);
        }
        bucket.output_tokens = bucket.output_tokens.saturating_add(output.unwrap_or(0));
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
            if row.cost_kind == "actual" {
                bucket.actual_cost_requests += 1;
            } else {
                bucket.estimated_cost_requests += 1;
            }
        } else {
            bucket.unpriced_requests = bucket.unpriced_requests.saturating_add(1);
            let mut usage = token_station_protocol::Usage {
                input_tokens: input.unwrap_or(0),
                output_tokens: output.unwrap_or(0),
                cache_read_tokens: row.cache_read_tokens.unwrap_or(0),
                cache_write_tokens: row.cache_write_tokens.unwrap_or(0),
                reasoning_tokens: row.reasoning_tokens.unwrap_or(0),
                ..token_station_protocol::Usage::default()
            };
            if let Some(observation) = row.observation {
                observation.apply(&mut usage);
            }
            let complete_usage = input.is_some()
                && output.is_some()
                && crate::accounting::can_estimate(&usage, row.observation);
            if complete_usage {
                bucket.missing_price_requests += 1;
            } else {
                bucket.missing_usage_requests += 1;
            }
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
    use super::{
        GroupBy, MAX_STATS_ROWS, StatsFilter, collect, enforce_stats_row_limit, parse_since,
    };
    use crate::store::SqliteStore;
    use std::path::PathBuf;
    use token_station_metrics::{RecordedDecidedBy, Recorder, RequestRecord, RoutingRecord};
    use token_station_router_core::RequestFeatures;

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
            decided_by: RecordedDecidedBy::Default,
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
    #[test]
    fn coverage_distinguishes_missing_prices_usage_and_reported_zero_cache() {
        let path =
            std::env::temp_dir().join(format!("ts-stats-{}-coverage.sqlite", std::process::id()));
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).unwrap();
        let mut priced = record(1, 1, 200, Some("p"), Some((10, 2)));
        priced.cost_kind = token_station_metrics::CostKind::Actual;
        priced.cost_micros = Some(1);
        priced.usage_observation = Some(token_station_metrics::UsageObservation {
            input_tokens: Some(10),
            output_tokens: Some(2),
            cache_write_tokens: Some(0),
            ..token_station_metrics::UsageObservation::default()
        });
        store.record(&priced);
        store.record(&record(2, 1, 200, Some("p"), Some((20, 3))));
        store.record(&record(3, 1, 429, Some("p"), None));
        let result = collect(&path, None, None).unwrap().total;
        assert_eq!(
            (
                result.requests,
                result.priced_requests,
                result.unpriced_requests
            ),
            (3, 1, 2)
        );
        assert_eq!(
            (result.missing_price_requests, result.missing_usage_requests),
            (1, 1)
        );
        assert_eq!(
            (result.actual_cost_requests, result.estimated_cost_requests),
            (1, 0)
        );
        assert_eq!(result.cache_write_reported_requests, 1);
        assert_eq!(result.cache_read_reported_requests, 0);
        assert_eq!(result.cost_micros, Some(1));
        drop(store);
        std::fs::remove_file(path).ok();
    }

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
    fn usage_population_separates_successful_legacy_usage_from_no_usage_failures() {
        let path = std::env::temp_dir().join(format!(
            "ts-stats-{}-usage-population.sqlite",
            std::process::id()
        ));
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).unwrap();
        for index in 0..85 {
            let mut row = record(index, 1, 200, Some("p"), Some((100, 2)));
            row.usage.as_mut().unwrap().cache_read_tokens = 80;
            store.record(&row);
        }
        for (status, count) in [(429, 77), (404, 14), (502, 5), (499, 3)] {
            for index in 0..count {
                store.record(&record(100 + index, 1, status, Some("p"), None));
            }
        }
        let result = collect(&path, None, None).unwrap().total;
        assert_eq!((result.requests, result.errors), (184, 96));
        assert_eq!(result.usage_expected_requests, 85);
        assert_eq!(result.failed_without_usage_requests, 99);
        assert_eq!(result.unpriced_failed_without_usage_requests, 99);
        assert_eq!(
            (
                result.input_reported_requests,
                result.output_reported_requests,
                result.cache_read_reported_requests
            ),
            (85, 85, 85)
        );
        assert_eq!(
            (
                result.cache_read_unrecorded_requests,
                result.cache_write_unrecorded_requests
            ),
            (0, 85)
        );
        assert_eq!(
            (
                result.input_tokens,
                result.output_tokens,
                result.cache_read_tokens
            ),
            (8500, 170, 6800)
        );
        assert_eq!(
            (
                result.missing_price_requests,
                result.missing_usage_requests,
                result.cost_micros
            ),
            (85, 99, None)
        );
        drop(store);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn usage_population_keeps_failed_partial_usage_successful_missing_usage_and_actual_cost() {
        use token_station_metrics::UsageObservation;
        let path = std::env::temp_dir().join(format!(
            "ts-stats-{}-partial-population.sqlite",
            std::process::id()
        ));
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).unwrap();
        let mut partial = record(1, 1, 499, Some("p"), Some((10, 0)));
        partial.usage_observation = Some(UsageObservation {
            input_tokens: Some(10),
            incomplete: true,
            ..UsageObservation::default()
        });
        store.record(&partial);
        store.record(&record(2, 1, 200, Some("p"), None));
        let mut priced_failure = record(3, 1, 502, Some("p"), None);
        priced_failure.cost_micros = Some(7);
        priced_failure.cost_kind = token_station_metrics::CostKind::Actual;
        store.record(&priced_failure);
        let mut cache_only = record(4, 1, 502, Some("p"), Some((0, 0)));
        cache_only.usage_observation = Some(UsageObservation {
            cache_read_tokens: Some(0),
            incomplete: true,
            ..UsageObservation::default()
        });
        store.record(&cache_only);
        let mut empty_failure = record(5, 1, 200, Some("p"), None);
        empty_failure.error_code = Some(token_station_protocol::ErrorCode::UpstreamUnavailable);
        empty_failure.usage_observation = Some(UsageObservation {
            incomplete: true,
            ..UsageObservation::default()
        });
        store.record(&empty_failure);
        let result = collect(&path, None, None).unwrap().total;
        assert_eq!((result.requests, result.errors), (5, 3));
        assert_eq!(result.usage_expected_requests, 3);
        assert_eq!(result.failed_without_usage_requests, 2);
        assert_eq!(result.unpriced_failed_without_usage_requests, 1);
        assert_eq!((result.input_tokens, result.output_tokens), (10, 0));
        assert_eq!(result.incomplete_usage_requests, 2);
        assert_eq!(
            (result.cost_micros, result.actual_cost_requests),
            (Some(7), 1)
        );
        drop(store);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn token_reporting_coverage_preserves_partial_snapshots_and_legacy_values() {
        use token_station_metrics::UsageObservation;
        let path = std::env::temp_dir().join(format!(
            "ts-stats-{}-token-coverage.sqlite",
            std::process::id()
        ));
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).unwrap();
        let mut zero = record(1, 1, 200, Some("p"), Some((0, 0)));
        zero.usage_observation = Some(UsageObservation {
            input_tokens: Some(0),
            output_tokens: Some(0),
            cache_write_tokens: Some(0),
            ..UsageObservation::default()
        });
        store.record(&zero);
        let mut input_only = record(2, 1, 200, Some("p"), Some((10, 99)));
        input_only.usage_observation = Some(UsageObservation {
            input_tokens: Some(10),
            ..UsageObservation::default()
        });
        store.record(&input_only);
        let mut partial = record(3, 1, 502, Some("p"), Some((99, 5)));
        partial.usage_observation = Some(UsageObservation {
            output_tokens: Some(5),
            incomplete: true,
            ..UsageObservation::default()
        });
        store.record(&partial);
        store.record(&record(4, 1, 429, Some("p"), None));
        store.record(&record(5, 1, 200, Some("p"), Some((0, 0))));
        let mut invalid = record(6, 1, 200, Some("p"), Some((2, 1)));
        invalid.usage.as_mut().unwrap().cache_read_tokens = 3;
        invalid.usage_observation = Some(UsageObservation {
            input_tokens: Some(2),
            output_tokens: Some(1),
            cache_read_tokens: Some(3),
            ..UsageObservation::default()
        });
        store.record(&invalid);
        let result = collect(&path, None, None).unwrap().total;
        assert_eq!((result.input_tokens, result.output_tokens), (12, 6));
        assert_eq!(
            (
                result.input_reported_requests,
                result.output_reported_requests,
                result.total_reported_requests
            ),
            (4, 4, 3)
        );
        assert_eq!(result.incomplete_usage_requests, 1);
        assert_eq!(
            (result.missing_price_requests, result.missing_usage_requests),
            (2, 4)
        );
        assert_eq!(
            result.cache_write_reported_requests, 1,
            "historical cache zero stays unknown"
        );
        let only = super::collect_range(&path, Some(2), Some(3), None)
            .unwrap()
            .total;
        assert_eq!(
            (
                only.input_reported_requests,
                only.output_reported_requests,
                only.total_reported_requests
            ),
            (1, 0, 0)
        );
        drop(store);
        std::fs::remove_file(path).ok();
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
    fn stats_scan_has_a_hard_process_memory_boundary() {
        assert!(enforce_stats_row_limit(MAX_STATS_ROWS).is_ok());
        let error = enforce_stats_row_limit(MAX_STATS_ROWS + 1).unwrap_err();
        assert!(error.contains("narrow the time window"));
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
            ..token_station_protocol::Usage::default()
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
    fn mixed_history_reports_how_many_input_values_are_legacy() {
        let path = std::env::temp_dir().join(format!(
            "ts-stats-{}-mixed-usage-semantics.sqlite",
            std::process::id()
        ));
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        let mut canonical = record(10, 1, 200, Some("provider"), Some((150, 5)));
        canonical.request_id = "canonical".to_owned();
        store.record(&canonical);
        let mut legacy = record(11, 1, 200, Some("provider"), Some((30, 5)));
        legacy.request_id = "legacy".to_owned();
        store.record(&legacy);
        drop(store);
        rusqlite::Connection::open(&path)
            .expect("opens")
            .execute(
                "UPDATE requests SET usage_semantics='provider_reported_v1' WHERE request_id='legacy'",
                [],
            )
            .expect("marks released semantics");

        let report = collect(&path, None, None).expect("collects");
        assert_eq!(report.total.input_tokens, 180);
        assert_eq!(report.total.legacy_input_requests, 1);
        let rendered = super::render(&report, None);
        assert!(
            rendered.contains("1 historical request(s) use provider-reported semantics"),
            "{rendered}"
        );

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
    fn a_client_cancel_is_counted_as_a_request_but_not_an_error() {
        let path = std::env::temp_dir().join(format!(
            "ts-stats-{}-client-cancel.sqlite",
            std::process::id()
        ));
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        store.record(&record(1, 50, 499, Some("mock_primary"), None));

        let report = collect(&path, None, None).expect("collects");
        assert_eq!(report.total.requests, 1);
        assert_eq!(report.total.errors, 0);

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
    fn engine_and_fallback_groups_read_the_attempt_that_settled_the_request() {
        use token_station_metrics::{AttemptRecord, ProviderCallEngine, SouthFallbackReason};
        let path = std::env::temp_dir().join(format!(
            "ts-stats-{}-engine-groups.sqlite",
            std::process::id()
        ));
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        let attempt = |ordinal: u32, engine: ProviderCallEngine, reason| AttemptRecord {
            ordinal,
            upstream: "provider".to_owned(),
            model: "m1".to_owned(),
            latency_ms: 1,
            http_status: Some(200),
            error_code: None,
            stream_outcome: None,
            provider_call_engine: engine,
            south_fallback_reason: reason,
            fallback_allowed: false,
        };
        // Served on South after a legacy first attempt: the last attempt wins.
        let mut south = record(10, 1, 200, Some("provider"), None);
        south.request_id = "south".to_owned();
        south.attempt_records = vec![
            attempt(
                1,
                ProviderCallEngine::Legacy,
                Some(SouthFallbackReason::Headers),
            ),
            attempt(2, ProviderCallEngine::SouthV1Streaming, None),
        ];
        store.record(&south);
        let mut pinned = record(10, 1, 200, Some("provider"), None);
        pinned.request_id = "pinned".to_owned();
        pinned.attempt_records = vec![attempt(
            1,
            ProviderCallEngine::Legacy,
            Some(SouthFallbackReason::ConfiguredLegacy),
        )];
        store.record(&pinned);
        let mut old = record(10, 1, 200, Some("provider"), None);
        old.request_id = "old".to_owned();
        old.attempt_records = vec![attempt(1, ProviderCallEngine::Legacy, None)];
        store.record(&old);
        let mut unrouted = record(10, 1, 400, None, None);
        unrouted.request_id = "unrouted".to_owned();
        store.record(&unrouted);

        let engines = collect(&path, None, Some(GroupBy::Engine)).expect("groups by engine");
        let keys: Vec<&str> = engines.groups.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, ["(unrouted)", "legacy", "south_v1_streaming"]);
        assert_eq!(engines.groups[1].1.requests, 2);

        let reasons = collect(&path, None, Some(GroupBy::Fallback)).expect("groups by reason");
        let keys: Vec<&str> = reasons.groups.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            keys,
            ["(south)", "(unrecorded)", "(unrouted)", "configured_legacy"]
        );
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

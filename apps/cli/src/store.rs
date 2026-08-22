//! The local metrics store: one `SQLite` database, on by default, holding
//! request metadata and nothing else.
//!
//! Created on first start, written on every exchange, and — the
//! part that matters — **structurally unable to hold content**: every column
//! is drawn from `token-station-metrics`' `RequestRecord`, whose fields are
//! numbers, closed enums, or operator-configured names. There is no column a
//! prompt could go into. The integration tests dump the raw database file and
//! assert a canary that was present in the request body never reached disk.
//!
//! This is also the ledger the reconciliation command (C3#1) diffs against
//! platform bills, which is why writes happen even for failed exchanges: an
//! upstream that billed a request the client thinks failed is exactly the
//! discrepancy worth finding.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Row, Transaction, named_params, types::Type,
};
use token_station_metrics::{
    AttemptRecord, ConversionOutcome, ConversionReasonCode, ConversionReasonDetail,
    ConversionRecord, ConversionStage, CostKind, DecisionRecord, ProviderCallEngine,
    QuotaDecisionSnapshot, ReceiptView, RecordedDecidedBy, Recorder, RequestPathKind,
    RequestRecord, RoutingRecord, SCHEMA_VERSION,
};
use token_station_protocol::{ErrorCode, HintKind, StreamOutcome, Usage};
use token_station_router_core::RequestFeatures;

/// One forward, idempotent step from schema `to - 1` to schema `to`.
struct Migration {
    to: u32,
    sql: &'static str,
}

/// Read-only compatibility result used by recovery surfaces before the data
/// plane is allowed to open or migrate the metrics store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaCompatibility {
    Missing,
    Current { version: u32 },
    Older { found: u32, supported: u32 },
    Newer { found: u32, supported: u32 },
}

/// Inspects only `PRAGMA user_version` through a read-only `SQLite` connection.
/// It never creates or migrates the store.
///
/// # Errors
///
/// Returns an error when an existing store cannot be opened read-only or its
/// schema version cannot be read.
pub fn inspect_schema(path: &Path) -> Result<SchemaCompatibility, String> {
    if !path.exists() {
        return Ok(SchemaCompatibility::Missing);
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("metrics store `{}`: {error}", path.display()))?;
    let found: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| format!("metrics store version: {error}"))?;
    Ok(match found.cmp(&SCHEMA_VERSION) {
        std::cmp::Ordering::Less => SchemaCompatibility::Older {
            found,
            supported: SCHEMA_VERSION,
        },
        std::cmp::Ordering::Equal => SchemaCompatibility::Current { version: found },
        std::cmp::Ordering::Greater => SchemaCompatibility::Newer {
            found,
            supported: SCHEMA_VERSION,
        },
    })
}

/// Creates a consistent `SQLite` snapshot without interpreting or migrating the
/// source schema. This remains usable for a database written by a newer build.
///
/// # Errors
///
/// Returns an error when the source cannot be opened read-only or `SQLite`'s
/// online backup cannot write the destination.
pub fn snapshot_database(source: &Path, destination: &Path) -> Result<(), String> {
    let connection = Connection::open_with_flags(
        source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("metrics store `{}`: {error}", source.display()))?;
    connection
        .backup(rusqlite::MAIN_DB, destination, None)
        .map_err(|error| {
            format!(
                "metrics snapshot `{}` -> `{}`: {error}",
                source.display(),
                destination.display()
            )
        })
}

/// The ordered migration registry. An older store is brought up to
/// [`SCHEMA_VERSION`] by applying every migration whose `to` it has not yet
/// reached — never by re-creating the whole store and losing rows. Each step
/// runs in its own transaction and stamps `user_version` only on commit, so an
/// interrupted additive or table-rebuild migration resumes from the prior
/// version rather than observing a partial schema.
const MIGRATIONS: &[Migration] = &[
    Migration {
        // v1 → v2: the stable accounting id (see B-6). Existing rows keep the
        // empty default, which the partial unique index exempts.
        to: 2,
        sql: "
            ALTER TABLE requests ADD COLUMN request_id TEXT NOT NULL DEFAULT '';
            CREATE UNIQUE INDEX IF NOT EXISTS requests_request_id
                ON requests (request_id) WHERE request_id <> '';
        ",
    },
    Migration {
        // v2 → v3: the price table version a cost was computed under. NULL on
        // existing rows means their cost predates versioned pricing.
        to: 3,
        sql: "ALTER TABLE requests ADD COLUMN price_version INTEGER;",
    },
    Migration {
        // v3 -> v4: the normalized request receipt. Existing flat rows remain
        // readable and deliberately receive no invented child events.
        to: 4,
        sql: "
            ALTER TABLE requests ADD COLUMN agent_id TEXT;
            ALTER TABLE requests ADD COLUMN running_revision INTEGER;
            ALTER TABLE requests ADD COLUMN cost_kind TEXT NOT NULL DEFAULT 'unknown'
                CHECK (cost_kind IN ('actual', 'estimated', 'unknown'));

            UPDATE requests
               SET cost_micros = NULL, price_version = NULL, cost_kind = 'unknown'
             WHERE cost_micros IS NULL OR cost_micros < 0 OR price_version IS NULL;
            UPDATE requests
               SET cost_kind = 'estimated'
             WHERE cost_micros >= 0 AND price_version IS NOT NULL;

            CREATE TABLE decisions (
                request_id TEXT PRIMARY KEY,
                upstream TEXT NOT NULL,
                model TEXT NOT NULL,
                pool TEXT NOT NULL,
                decision_kind TEXT NOT NULL
                    CHECK (decision_kind IN ('rule', 'hint', 'heuristic', 'default', 'exact_model')),
                rule_id TEXT,
                hint_kind TEXT,
                hint_value TEXT,
                heuristic_score INTEGER,
                heuristic_threshold INTEGER,
                fallbacks INTEGER NOT NULL,
                est_input_tokens INTEGER NOT NULL,
                message_count INTEGER NOT NULL,
                tool_count INTEGER NOT NULL,
                has_images INTEGER NOT NULL,
                requires_json_schema INTEGER NOT NULL,
                code_block_count INTEGER NOT NULL,
                requested_max_output_tokens INTEGER,
                hint_count INTEGER NOT NULL,
                reasoning_marker_count INTEGER NOT NULL,
                technical_term_count INTEGER NOT NULL,
                simple_indicator_count INTEGER NOT NULL,
                code_keyword_count INTEGER NOT NULL,
                math_term_count INTEGER NOT NULL,
                creative_term_count INTEGER NOT NULL,
                multi_step_signal INTEGER NOT NULL,
                question_count INTEGER NOT NULL,
                system_format_hint INTEGER NOT NULL
            );

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
                fallback_allowed INTEGER NOT NULL,
                PRIMARY KEY (request_id, ordinal)
            );

            CREATE TABLE conversion_reports (
                request_id TEXT NOT NULL,
                ordinal INTEGER NOT NULL,
                stage TEXT NOT NULL CHECK (stage IN (
                    'inbound_normalize', 'provider_request', 'provider_response',
                    'outbound_render', 'stream_translate'
                )),
                source_protocol TEXT NOT NULL,
                target_protocol TEXT NOT NULL,
                succeeded INTEGER NOT NULL,
                error_code TEXT,
                PRIMARY KEY (request_id, ordinal)
            );
        ",
    },
    Migration {
        // v4 -> v5: difficulty tokens that exclude the system prompt, split out
        // from the whole-request est_input_tokens so scoring reflects the user's
        // turn, not the agent's harness. Legacy rows default to 0 (no split
        // recorded); their est_input_tokens still stands as the whole-request
        // figure.
        to: 5,
        sql: "ALTER TABLE decisions ADD COLUMN conversation_tokens INTEGER NOT NULL DEFAULT 0;",
    },
    Migration {
        // v5 -> v6: admit the `quota` decision kind. A CHECK constraint cannot be
        // altered in place, so the table is rebuilt with the widened check and
        // its rows copied over. The copy names every column explicitly (not
        // `SELECT *`): `conversation_tokens` was appended by an `ALTER` in v5, so
        // in a database migrated up it is the last column, not the middle one the
        // fresh schema declares — a positional copy would shift every column.
        to: 6,
        sql: "
            DROP TABLE IF EXISTS decisions_v6;
            CREATE TABLE decisions_v6 (
                request_id TEXT PRIMARY KEY,
                upstream TEXT NOT NULL,
                model TEXT NOT NULL,
                pool TEXT NOT NULL,
                decision_kind TEXT NOT NULL
                    CHECK (decision_kind IN ('rule', 'hint', 'heuristic', 'default', 'exact_model', 'quota')),
                rule_id TEXT,
                hint_kind TEXT,
                hint_value TEXT,
                heuristic_score INTEGER,
                heuristic_threshold INTEGER,
                fallbacks INTEGER NOT NULL,
                est_input_tokens INTEGER NOT NULL,
                conversation_tokens INTEGER NOT NULL DEFAULT 0,
                message_count INTEGER NOT NULL,
                tool_count INTEGER NOT NULL,
                has_images INTEGER NOT NULL,
                requires_json_schema INTEGER NOT NULL,
                code_block_count INTEGER NOT NULL,
                requested_max_output_tokens INTEGER,
                hint_count INTEGER NOT NULL,
                reasoning_marker_count INTEGER NOT NULL,
                technical_term_count INTEGER NOT NULL,
                simple_indicator_count INTEGER NOT NULL,
                code_keyword_count INTEGER NOT NULL,
                math_term_count INTEGER NOT NULL,
                creative_term_count INTEGER NOT NULL,
                multi_step_signal INTEGER NOT NULL,
                question_count INTEGER NOT NULL,
                system_format_hint INTEGER NOT NULL
            );
            INSERT INTO decisions_v6 (
                request_id, upstream, model, pool, decision_kind, rule_id, hint_kind,
                hint_value, heuristic_score, heuristic_threshold, fallbacks,
                est_input_tokens, conversation_tokens, message_count, tool_count, has_images,
                requires_json_schema, code_block_count, requested_max_output_tokens, hint_count,
                reasoning_marker_count, technical_term_count, simple_indicator_count,
                code_keyword_count, math_term_count, creative_term_count, multi_step_signal,
                question_count, system_format_hint
            )
            SELECT
                request_id, upstream, model, pool, decision_kind, rule_id, hint_kind,
                hint_value, heuristic_score, heuristic_threshold, fallbacks,
                est_input_tokens, conversation_tokens, message_count, tool_count, has_images,
                requires_json_schema, code_block_count, requested_max_output_tokens, hint_count,
                reasoning_marker_count, technical_term_count, simple_indicator_count,
                code_keyword_count, math_term_count, creative_term_count, multi_step_signal,
                question_count, system_format_hint
            FROM decisions;
            DROP TABLE decisions;
            ALTER TABLE decisions_v6 RENAME TO decisions;
        ",
    },
    Migration {
        to: 7,
        // The quota-first decision snapshot: five nullable columns, so an
        // ALTER (no table rebuild) is enough. NULL on every existing row and on
        // tiered routes.
        sql: "
            ALTER TABLE decisions ADD COLUMN quota_reset_ms INTEGER;
            ALTER TABLE decisions ADD COLUMN quota_remaining_permille INTEGER;
            ALTER TABLE decisions ADD COLUMN quota_headroom_permille INTEGER;
            ALTER TABLE decisions ADD COLUMN quota_pressured INTEGER;
            ALTER TABLE decisions ADD COLUMN quota_exhausted INTEGER;
        ",
    },
    Migration {
        to: 8,
        // Content-free request diagnostics and conversion cancellation/reason
        // semantics. Existing rows retain unknown transport metadata, and
        // legacy conversion rows derive their outcome from `succeeded`.
        sql: "
            ALTER TABLE requests ADD COLUMN request_method TEXT;
            ALTER TABLE requests ADD COLUMN path_kind TEXT NOT NULL DEFAULT 'unknown'
                CHECK (path_kind IN (
                    'chat_completions', 'responses', 'messages',
                    'gemini_generate_content', 'models', 'embeddings', 'admin',
                    'unknown_agent_endpoint', 'unknown'
                ));
            ALTER TABLE conversion_reports ADD COLUMN outcome TEXT NOT NULL DEFAULT 'unknown'
                CHECK (outcome IN ('succeeded', 'failed', 'cancelled', 'unknown'));
            ALTER TABLE conversion_reports ADD COLUMN reason_code TEXT
                CHECK (reason_code IS NULL OR reason_code IN (
                    'unsupported_tool_type', 'provider_tool_unsupported',
                    'stateful_chaining', 'structured_output', 'reasoning_item',
                    'unsupported_media', 'invalid_json', 'invalid_protocol_shape',
                    'adapter_failure'
                ));
            ALTER TABLE conversion_reports ADD COLUMN reason_detail TEXT
                CHECK (reason_detail IS NULL OR reason_detail IN (
                    'local_shell', 'web_search', 'function_tool',
                    'previous_response_id', 'json_schema', 'reasoning', 'image',
                    'request_body', 'other_tool_type'
                ));
            UPDATE conversion_reports
               SET outcome = CASE WHEN succeeded THEN 'succeeded' ELSE 'failed' END
             WHERE outcome = 'unknown';
        ",
    },
    Migration {
        // v8 -> v9: the actual engine for each attempt. Existing rows predate
        // the distinction and remain explicitly unknown rather than guessed.
        to: 9,
        sql: "ALTER TABLE attempts ADD COLUMN provider_call_engine TEXT NOT NULL DEFAULT 'unknown'
                CHECK (provider_call_engine IN ('legacy', 'south_v1_buffered', 'unknown'));",
    },
    Migration {
        // v9 -> v10: widen the closed engine set. SQLite cannot alter a CHECK
        // constraint in place, so preserve every attempt through a named copy.
        to: 10,
        sql: "
            CREATE TABLE attempts_v10 (
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
                        'legacy', 'south_v1_buffered', 'south_v1_streaming', 'unknown'
                    )),
                fallback_allowed INTEGER NOT NULL,
                PRIMARY KEY (request_id, ordinal)
            );
            INSERT INTO attempts_v10 (
                request_id, ordinal, upstream, model, latency_ms, http_status,
                error_code, stream_outcome, provider_call_engine, fallback_allowed
            )
            SELECT
                request_id, ordinal, upstream, model, latency_ms, http_status,
                error_code, stream_outcome, provider_call_engine, fallback_allowed
            FROM attempts;
            DROP TABLE attempts;
            ALTER TABLE attempts_v10 RENAME TO attempts;
        ",
    },
    Migration {
        // v10 -> v11: why a South-eligible attempt ran on legacy. Existing
        // rows predate the South default and stay NULL rather than guessed.
        to: 11,
        sql: "ALTER TABLE attempts ADD COLUMN south_fallback_reason TEXT
                CHECK (south_fallback_reason IS NULL OR south_fallback_reason IN (
                    'configured_legacy', 'buffered_mode_cannot_stream', 'unauthenticated_upstream',
                    'no_provider_runtime', 'credential_resolver', 'provider_dialect',
                    'provider_package_unapproved', 'api_dialect', 'egress', 'streaming', 'method',
                    'auth', 'body', 'secret_source', 'response_metadata', 'headers'
                ));",
    },
];

/// One row per exchange, flattened from `RequestRecord`.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS requests (
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
    -- routing (NULL row-wise when the request failed before a decision)
    upstream            TEXT,
    model               TEXT,
    pool                TEXT,
    tier                TEXT,
    rule_id             TEXT,
    hint_kind           TEXT,
    hint_value          TEXT,
    heuristic_score     INTEGER,
    heuristic_threshold INTEGER,
    fallbacks           INTEGER,
    -- features (isomorphic to router-core's RequestFeatures)
    est_input_tokens    INTEGER,
    message_count       INTEGER,
    tool_count          INTEGER,
    has_images          INTEGER,
    requires_json_schema INTEGER,
    code_block_count    INTEGER,
    requested_max_output_tokens INTEGER,
    hint_count          INTEGER,
    -- usage (the protocol::Usage vocabulary; NULL means unreported, not zero)
    input_tokens        INTEGER,
    output_tokens       INTEGER,
    cache_read_tokens   INTEGER,
    cache_write_tokens  INTEGER,
    reasoning_tokens    INTEGER,
    -- micro-units; NULL when the model has no price (unknown, not zero)
    cost_micros         INTEGER,
    -- actual/estimated/unknown; unknown never carries a numeric cost
    cost_kind           TEXT NOT NULL DEFAULT 'unknown'
        CHECK (cost_kind IN ('actual', 'estimated', 'unknown')),
    -- the price table version cost_micros was computed under (NULL if unpriced)
    price_version       INTEGER,
    request_method      TEXT,
    path_kind           TEXT NOT NULL DEFAULT 'unknown'
        CHECK (path_kind IN (
            'chat_completions', 'responses', 'messages',
            'gemini_generate_content', 'models', 'embeddings', 'admin',
            'unknown_agent_endpoint', 'unknown'
        ))
);
CREATE INDEX IF NOT EXISTS requests_started_at ON requests (started_at_ms);
-- A stable accounting id is unique: writing the same request twice (a derived
-- table rebuild) is idempotent. Empty ids (legacy rows) are exempt.
CREATE UNIQUE INDEX IF NOT EXISTS requests_request_id
    ON requests (request_id) WHERE request_id <> '';

CREATE TABLE IF NOT EXISTS decisions (
    request_id TEXT PRIMARY KEY,
    upstream TEXT NOT NULL,
    model TEXT NOT NULL,
    pool TEXT NOT NULL,
    decision_kind TEXT NOT NULL
        CHECK (decision_kind IN ('rule', 'hint', 'heuristic', 'default', 'exact_model', 'quota')),
    rule_id TEXT,
    hint_kind TEXT,
    hint_value TEXT,
    heuristic_score INTEGER,
    heuristic_threshold INTEGER,
    fallbacks INTEGER NOT NULL,
    est_input_tokens INTEGER NOT NULL,
    conversation_tokens INTEGER NOT NULL DEFAULT 0,
    message_count INTEGER NOT NULL,
    tool_count INTEGER NOT NULL,
    has_images INTEGER NOT NULL,
    requires_json_schema INTEGER NOT NULL,
    code_block_count INTEGER NOT NULL,
    requested_max_output_tokens INTEGER,
    hint_count INTEGER NOT NULL,
    reasoning_marker_count INTEGER NOT NULL,
    technical_term_count INTEGER NOT NULL,
    simple_indicator_count INTEGER NOT NULL,
    code_keyword_count INTEGER NOT NULL,
    math_term_count INTEGER NOT NULL,
    creative_term_count INTEGER NOT NULL,
    multi_step_signal INTEGER NOT NULL,
    question_count INTEGER NOT NULL,
    system_format_hint INTEGER NOT NULL,
    -- quota-first decision snapshot (NULL for tiered routes)
    quota_reset_ms INTEGER,
    quota_remaining_permille INTEGER,
    quota_headroom_permille INTEGER,
    quota_pressured INTEGER,
    quota_exhausted INTEGER
);

CREATE TABLE IF NOT EXISTS attempts (
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
            'legacy', 'south_v1_buffered', 'south_v1_streaming', 'unknown'
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

CREATE TABLE IF NOT EXISTS conversion_reports (
    request_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    stage TEXT NOT NULL CHECK (stage IN (
        'inbound_normalize', 'provider_request', 'provider_response',
        'outbound_render', 'stream_translate'
    )),
    source_protocol TEXT NOT NULL,
    target_protocol TEXT NOT NULL,
    succeeded INTEGER NOT NULL,
    error_code TEXT,
    outcome TEXT NOT NULL DEFAULT 'unknown'
        CHECK (outcome IN ('succeeded', 'failed', 'cancelled', 'unknown')),
    reason_code TEXT CHECK (reason_code IS NULL OR reason_code IN (
        'unsupported_tool_type', 'provider_tool_unsupported',
        'stateful_chaining', 'structured_output', 'reasoning_item',
        'unsupported_media', 'invalid_json', 'invalid_protocol_shape',
        'adapter_failure'
    )),
    reason_detail TEXT CHECK (reason_detail IS NULL OR reason_detail IN (
        'local_shell', 'web_search', 'function_tool',
        'previous_response_id', 'json_schema', 'reasoning', 'image',
        'request_body', 'other_tool_type'
    )),
    PRIMARY KEY (request_id, ordinal)
);
";

/// The SQLite-backed [`Recorder`].
pub struct SqliteStore {
    connection: Mutex<Connection>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReceiptQuery {
    pub since_ms: Option<u64>,
    pub agent_id: Option<String>,
    pub upstream: Option<String>,
    pub model: Option<String>,
    /// `success`, `error`, or absent.
    pub status: Option<String>,
}

#[derive(Debug)]
pub struct ReceiptPage {
    pub items: Vec<ReceiptView>,
    pub total: u64,
}

fn wide(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn hint_kind_name(kind: HintKind) -> &'static str {
    match kind {
        HintKind::StepType => "step_type",
        HintKind::TaskType => "task_type",
        HintKind::Preference => "preference",
        HintKind::Capability => "capability",
    }
}

fn stream_outcome_name(outcome: StreamOutcome) -> &'static str {
    match outcome {
        StreamOutcome::Complete => "complete",
        StreamOutcome::FailedAfterPartial => "failed_after_partial",
        StreamOutcome::FailedBeforeOutput => "failed_before_output",
        StreamOutcome::ClientCancelled => "client_cancelled",
    }
}

/// Storage columns for the closed [`RecordedDecidedBy`] vocabulary. Values are either
/// configured identifiers or bounded numeric facts; no generic detail string
/// is introduced.
struct DecisionColumns {
    kind: &'static str,
    rule_id: Option<String>,
    hint_kind: Option<String>,
    hint_value: Option<String>,
    score: Option<u32>,
    threshold: Option<u32>,
}

fn decision_columns(decided_by: &RecordedDecidedBy) -> DecisionColumns {
    match decided_by {
        RecordedDecidedBy::Rule { rule } => DecisionColumns {
            kind: "rule",
            rule_id: Some(rule.clone()),
            hint_kind: None,
            hint_value: None,
            score: None,
            threshold: None,
        },
        RecordedDecidedBy::Hint { kind, value } => DecisionColumns {
            kind: "hint",
            rule_id: None,
            hint_kind: Some(hint_kind_name(*kind).to_owned()),
            hint_value: Some(value.clone()),
            score: None,
            threshold: None,
        },
        RecordedDecidedBy::Heuristic {
            score,
            matched_band_at_least,
        } => DecisionColumns {
            kind: "heuristic",
            rule_id: None,
            hint_kind: None,
            hint_value: None,
            score: Some(*score),
            threshold: Some(*matched_band_at_least),
        },
        RecordedDecidedBy::Default => DecisionColumns {
            kind: "default",
            rule_id: None,
            hint_kind: None,
            hint_value: None,
            score: None,
            threshold: None,
        },
        RecordedDecidedBy::ExactModel { model } => DecisionColumns {
            kind: "exact_model",
            rule_id: Some(model.clone()),
            hint_kind: None,
            hint_value: None,
            score: None,
            threshold: None,
        },
        // Quota-first mode. The chosen account lives in the decision's own
        // columns; there is no tier-like sub-field to record. The `decisions`
        // table's `decision_kind` CHECK is widened to admit 'quota' in the
        // schema migration that ships with quota-first host wiring.
        RecordedDecidedBy::Quota => DecisionColumns {
            kind: "quota",
            rule_id: None,
            hint_kind: None,
            hint_value: None,
            score: None,
            threshold: None,
        },
    }
}

fn normalized_cost(record: &RequestRecord) -> (CostKind, Option<i64>, Option<u32>) {
    match (record.cost_kind, record.cost_micros) {
        (CostKind::Actual, Some(cost)) if cost >= 0 => {
            (CostKind::Actual, Some(cost), record.price_version)
        }
        (CostKind::Estimated, Some(cost)) if cost >= 0 && record.price_version.is_some() => {
            (CostKind::Estimated, Some(cost), record.price_version)
        }
        _ => (CostKind::Unknown, None, None),
    }
}

impl SqliteStore {
    /// Reads one filtered page from the complete metadata-only Receipt ledger.
    ///
    /// # Errors
    ///
    /// Returns an operator-facing message when the query cannot be validated,
    /// opened, or decoded.
    pub fn receipt_page(
        path: &Path,
        query: &ReceiptQuery,
        limit: usize,
        offset: usize,
    ) -> Result<ReceiptPage, String> {
        if !matches!(query.status.as_deref(), None | Some("success" | "error")) {
            return Err("request receipt status must be `success` or `error`".to_owned());
        }
        if !path.exists() {
            return Ok(ReceiptPage {
                items: Vec::new(),
                total: 0,
            });
        }
        let store = Self::open(path)?;
        let connection = store
            .connection
            .lock()
            .map_err(|_| "request receipt store lock is poisoned".to_owned())?;
        let since_ms = query
            .since_ms
            .map(|value| i64::try_from(value).unwrap_or(i64::MAX));
        let where_sql = "
             WHERE (?1 IS NULL OR started_at_ms >= ?1)
               AND (?2 IS NULL OR agent_id = ?2)
               AND (?3 IS NULL OR upstream = ?3)
               AND (?4 IS NULL OR model = ?4)
               AND (
                    ?5 IS NULL
                    OR (?5 = 'success' AND status >= 200 AND status < 400 AND error_code IS NULL)
                    OR (?5 = 'error' AND status <> 499
                        AND (status < 200 OR status >= 400 OR error_code IS NOT NULL))
               )";
        let total = connection
            .query_row(
                &format!("SELECT COUNT(*) FROM requests {where_sql}"),
                rusqlite::params![
                    since_ms,
                    query.agent_id.as_deref(),
                    query.upstream.as_deref(),
                    query.model.as_deref(),
                    query.status.as_deref(),
                ],
                |row| row.get::<_, i64>(0),
            )
            .map(narrow)
            .map_err(|error| format!("request receipt count: {error}"))?;
        let bounded_limit = limit.clamp(1, 50);
        let mut statement = connection
            .prepare(&format!(
                "SELECT id, request_id, started_at_ms, latency_ms, protocol, requested_model,
                        stream, status, error_code, agent_id, running_revision,
                        upstream, model, pool, tier, rule_id, hint_kind, hint_value,
                        heuristic_score, heuristic_threshold, fallbacks,
                        est_input_tokens, message_count, tool_count, has_images,
                        requires_json_schema, code_block_count, requested_max_output_tokens,
                        hint_count, input_tokens, output_tokens, cache_read_tokens,
                        cache_write_tokens, reasoning_tokens, cost_kind, cost_micros, price_version,
                        attempts, request_method, path_kind
                   FROM requests
                  {where_sql}
                  ORDER BY started_at_ms DESC, request_id DESC, id DESC
                  LIMIT ?6 OFFSET ?7"
            ))
            .map_err(|error| format!("request receipt page query: {error}"))?;
        let mut seeds = statement
            .query_map(
                rusqlite::params![
                    since_ms,
                    query.agent_id.as_deref(),
                    query.upstream.as_deref(),
                    query.model.as_deref(),
                    query.status.as_deref(),
                    i64::try_from(bounded_limit).unwrap_or(50),
                    i64::try_from(offset).unwrap_or(i64::MAX),
                ],
                receipt_seed,
            )
            .and_then(Iterator::collect::<Result<Vec<_>, _>>)
            .map_err(|error| format!("request receipt page decode: {error}"))?;
        drop(statement);
        for seed in &mut seeds {
            if seed.persisted_request_id.is_empty() {
                continue;
            }
            seed.view.decision = read_decision(&connection, &seed.persisted_request_id)
                .map_err(|error| format!("request decision decode: {error}"))?;
            seed.view.attempt_records = read_attempts(&connection, &seed.persisted_request_id)
                .map_err(|error| format!("request attempts decode: {error}"))?;
            seed.view.conversion_reports =
                read_conversions(&connection, &seed.persisted_request_id)
                    .map_err(|error| format!("request conversions decode: {error}"))?;
        }
        Ok(ReceiptPage {
            items: seeds.into_iter().map(|seed| seed.view).collect(),
            total,
        })
    }

    /// Applies the current price table only to historical rows that never had
    /// a cost. Existing actual or estimated values remain immutable.
    ///
    /// # Errors
    ///
    /// Returns an operator-facing message when the ledger cannot be opened,
    /// queried, or updated.
    pub fn backfill_unknown_costs(
        path: &Path,
        pricing: &crate::pricing::PriceTable,
    ) -> Result<usize, String> {
        const BATCH_SIZE: usize = 500;
        if !path.exists() || pricing.models.is_empty() {
            return Ok(0);
        }
        let store = Self::open(path)?;
        let mut connection = store
            .connection
            .lock()
            .map_err(|_| "cost backfill store lock is poisoned".to_owned())?;
        let mut updated = 0usize;
        let mut last_id = 0i64;
        loop {
            let transaction = connection
                .transaction()
                .map_err(|error| format!("cost backfill transaction: {error}"))?;
            let candidates = {
                let mut statement = transaction
                    .prepare(
                        "SELECT id, upstream, model,
                                COALESCE(input_tokens, 0), COALESCE(output_tokens, 0),
                                COALESCE(cache_read_tokens, 0), COALESCE(cache_write_tokens, 0),
                                COALESCE(reasoning_tokens, 0)
                           FROM requests
                          WHERE id > ?1
                            AND cost_kind = 'unknown'
                            AND model IS NOT NULL
                            AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL)
                          ORDER BY id
                          LIMIT ?2",
                    )
                    .map_err(|error| format!("cost backfill query: {error}"))?;
                statement
                    .query_map(
                        rusqlite::params![last_id, i64::try_from(BATCH_SIZE).unwrap_or(i64::MAX)],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, Option<String>>(1)?,
                                row.get::<_, String>(2)?,
                                Usage {
                                    input_tokens: narrow(row.get::<_, i64>(3)?),
                                    output_tokens: narrow(row.get::<_, i64>(4)?),
                                    cache_read_tokens: narrow(row.get::<_, i64>(5)?),
                                    cache_write_tokens: narrow(row.get::<_, i64>(6)?),
                                    reasoning_tokens: narrow(row.get::<_, i64>(7)?),
                                    ..Usage::default()
                                },
                            ))
                        },
                    )
                    .and_then(Iterator::collect::<Result<Vec<_>, _>>)
                    .map_err(|error| format!("cost backfill decode: {error}"))?
            };
            let batch_len = candidates.len();
            for (id, upstream, model, usage) in candidates {
                last_id = id;
                let priced = upstream.as_deref().map_or_else(
                    || pricing.price(&model, &usage),
                    |upstream| pricing.price_for_upstream(upstream, &model, &usage),
                );
                let Some((cost, version)) = priced else {
                    continue;
                };
                updated += transaction
                    .execute(
                        "UPDATE requests
                            SET cost_kind = 'estimated', cost_micros = ?1, price_version = ?2
                          WHERE id = ?3 AND cost_kind = 'unknown'",
                        rusqlite::params![cost, version, id],
                    )
                    .map_err(|error| format!("cost backfill update: {error}"))?;
            }
            transaction
                .commit()
                .map_err(|error| format!("cost backfill commit: {error}"))?;
            if batch_len < BATCH_SIZE {
                break;
            }
        }
        Ok(updated)
    }

    /// Opens (or creates) the database and brings the schema up.
    ///
    /// # Errors
    ///
    /// A message for the operator: the store failing to open is a startup
    /// error, unlike a single record failing to write.
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut connection = Connection::open(path)
            .map_err(|error| format!("metrics store `{}`: {error}", path.display()))?;

        let version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|error| format!("metrics store version: {error}"))?;
        match version {
            0 => {
                connection
                    .execute_batch(SCHEMA)
                    .map_err(|error| format!("metrics schema: {error}"))?;
                connection
                    .pragma_update(None, "user_version", SCHEMA_VERSION)
                    .map_err(|error| format!("metrics schema version: {error}"))?;
            }
            SCHEMA_VERSION => {}
            older if older < SCHEMA_VERSION => {
                // An older store: migrate it forward instead of bricking it.
                Self::migrate(path, &mut connection, older)?;
            }
            newer => {
                // A newer client wrote here. Refusing beats silently writing
                // rows a future schema no longer means the same thing by.
                return Err(format!(
                    "metrics store `{}` has schema version {newer}, this build knows {SCHEMA_VERSION}",
                    path.display()
                ));
            }
        }

        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Brings an older store up to [`SCHEMA_VERSION`], applying each pending
    /// migration in order and stamping `user_version` as each lands. A copy of
    /// the database is taken first, so a failed or unwanted upgrade can be
    /// rolled back to exactly the bytes that were there before.
    fn migrate(path: &Path, connection: &mut Connection, from: u32) -> Result<(), String> {
        let backup = path.with_extension(format!("v{from}.bak"));
        std::fs::copy(path, &backup).map_err(|error| {
            format!(
                "metrics backup `{}` before migrating: {error}",
                backup.display()
            )
        })?;

        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.to > from && migration.to <= SCHEMA_VERSION)
        {
            let transaction = connection.transaction().map_err(|error| {
                format!("metrics migration transaction v{}: {error}", migration.to)
            })?;
            transaction
                .execute_batch(migration.sql)
                .map_err(|error| format!("metrics migration to v{}: {error}", migration.to))?;
            transaction
                .pragma_update(None, "user_version", migration.to)
                .map_err(|error| format!("metrics migration version v{}: {error}", migration.to))?;
            transaction
                .commit()
                .map_err(|error| format!("metrics migration commit v{}: {error}", migration.to))?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // one atomic parent + three child-table write
    fn insert(&self, record: &RequestRecord) -> Result<(), rusqlite::Error> {
        let routing = record.routing.as_ref();
        let features = routing.map(|routing| routing.features);
        let route_decision = routing.map_or_else(
            || DecisionColumns {
                kind: "",
                rule_id: None,
                hint_kind: None,
                hint_value: None,
                score: None,
                threshold: None,
            },
            |routing| decision_columns(&routing.decided_by),
        );
        let (cost_kind, cost_micros, price_version) = normalized_cost(record);

        let mut connection = self.connection.lock().expect("store lock");
        let transaction = connection.transaction()?;
        let inserted = transaction.execute(
            // OR IGNORE makes a re-write of the same accounting id a no-op: the
            // unique index dedups it rather than double-counting. Child rows are
            // inserted only when this statement actually inserted the parent.
            "INSERT OR IGNORE INTO requests (
                request_id, agent_id, running_revision, request_method, path_kind,
                started_at_ms, latency_ms, protocol, requested_model, stream, status,
                error_code, attempts,
                upstream, model, pool, tier, rule_id, hint_kind, hint_value,
                heuristic_score, heuristic_threshold, fallbacks,
                est_input_tokens, message_count, tool_count, has_images,
                requires_json_schema, code_block_count, requested_max_output_tokens, hint_count,
                input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                reasoning_tokens, cost_micros, cost_kind, price_version
            ) VALUES (
                :request_id, :agent_id, :running_revision, :request_method, :path_kind,
                :started_at_ms, :latency_ms, :protocol, :requested_model, :stream, :status,
                :error_code, :attempts,
                :upstream, :model, :pool, :tier, :rule_id, :hint_kind, :hint_value,
                :heuristic_score, :heuristic_threshold, :fallbacks,
                :est_input_tokens, :message_count, :tool_count, :has_images,
                :requires_json_schema, :code_block_count, :requested_max_output_tokens, :hint_count,
                :input_tokens, :output_tokens, :cache_read_tokens, :cache_write_tokens,
                :reasoning_tokens, :cost_micros, :cost_kind, :price_version
            )",
            named_params! {
                ":request_id": record.request_id,
                ":agent_id": record.agent_id,
                ":running_revision": record.running_revision.map(wide),
                ":request_method": record.request_method,
                ":path_kind": record.path_kind.as_str(),
                ":started_at_ms": wide(record.started_at_ms),
                ":latency_ms": wide(record.latency_ms),
                ":protocol": record.protocol,
                ":requested_model": record.requested_model,
                ":stream": record.stream,
                ":status": record.status,
                ":error_code": record.error_code.map(ErrorCode::as_str),
                ":attempts": record.attempts,
                ":upstream": routing.map(|value| value.upstream.as_str()),
                ":model": routing.map(|value| value.model.as_str()),
                ":pool": routing.map(|value| value.pool.as_str()),
                ":tier": routing.map(|_| route_decision.kind),
                ":rule_id": route_decision.rule_id,
                ":hint_kind": route_decision.hint_kind,
                ":hint_value": route_decision.hint_value,
                ":heuristic_score": route_decision.score,
                ":heuristic_threshold": route_decision.threshold,
                ":fallbacks": routing.map(|value| value.fallbacks),
                ":est_input_tokens": features.map(|value| value.estimated_input_tokens),
                ":message_count": features.map(|value| value.message_count),
                ":tool_count": features.map(|value| value.tool_count),
                ":has_images": features.map(|value| value.has_images),
                ":requires_json_schema": features.map(|value| value.requires_json_schema),
                ":code_block_count": features.map(|value| value.code_block_count),
                ":requested_max_output_tokens": features.and_then(|value| value.requested_max_output_tokens),
                ":hint_count": features.map(|value| value.hint_count),
                ":input_tokens": record.usage.map(|value| wide(value.input_tokens)),
                ":output_tokens": record.usage.map(|value| wide(value.output_tokens)),
                ":cache_read_tokens": record.usage.map(|value| wide(value.cache_read_tokens)),
                ":cache_write_tokens": record.usage.map(|value| wide(value.cache_write_tokens)),
                ":reasoning_tokens": record.usage.map(|value| wide(value.reasoning_tokens)),
                ":cost_micros": cost_micros,
                ":cost_kind": cost_kind.as_str(),
                ":price_version": price_version,
            },
        )?;

        if inserted == 0 {
            transaction.commit()?;
            return Ok(());
        }

        if let Some(decision) = &record.decision {
            insert_decision(&transaction, &record.request_id, decision)?;
        }
        for attempt in &record.attempt_records {
            transaction.execute(
                "INSERT INTO attempts (
                    request_id, ordinal, upstream, model, latency_ms, http_status,
                    error_code, stream_outcome, provider_call_engine, fallback_allowed,
                    south_fallback_reason
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    record.request_id,
                    attempt.ordinal,
                    attempt.upstream,
                    attempt.model,
                    wide(attempt.latency_ms),
                    attempt.http_status,
                    attempt.error_code.map(ErrorCode::as_str),
                    attempt.stream_outcome.map(stream_outcome_name),
                    attempt.provider_call_engine.as_str(),
                    attempt.fallback_allowed,
                    attempt
                        .south_fallback_reason
                        .map(token_station_metrics::SouthFallbackReason::as_str),
                ],
            )?;
        }
        for conversion in &record.conversion_reports {
            transaction.execute(
                "INSERT INTO conversion_reports (
                    request_id, ordinal, stage, source_protocol, target_protocol,
                    succeeded, error_code, outcome, reason_code, reason_detail
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    record.request_id,
                    conversion.ordinal,
                    conversion.stage.as_str(),
                    conversion.source_protocol,
                    conversion.target_protocol,
                    conversion.succeeded,
                    conversion.error_code.map(ErrorCode::as_str),
                    conversion.outcome.as_str(),
                    conversion.reason_code.map(ConversionReasonCode::as_str),
                    conversion.reason_detail.map(ConversionReasonDetail::as_str),
                ],
            )?;
        }

        transaction.commit()?;
        Ok(())
    }
}

fn insert_decision(
    transaction: &Transaction<'_>,
    request_id: &str,
    decision: &DecisionRecord,
) -> Result<(), rusqlite::Error> {
    let columns = decision_columns(&decision.decided_by);
    let features = decision.features;
    let quota = decision.quota.as_ref();
    transaction.execute(
        "INSERT INTO decisions (
            request_id, upstream, model, pool, decision_kind, rule_id, hint_kind,
            hint_value, heuristic_score, heuristic_threshold, fallbacks,
            est_input_tokens, conversation_tokens, message_count, tool_count, has_images,
            requires_json_schema, code_block_count, requested_max_output_tokens, hint_count,
            reasoning_marker_count, technical_term_count, simple_indicator_count,
            code_keyword_count, math_term_count, creative_term_count, multi_step_signal,
            question_count, system_format_hint,
            quota_reset_ms, quota_remaining_permille, quota_headroom_permille,
            quota_pressured, quota_exhausted
         ) VALUES (
            :request_id, :upstream, :model, :pool, :decision_kind, :rule_id, :hint_kind,
            :hint_value, :heuristic_score, :heuristic_threshold, :fallbacks,
            :est_input_tokens, :conversation_tokens, :message_count, :tool_count, :has_images,
            :requires_json_schema, :code_block_count, :requested_max_output_tokens, :hint_count,
            :reasoning_marker_count, :technical_term_count, :simple_indicator_count,
            :code_keyword_count, :math_term_count, :creative_term_count, :multi_step_signal,
            :question_count, :system_format_hint,
            :quota_reset_ms, :quota_remaining_permille, :quota_headroom_permille,
            :quota_pressured, :quota_exhausted
         )",
        named_params! {
            ":request_id": request_id,
            ":upstream": decision.upstream,
            ":model": decision.model,
            ":pool": decision.pool,
            ":decision_kind": columns.kind,
            ":rule_id": columns.rule_id,
            ":hint_kind": columns.hint_kind,
            ":hint_value": columns.hint_value,
            ":heuristic_score": columns.score,
            ":heuristic_threshold": columns.threshold,
            ":fallbacks": decision.fallbacks,
            ":est_input_tokens": features.estimated_input_tokens,
            ":conversation_tokens": features.conversation_tokens,
            ":message_count": features.message_count,
            ":tool_count": features.tool_count,
            ":has_images": features.has_images,
            ":requires_json_schema": features.requires_json_schema,
            ":code_block_count": features.code_block_count,
            ":requested_max_output_tokens": features.requested_max_output_tokens,
            ":hint_count": features.hint_count,
            ":reasoning_marker_count": features.reasoning_marker_count,
            ":technical_term_count": features.technical_term_count,
            ":simple_indicator_count": features.simple_indicator_count,
            ":code_keyword_count": features.code_keyword_count,
            ":math_term_count": features.math_term_count,
            ":creative_term_count": features.creative_term_count,
            ":multi_step_signal": features.multi_step_signal,
            ":question_count": features.question_count,
            ":system_format_hint": features.system_format_hint,
            ":quota_reset_ms": quota
                .and_then(|q| q.reset_ms)
                .map(|ms| i64::try_from(ms).unwrap_or(i64::MAX)),
            ":quota_remaining_permille": quota.and_then(|q| q.remaining_permille),
            ":quota_headroom_permille": quota.map(|q| q.headroom_permille),
            ":quota_pressured": quota.map(|q| q.pressured),
            ":quota_exhausted": quota.map(|q| q.exhausted),
        },
    )?;
    Ok(())
}

fn invalid_enum(column: usize, kind: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unknown {kind} `{value}`"),
        )),
    )
}

fn error_code(column: usize, value: &str) -> Result<ErrorCode, rusqlite::Error> {
    match value {
        "invalid_request" => Ok(ErrorCode::InvalidRequest),
        "auth" => Ok(ErrorCode::Auth),
        "payment_required" => Ok(ErrorCode::PaymentRequired),
        "rate_limit" => Ok(ErrorCode::RateLimit),
        "capacity" => Ok(ErrorCode::Capacity),
        "capability" => Ok(ErrorCode::Capability),
        "content_policy" => Ok(ErrorCode::ContentPolicy),
        "upstream_unavailable" => Ok(ErrorCode::UpstreamUnavailable),
        "transport_truncated" => Ok(ErrorCode::TransportTruncated),
        "context_length" => Ok(ErrorCode::ContextLength),
        "provider_protocol_error" => Ok(ErrorCode::ProviderProtocolError),
        "timeout" => Ok(ErrorCode::Timeout),
        "internal" => Ok(ErrorCode::Internal),
        other => Err(invalid_enum(column, "error code", other)),
    }
}

fn optional_error_code(
    column: usize,
    value: Option<&str>,
) -> Result<Option<ErrorCode>, rusqlite::Error> {
    value.map(|value| error_code(column, value)).transpose()
}

fn hint_kind(column: usize, value: &str) -> Result<HintKind, rusqlite::Error> {
    match value {
        "step_type" => Ok(HintKind::StepType),
        "task_type" => Ok(HintKind::TaskType),
        "preference" => Ok(HintKind::Preference),
        "capability" => Ok(HintKind::Capability),
        other => Err(invalid_enum(column, "hint kind", other)),
    }
}

fn decided_by(
    column: usize,
    kind: &str,
    rule_id: Option<String>,
    hint_kind_value: Option<&str>,
    hint_value: Option<String>,
    score: Option<u32>,
    threshold: Option<u32>,
) -> Result<RecordedDecidedBy, rusqlite::Error> {
    match kind {
        "rule" => Ok(RecordedDecidedBy::Rule {
            rule: rule_id.unwrap_or_default(),
        }),
        "hint" => Ok(RecordedDecidedBy::Hint {
            kind: hint_kind(column, hint_kind_value.unwrap_or(""))?,
            value: hint_value.unwrap_or_default(),
        }),
        "heuristic" => Ok(RecordedDecidedBy::Heuristic {
            score: score.unwrap_or(0),
            matched_band_at_least: threshold.unwrap_or(0),
        }),
        "default" | "" => Ok(RecordedDecidedBy::Default),
        "exact_model" => Ok(RecordedDecidedBy::ExactModel {
            model: rule_id.unwrap_or_default(),
        }),
        "quota" => Ok(RecordedDecidedBy::Quota),
        other => Err(invalid_enum(column, "decision kind", other)),
    }
}

fn cost_kind(column: usize, value: &str) -> Result<CostKind, rusqlite::Error> {
    match value {
        "actual" => Ok(CostKind::Actual),
        "estimated" => Ok(CostKind::Estimated),
        "unknown" => Ok(CostKind::Unknown),
        other => Err(invalid_enum(column, "cost kind", other)),
    }
}

fn stream_outcome(column: usize, value: &str) -> Result<StreamOutcome, rusqlite::Error> {
    match value {
        "complete" => Ok(StreamOutcome::Complete),
        "failed_after_partial" => Ok(StreamOutcome::FailedAfterPartial),
        "failed_before_output" => Ok(StreamOutcome::FailedBeforeOutput),
        "client_cancelled" => Ok(StreamOutcome::ClientCancelled),
        other => Err(invalid_enum(column, "stream outcome", other)),
    }
}

fn provider_call_engine(column: usize, value: &str) -> Result<ProviderCallEngine, rusqlite::Error> {
    match value {
        "legacy" => Ok(ProviderCallEngine::Legacy),
        "south_v1_buffered" => Ok(ProviderCallEngine::SouthV1Buffered),
        "south_v1_streaming" => Ok(ProviderCallEngine::SouthV1Streaming),
        "unknown" => Ok(ProviderCallEngine::Unknown),
        other => Err(invalid_enum(column, "provider call engine", other)),
    }
}

fn south_fallback_reason(
    column: usize,
    value: &str,
) -> Result<token_station_metrics::SouthFallbackReason, rusqlite::Error> {
    token_station_metrics::SouthFallbackReason::parse(value)
        .ok_or_else(|| invalid_enum(column, "south fallback reason", value))
}

fn conversion_stage(column: usize, value: &str) -> Result<ConversionStage, rusqlite::Error> {
    match value {
        "inbound_normalize" => Ok(ConversionStage::InboundNormalize),
        "provider_request" => Ok(ConversionStage::ProviderRequest),
        "provider_response" => Ok(ConversionStage::ProviderResponse),
        "outbound_render" => Ok(ConversionStage::OutboundRender),
        "stream_translate" => Ok(ConversionStage::StreamTranslate),
        other => Err(invalid_enum(column, "conversion stage", other)),
    }
}

fn conversion_outcome(column: usize, value: &str) -> Result<ConversionOutcome, rusqlite::Error> {
    match value {
        "succeeded" => Ok(ConversionOutcome::Succeeded),
        "failed" => Ok(ConversionOutcome::Failed),
        "cancelled" => Ok(ConversionOutcome::Cancelled),
        "unknown" => Ok(ConversionOutcome::Unknown),
        other => Err(invalid_enum(column, "conversion outcome", other)),
    }
}

fn conversion_reason_code(
    column: usize,
    value: &str,
) -> Result<ConversionReasonCode, rusqlite::Error> {
    match value {
        "unsupported_tool_type" => Ok(ConversionReasonCode::UnsupportedToolType),
        "provider_tool_unsupported" => Ok(ConversionReasonCode::ProviderToolUnsupported),
        "stateful_chaining" => Ok(ConversionReasonCode::StatefulChaining),
        "structured_output" => Ok(ConversionReasonCode::StructuredOutput),
        "reasoning_item" => Ok(ConversionReasonCode::ReasoningItem),
        "unsupported_media" => Ok(ConversionReasonCode::UnsupportedMedia),
        "invalid_json" => Ok(ConversionReasonCode::InvalidJson),
        "invalid_protocol_shape" => Ok(ConversionReasonCode::InvalidProtocolShape),
        "adapter_failure" => Ok(ConversionReasonCode::AdapterFailure),
        other => Err(invalid_enum(column, "conversion reason code", other)),
    }
}

fn conversion_reason_detail(
    column: usize,
    value: &str,
) -> Result<ConversionReasonDetail, rusqlite::Error> {
    match value {
        "local_shell" => Ok(ConversionReasonDetail::LocalShell),
        "web_search" => Ok(ConversionReasonDetail::WebSearch),
        "function_tool" => Ok(ConversionReasonDetail::FunctionTool),
        "previous_response_id" => Ok(ConversionReasonDetail::PreviousResponseId),
        "json_schema" => Ok(ConversionReasonDetail::JsonSchema),
        "reasoning" => Ok(ConversionReasonDetail::Reasoning),
        "image" => Ok(ConversionReasonDetail::Image),
        "request_body" => Ok(ConversionReasonDetail::RequestBody),
        "other_tool_type" => Ok(ConversionReasonDetail::OtherToolType),
        other => Err(invalid_enum(column, "conversion reason detail", other)),
    }
}

fn request_path_kind(column: usize, value: &str) -> Result<RequestPathKind, rusqlite::Error> {
    match value {
        "chat_completions" => Ok(RequestPathKind::ChatCompletions),
        "responses" => Ok(RequestPathKind::Responses),
        "messages" => Ok(RequestPathKind::Messages),
        "gemini_generate_content" => Ok(RequestPathKind::GeminiGenerateContent),
        "models" => Ok(RequestPathKind::Models),
        "embeddings" => Ok(RequestPathKind::Embeddings),
        "admin" => Ok(RequestPathKind::Admin),
        "unknown_agent_endpoint" => Ok(RequestPathKind::UnknownAgentEndpoint),
        "unknown" => Ok(RequestPathKind::Unknown),
        other => Err(invalid_enum(column, "request path kind", other)),
    }
}

fn narrow(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

struct ReceiptSeed {
    persisted_request_id: String,
    view: ReceiptView,
}

/// Reads at most five recent request receipts. A missing store is the normal
/// empty state; an older store is backed up and migrated before it is read so
/// historical flat rows remain visible with empty child timelines.
///
/// # Errors
///
/// Returns an operator-facing message when the database cannot be opened,
/// migrated, queried, or decoded under the current closed enum vocabulary.
pub fn recent_receipts(path: &Path, limit: usize) -> Result<Vec<ReceiptView>, String> {
    SqliteStore::recent_receipts(path, limit)
}

impl SqliteStore {
    /// Associated form used by the admin surfaces; kept alongside the free
    /// function so callers can choose the module-style API without divergence.
    ///
    /// # Errors
    ///
    /// Returns an operator-facing message when the database cannot be opened,
    /// migrated, queried, or decoded under the current closed enum vocabulary.
    pub fn recent_receipts(path: &Path, limit: usize) -> Result<Vec<ReceiptView>, String> {
        let limit = limit.min(5);
        if limit == 0 || !path.exists() {
            return Ok(Vec::new());
        }
        let store = SqliteStore::open(path)?;
        store.read_recent(limit)
    }

    fn read_recent(&self, limit: usize) -> Result<Vec<ReceiptView>, String> {
        let connection = self.connection.lock().expect("store lock");
        let mut statement = connection
            .prepare(
                "SELECT id, request_id, started_at_ms, latency_ms, protocol, requested_model,
                        stream, status, error_code, agent_id, running_revision,
                        upstream, model, pool, tier, rule_id, hint_kind, hint_value,
                        heuristic_score, heuristic_threshold, fallbacks,
                        est_input_tokens, message_count, tool_count, has_images,
                        requires_json_schema, code_block_count, requested_max_output_tokens,
                        hint_count, input_tokens, output_tokens, cache_read_tokens,
                        cache_write_tokens, reasoning_tokens, cost_kind, cost_micros, price_version,
                        attempts, request_method, path_kind
                   FROM requests
                  ORDER BY started_at_ms DESC, request_id DESC, id DESC
                  LIMIT ?1",
            )
            .map_err(|error| format!("request receipts query: {error}"))?;
        let mut seeds = statement
            .query_map([i64::try_from(limit).unwrap_or(5)], receipt_seed)
            .and_then(Iterator::collect::<Result<Vec<_>, _>>)
            .map_err(|error| format!("request receipts decode: {error}"))?;
        drop(statement);

        for seed in &mut seeds {
            if seed.persisted_request_id.is_empty() {
                continue;
            }
            seed.view.decision = read_decision(&connection, &seed.persisted_request_id)
                .map_err(|error| format!("request decision decode: {error}"))?;
            seed.view.attempt_records = read_attempts(&connection, &seed.persisted_request_id)
                .map_err(|error| format!("request attempts decode: {error}"))?;
            seed.view.conversion_reports =
                read_conversions(&connection, &seed.persisted_request_id)
                    .map_err(|error| format!("request conversions decode: {error}"))?;
        }
        Ok(seeds.into_iter().map(|seed| seed.view).collect())
    }
}

fn receipt_seed(row: &Row<'_>) -> Result<ReceiptSeed, rusqlite::Error> {
    let database_id = row.get::<_, i64>(0)?;
    let persisted_request_id = row.get::<_, String>(1)?;
    let raw_error_code = row.get::<_, Option<String>>(8)?;
    let error_code = optional_error_code(8, raw_error_code.as_deref())?;
    let route_upstream = row.get::<_, Option<String>>(11)?;
    let route_model = row.get::<_, Option<String>>(12)?;
    let route_pool = row.get::<_, Option<String>>(13)?;
    let route_kind = row.get::<_, Option<String>>(14)?;
    let route_rule_id = row.get::<_, Option<String>>(15)?;
    let route_hint_kind = row.get::<_, Option<String>>(16)?;
    let route_hint_value = row.get::<_, Option<String>>(17)?;
    let routing = match (route_upstream, route_model, route_pool) {
        (Some(upstream), Some(model), Some(pool)) => Some(RoutingRecord {
            upstream,
            model,
            pool,
            decided_by: decided_by(
                14,
                route_kind.as_deref().unwrap_or(""),
                route_rule_id,
                route_hint_kind.as_deref(),
                route_hint_value,
                row.get(18)?,
                row.get(19)?,
            )?,
            fallbacks: row.get::<_, Option<u32>>(20)?.unwrap_or(0),
            features: RequestFeatures {
                estimated_input_tokens: row.get::<_, Option<u32>>(21)?.unwrap_or(0),
                message_count: row.get::<_, Option<u32>>(22)?.unwrap_or(0),
                tool_count: row.get::<_, Option<u32>>(23)?.unwrap_or(0),
                has_images: row.get::<_, Option<bool>>(24)?.unwrap_or(false),
                requires_json_schema: row.get::<_, Option<bool>>(25)?.unwrap_or(false),
                code_block_count: row.get::<_, Option<u32>>(26)?.unwrap_or(0),
                requested_max_output_tokens: row.get(27)?,
                hint_count: row.get::<_, Option<u32>>(28)?.unwrap_or(0),
                ..RequestFeatures::default()
            },
        }),
        _ => None,
    };
    let input_tokens = row.get::<_, Option<i64>>(29)?.map(narrow);
    let output_tokens = row.get::<_, Option<i64>>(30)?.map(narrow);
    let cache_read_tokens = row.get::<_, Option<i64>>(31)?.map(narrow);
    let cache_write_tokens = row.get::<_, Option<i64>>(32)?.map(narrow);
    let reasoning_tokens = row.get::<_, Option<i64>>(33)?.map(narrow);
    let usage = (input_tokens.is_some()
        || output_tokens.is_some()
        || cache_read_tokens.is_some()
        || cache_write_tokens.is_some()
        || reasoning_tokens.is_some())
    .then_some(Usage {
        input_tokens: input_tokens.unwrap_or(0),
        output_tokens: output_tokens.unwrap_or(0),
        cache_read_tokens: cache_read_tokens.unwrap_or(0),
        cache_write_tokens: cache_write_tokens.unwrap_or(0),
        reasoning_tokens: reasoning_tokens.unwrap_or(0),
        ..Usage::default()
    });
    let cost_kind = cost_kind(34, &row.get::<_, String>(34)?)?;
    let (cost_micros, price_version) = match cost_kind {
        CostKind::Unknown => (None, None),
        CostKind::Actual | CostKind::Estimated => (row.get(35)?, row.get(36)?),
    };
    let request_id = if persisted_request_id.is_empty() {
        format!("legacy-{database_id}")
    } else {
        persisted_request_id.clone()
    };

    Ok(ReceiptSeed {
        persisted_request_id,
        view: ReceiptView {
            request_id,
            started_at_ms: narrow(row.get(2)?),
            latency_ms: narrow(row.get(3)?),
            protocol: row.get(4)?,
            agent_id: row.get(9)?,
            running_revision: row.get::<_, Option<i64>>(10)?.map(narrow),
            request_method: row.get(38)?,
            path_kind: request_path_kind(39, &row.get::<_, String>(39)?)?,
            requested_model: row.get(5)?,
            stream: row.get(6)?,
            status: row.get(7)?,
            error_code,
            attempts: row.get(37)?,
            routing,
            usage,
            cost_kind,
            cost_micros,
            price_version,
            decision: None,
            attempt_records: Vec::new(),
            conversion_reports: Vec::new(),
        },
    })
}

fn read_decision(
    connection: &Connection,
    request_id: &str,
) -> Result<Option<DecisionRecord>, rusqlite::Error> {
    connection
        .query_row(
            "SELECT upstream, model, pool, decision_kind, rule_id, hint_kind, hint_value,
                    heuristic_score, heuristic_threshold, fallbacks,
                    est_input_tokens, conversation_tokens, message_count, tool_count, has_images,
                    requires_json_schema, code_block_count, requested_max_output_tokens, hint_count,
                    reasoning_marker_count, technical_term_count, simple_indicator_count,
                    code_keyword_count, math_term_count, creative_term_count, multi_step_signal,
                    question_count, system_format_hint,
                    quota_reset_ms, quota_remaining_permille, quota_headroom_permille,
                    quota_pressured, quota_exhausted
               FROM decisions WHERE request_id = ?1",
            [request_id],
            |row| {
                let kind = row.get::<_, String>(3)?;
                let hint_kind_value = row.get::<_, Option<String>>(5)?;
                // Quota snapshot (all NULL for tiered routes). Headroom is always
                // present on a quota route, so it gates whether we build one.
                let quota_reset: Option<i64> = row.get(28)?;
                let quota_remaining: Option<u16> = row.get(29)?;
                let quota_headroom: Option<u16> = row.get(30)?;
                let quota_pressured: Option<bool> = row.get(31)?;
                let quota_exhausted: Option<bool> = row.get(32)?;
                let quota = quota_headroom.map(|headroom| QuotaDecisionSnapshot {
                    reset_ms: quota_reset.map(|ms| u64::try_from(ms).unwrap_or(0)),
                    remaining_permille: quota_remaining,
                    headroom_permille: headroom,
                    pressured: quota_pressured.unwrap_or(false),
                    exhausted: quota_exhausted.unwrap_or(false),
                });
                Ok(DecisionRecord {
                    upstream: row.get(0)?,
                    model: row.get(1)?,
                    pool: row.get(2)?,
                    decided_by: decided_by(
                        3,
                        &kind,
                        row.get(4)?,
                        hint_kind_value.as_deref(),
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    )?,
                    fallbacks: row.get(9)?,
                    features: RequestFeatures {
                        estimated_input_tokens: row.get(10)?,
                        conversation_tokens: row.get(11)?,
                        message_count: row.get(12)?,
                        tool_count: row.get(13)?,
                        has_images: row.get(14)?,
                        requires_json_schema: row.get(15)?,
                        code_block_count: row.get(16)?,
                        requested_max_output_tokens: row.get(17)?,
                        hint_count: row.get(18)?,
                        reasoning_marker_count: row.get(19)?,
                        technical_term_count: row.get(20)?,
                        simple_indicator_count: row.get(21)?,
                        code_keyword_count: row.get(22)?,
                        math_term_count: row.get(23)?,
                        creative_term_count: row.get(24)?,
                        multi_step_signal: row.get(25)?,
                        question_count: row.get(26)?,
                        system_format_hint: row.get(27)?,
                    },
                    quota,
                })
            },
        )
        .optional()
}

fn read_attempts(
    connection: &Connection,
    request_id: &str,
) -> Result<Vec<AttemptRecord>, rusqlite::Error> {
    let mut statement = connection.prepare(
        "SELECT ordinal, upstream, model, latency_ms, http_status, error_code,
                stream_outcome, fallback_allowed, provider_call_engine, south_fallback_reason
           FROM attempts WHERE request_id = ?1 ORDER BY ordinal",
    )?;
    statement
        .query_map([request_id], |row| {
            let outcome = row
                .get::<_, Option<String>>(6)?
                .as_deref()
                .map(|value| stream_outcome(6, value))
                .transpose()?;
            let raw_error_code = row.get::<_, Option<String>>(5)?;
            let fallback_reason = row
                .get::<_, Option<String>>(9)?
                .as_deref()
                .map(|value| south_fallback_reason(9, value))
                .transpose()?;
            Ok(AttemptRecord {
                ordinal: row.get(0)?,
                upstream: row.get(1)?,
                model: row.get(2)?,
                latency_ms: narrow(row.get(3)?),
                http_status: row.get(4)?,
                error_code: optional_error_code(5, raw_error_code.as_deref())?,
                stream_outcome: outcome,
                provider_call_engine: provider_call_engine(8, &row.get::<_, String>(8)?)?,
                south_fallback_reason: fallback_reason,
                fallback_allowed: row.get(7)?,
            })
        })?
        .collect()
}

fn read_conversions(
    connection: &Connection,
    request_id: &str,
) -> Result<Vec<ConversionRecord>, rusqlite::Error> {
    let mut statement = connection.prepare(
        "SELECT ordinal, stage, source_protocol, target_protocol, succeeded, error_code,
                outcome, reason_code, reason_detail
           FROM conversion_reports WHERE request_id = ?1 ORDER BY ordinal",
    )?;
    statement
        .query_map([request_id], |row| {
            let raw_error_code = row.get::<_, Option<String>>(5)?;
            let raw_reason_code = row.get::<_, Option<String>>(7)?;
            let raw_reason_detail = row.get::<_, Option<String>>(8)?;
            Ok(ConversionRecord {
                ordinal: row.get(0)?,
                stage: conversion_stage(1, &row.get::<_, String>(1)?)?,
                source_protocol: row.get(2)?,
                target_protocol: row.get(3)?,
                succeeded: row.get(4)?,
                outcome: conversion_outcome(6, &row.get::<_, String>(6)?)?,
                error_code: optional_error_code(5, raw_error_code.as_deref())?,
                reason_code: raw_reason_code
                    .as_deref()
                    .map(|value| conversion_reason_code(7, value))
                    .transpose()?,
                reason_detail: raw_reason_detail
                    .as_deref()
                    .map(|value| conversion_reason_detail(8, value))
                    .transpose()?,
            })
        })?
        .collect()
}

impl Recorder for SqliteStore {
    fn record(&self, record: &RequestRecord) {
        // Observability never fails a request. The operator learns from
        // stderr; the exchange already succeeded or failed on its own terms.
        if let Err(error) = self.insert(record) {
            eprintln!("metrics store write failed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ReceiptQuery, SqliteStore, recent_receipts};
    use token_station_metrics::{
        AttemptRecord, ConversionOutcome, ConversionRecord, ConversionStage, CostKind,
        DecisionRecord, ProviderCallEngine, QuotaDecisionSnapshot, RecordedDecidedBy, Recorder,
        RequestRecord, RoutingRecord,
    };
    use token_station_protocol::{ErrorCode, StreamOutcome, Usage};
    use token_station_router_core::RequestFeatures;

    const V3_SCHEMA: &str = "
        CREATE TABLE requests (
            id INTEGER PRIMARY KEY,
            request_id TEXT NOT NULL DEFAULT '',
            started_at_ms INTEGER NOT NULL,
            latency_ms INTEGER NOT NULL,
            protocol TEXT NOT NULL,
            requested_model TEXT NOT NULL,
            stream INTEGER NOT NULL,
            status INTEGER NOT NULL,
            error_code TEXT,
            attempts INTEGER NOT NULL,
            upstream TEXT,
            model TEXT,
            pool TEXT,
            tier TEXT,
            rule_id TEXT,
            hint_kind TEXT,
            hint_value TEXT,
            heuristic_score INTEGER,
            heuristic_threshold INTEGER,
            fallbacks INTEGER,
            est_input_tokens INTEGER,
            message_count INTEGER,
            tool_count INTEGER,
            has_images INTEGER,
            requires_json_schema INTEGER,
            code_block_count INTEGER,
            requested_max_output_tokens INTEGER,
            hint_count INTEGER,
            input_tokens INTEGER,
            output_tokens INTEGER,
            cache_read_tokens INTEGER,
            cache_write_tokens INTEGER,
            reasoning_tokens INTEGER,
            cost_micros INTEGER,
            price_version INTEGER
        );
        CREATE UNIQUE INDEX requests_request_id
            ON requests (request_id) WHERE request_id <> '';
        PRAGMA user_version = 3;
    ";

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ts-store-{}-{name}.sqlite", std::process::id()))
    }

    fn receipt(request_id: &str, started_at_ms: u64) -> RequestRecord {
        let features = RequestFeatures {
            estimated_input_tokens: 42,
            conversation_tokens: 30,
            tool_count: 1,
            technical_term_count: 3,
            ..RequestFeatures::default()
        };
        let original = DecisionRecord {
            upstream: "primary".to_owned(),
            model: "model-a".to_owned(),
            pool: "main".to_owned(),
            decided_by: RecordedDecidedBy::Default,
            fallbacks: 1,
            features,
            quota: None,
        };
        let mut record = RequestRecord::begin(started_at_ms, "openai-chat-completions");
        record.request_id = request_id.to_owned();
        record.agent_id = Some("codex".to_owned());
        record.running_revision = Some(7);
        record.requested_model = "auto".to_owned();
        record.status = 200;
        record.attempts = 2;
        record.routing = Some(RoutingRecord {
            upstream: "fallback".to_owned(),
            model: "model-b".to_owned(),
            pool: "main".to_owned(),
            decided_by: RecordedDecidedBy::Default,
            fallbacks: 1,
            features,
        });
        record.decision = Some(original);
        record.attempt_records = vec![
            AttemptRecord {
                ordinal: 1,
                upstream: "primary".to_owned(),
                model: "model-a".to_owned(),
                latency_ms: 12,
                http_status: Some(503),
                error_code: Some(ErrorCode::UpstreamUnavailable),
                stream_outcome: Some(StreamOutcome::FailedBeforeOutput),
                provider_call_engine: ProviderCallEngine::Legacy,
                south_fallback_reason: None,
                fallback_allowed: true,
            },
            AttemptRecord {
                ordinal: 2,
                upstream: "fallback".to_owned(),
                model: "model-b".to_owned(),
                latency_ms: 24,
                http_status: Some(200),
                error_code: None,
                stream_outcome: Some(StreamOutcome::Complete),
                provider_call_engine: ProviderCallEngine::SouthV1Buffered,
                south_fallback_reason: None,
                fallback_allowed: false,
            },
        ];
        record.conversion_reports = vec![
            ConversionRecord {
                ordinal: 1,
                stage: ConversionStage::InboundNormalize,
                source_protocol: "openai-chat-completions".to_owned(),
                target_protocol: "token-station-chat".to_owned(),
                succeeded: true,
                outcome: ConversionOutcome::Succeeded,
                error_code: None,
                reason_code: None,
                reason_detail: None,
            },
            ConversionRecord {
                ordinal: 2,
                stage: ConversionStage::ProviderResponse,
                source_protocol: "openai-compatible".to_owned(),
                target_protocol: "token-station-chat".to_owned(),
                succeeded: true,
                outcome: ConversionOutcome::Succeeded,
                error_code: None,
                reason_code: None,
                reason_detail: None,
            },
        ];
        record.usage = Some(Usage {
            input_tokens: 8,
            output_tokens: 3,
            ..Usage::default()
        });
        record.cost_kind = CostKind::Estimated;
        record.cost_micros = Some(11);
        record.price_version = Some(2);
        record
    }

    #[test]
    fn first_start_creates_the_schema_and_records_survive_reopen() {
        let path = scratch("create");
        std::fs::remove_file(&path).ok();

        {
            let store = SqliteStore::open(&path).expect("creates");
            let mut record = RequestRecord::begin(1_752_000_000_000, "openai-chat-completions");
            record.requested_model = "auto".to_owned();
            record.status = 200;
            record.attempts = 1;
            store.record(&record);
        }

        let store = SqliteStore::open(&path).expect("reopens at the same version");
        let count: i64 = store
            .connection
            .lock()
            .expect("lock")
            .query_row("SELECT count(*) FROM requests", [], |row| row.get(0))
            .expect("counts");
        assert_eq!(count, 1);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn a_quota_decision_snapshot_round_trips_through_the_store() {
        let path = scratch("quota-decision");
        std::fs::remove_file(&path).ok();

        {
            let store = SqliteStore::open(&path).expect("creates");
            let mut record = RequestRecord::begin(1_752_000_000_000, "openai-chat-completions");
            record.request_id = "req_1752000000000_9".to_owned();
            record.requested_model = "auto".to_owned();
            record.status = 200;
            record.attempts = 1;
            record.decision = Some(DecisionRecord {
                upstream: "claude_pro".to_owned(),
                model: "claude".to_owned(),
                pool: "quota".to_owned(),
                decided_by: RecordedDecidedBy::Quota,
                fallbacks: 1,
                features: RequestFeatures::default(),
                quota: Some(QuotaDecisionSnapshot {
                    reset_ms: Some(1_200_000),
                    remaining_permille: Some(640),
                    headroom_permille: 900,
                    pressured: false,
                    exhausted: false,
                }),
            });
            store.record(&record);
        }

        let receipts = recent_receipts(&path, 10).expect("reads");
        assert_eq!(receipts.len(), 1);
        let quota = receipts[0]
            .decision
            .as_ref()
            .and_then(|decision| decision.quota.as_ref())
            .expect("the quota snapshot survives write + read");
        assert_eq!(quota.reset_ms, Some(1_200_000));
        assert_eq!(quota.remaining_permille, Some(640));
        assert_eq!(quota.headroom_permille, 900);
        assert!(!quota.pressured);
        assert!(!quota.exhausted);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn the_same_accounting_id_written_twice_is_one_row() {
        let path = scratch("dedup");
        std::fs::remove_file(&path).ok();

        let store = SqliteStore::open(&path).expect("creates");
        let record = receipt("req_1752000000000_7", 1_752_000_000_000);
        // A rebuild of a derived table replays the same record.
        store.record(&record);
        let mut replay = record.clone();
        replay.attempt_records.push(AttemptRecord {
            ordinal: 3,
            upstream: "must_not_land".to_owned(),
            model: "must_not_land".to_owned(),
            latency_ms: 1,
            http_status: None,
            error_code: Some(ErrorCode::Internal),
            stream_outcome: None,
            provider_call_engine: ProviderCallEngine::Legacy,
            south_fallback_reason: None,
            fallback_allowed: false,
        });
        store.record(&replay);

        let connection = store.connection.lock().expect("lock");
        for (table, expected) in [
            ("requests", 1_i64),
            ("decisions", 1),
            ("attempts", 2),
            ("conversion_reports", 2),
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .expect("counts");
            assert_eq!(count, expected, "{table} is idempotent");
        }

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn a_v3_store_is_migrated_with_a_backup_and_legacy_rows_stay_readable() {
        let path = scratch("migrate");
        std::fs::remove_file(&path).ok();

        {
            let connection = rusqlite::Connection::open(&path).expect("opens");
            connection.execute_batch(V3_SCHEMA).expect("v3 schema");
            connection
                .execute(
                    "INSERT INTO requests (
                    request_id, started_at_ms, latency_ms, protocol, requested_model,
                    stream, status, attempts, upstream, model, pool, tier, fallbacks,
                    input_tokens, output_tokens, cost_micros, price_version
                 ) VALUES ('', 10, 9, 'legacy', 'auto', 0, 200, 3,
                           'old', 'old-model', 'main', 'default', 1, 4, 2, 0, 1)",
                    [],
                )
                .expect("legacy row");
            connection
                .execute(
                    "INSERT INTO requests (
                    request_id, started_at_ms, latency_ms, protocol, requested_model,
                    stream, status, attempts, cost_micros, price_version
                 ) VALUES ('old-priced', 11, 8, 'legacy', 'auto', 0, 200, 1, 25, 3)",
                    [],
                )
                .expect("priced legacy row");
        }

        let store = SqliteStore::open(&path).expect("migrates");
        {
            let connection = store.connection.lock().expect("lock");
            let version: u32 = connection
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .expect("version");
            assert_eq!(version, super::SCHEMA_VERSION, "brought up to current");
            for table in ["requests", "decisions", "attempts", "conversion_reports"] {
                let exists: i64 = connection
                    .query_row(
                        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                        [table],
                        |row| row.get(0),
                    )
                    .expect("schema query");
                assert_eq!(exists, 1, "{table} exists");
            }
        }

        let backup = path.with_extension("v3.bak");
        assert!(
            backup.exists(),
            "the pre-migration database was backed up first"
        );

        drop(store);
        let recent = recent_receipts(&path, 99).expect("legacy receipts read");
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].request_id, "old-priced");
        assert_eq!(recent[0].cost_kind, CostKind::Estimated);
        assert_eq!(recent[0].cost_micros, Some(25));
        assert!(recent[0].attempt_records.is_empty());
        assert!(recent[0].conversion_reports.is_empty());
        assert_eq!(recent[1].attempts, 3);
        assert_eq!(recent[1].cost_kind, CostKind::Estimated);
        assert_eq!(recent[1].cost_micros, Some(0));
        assert_eq!(recent[1].price_version, Some(1));
        assert!(recent[1].request_id.starts_with("legacy-"));
        let legacy_json = serde_json::to_value(&recent[1]).expect("receipt serializes");
        assert!(legacy_json["decision"].is_null());
        assert_eq!(legacy_json["attempt_records"], serde_json::json!([]));
        assert_eq!(legacy_json["conversion_reports"], serde_json::json!([]));

        std::fs::remove_file(&path).ok();
        std::fs::remove_file(&backup).ok();
    }

    #[test]
    fn a_v8_attempt_migrates_to_an_explicit_unknown_provider_call_engine() {
        let path = scratch("migrate-v8-engine");
        std::fs::remove_file(&path).ok();
        {
            let connection = rusqlite::Connection::open(&path).expect("opens");
            connection.execute_batch(V3_SCHEMA).expect("v3 schema");
            for migration in super::MIGRATIONS
                .iter()
                .filter(|migration| (4..=8).contains(&migration.to))
            {
                connection
                    .execute_batch(migration.sql)
                    .expect("migrates to v8");
            }
            connection
                .execute(
                    "INSERT INTO attempts (
                        request_id, ordinal, upstream, model, latency_ms, fallback_allowed
                     ) VALUES ('legacy-attempt', 1, 'old', 'old-model', 7, 0)",
                    [],
                )
                .expect("v8 attempt inserts");
            connection
                .pragma_update(None, "user_version", 8)
                .expect("stamps v8");
        }

        let store = SqliteStore::open(&path).expect("v8 migrates");
        let engine: String = store
            .connection
            .lock()
            .expect("lock")
            .query_row(
                "SELECT provider_call_engine FROM attempts WHERE request_id='legacy-attempt'",
                [],
                |row| row.get(0),
            )
            .expect("engine reads");
        assert_eq!(engine, "unknown");

        drop(store);
        std::fs::remove_file(&path).ok();
        std::fs::remove_file(path.with_extension("v8.bak")).ok();
    }

    #[test]
    fn a_v9_store_preserves_existing_engines_and_admits_south_streaming() {
        let path = scratch("migrate-v9-streaming-engine");
        std::fs::remove_file(&path).ok();
        {
            let connection = rusqlite::Connection::open(&path).expect("opens");
            connection.execute_batch(V3_SCHEMA).expect("v3 schema");
            for migration in super::MIGRATIONS
                .iter()
                .filter(|migration| (4..=9).contains(&migration.to))
            {
                connection
                    .execute_batch(migration.sql)
                    .expect("migrates to v9");
            }
            connection
                .execute(
                    "INSERT INTO attempts (
                        request_id, ordinal, upstream, model, latency_ms,
                        provider_call_engine, fallback_allowed
                     ) VALUES
                        ('legacy-attempt', 1, 'old', 'old-model', 7, 'legacy', 0),
                        ('buffered-attempt', 1, 'old', 'old-model', 8, 'south_v1_buffered', 0)",
                    [],
                )
                .expect("v9 attempts insert");
            connection
                .pragma_update(None, "user_version", 9)
                .expect("stamps v9");
        }

        let store = SqliteStore::open(&path).expect("v9 migrates");
        {
            let connection = store.connection.lock().expect("lock");
            let version: u32 = connection
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .expect("version");
            assert_eq!(version, super::SCHEMA_VERSION);
            let mut engines = connection
                .prepare("SELECT provider_call_engine FROM attempts ORDER BY request_id")
                .expect("query prepares")
                .query_map([], |row| row.get::<_, String>(0))
                .expect("query runs")
                .collect::<Result<Vec<_>, _>>()
                .expect("engines read");
            engines.sort();
            assert_eq!(engines, ["legacy", "south_v1_buffered"]);
            connection
                .execute(
                    "INSERT INTO attempts (
                        request_id, ordinal, upstream, model, latency_ms,
                        provider_call_engine, fallback_allowed
                     ) VALUES ('streaming-attempt', 1, 'new', 'new-model', 9,
                               'south_v1_streaming', 0)",
                    [],
                )
                .expect("v10 admits South streaming engine");
        }

        drop(store);
        std::fs::remove_file(&path).ok();
        std::fs::remove_file(path.with_extension("v9.bak")).ok();
    }

    #[test]
    fn a_child_failure_rolls_the_whole_receipt_back() {
        let path = scratch("transaction-rollback");
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        let mut record = receipt("req-rollback", 1);
        record.attempt_records[1].ordinal = record.attempt_records[0].ordinal;

        assert!(
            store.insert(&record).is_err(),
            "duplicate child ordinal fails"
        );
        let connection = store.connection.lock().expect("lock");
        for table in ["requests", "decisions", "attempts", "conversion_reports"] {
            let count: i64 = connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .expect("counts");
            assert_eq!(count, 0, "{table} rolled back");
        }
        drop(connection);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn recent_receipts_are_hard_capped_ordered_and_include_timelines() {
        let path = scratch("recent");
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        for index in 0..7 {
            store.record(&receipt(&format!("req-{index}"), 100 + index));
        }
        drop(store);

        let recent = recent_receipts(&path, usize::MAX).expect("reads");
        assert_eq!(recent.len(), 5, "backend hard cap");
        assert_eq!(recent[0].request_id, "req-6");
        assert_eq!(recent[4].request_id, "req-2");
        let newest = &recent[0];
        assert_eq!(newest.agent_id.as_deref(), Some("codex"));
        assert_eq!(
            newest.attempt_records[0].provider_call_engine,
            ProviderCallEngine::Legacy
        );
        assert_eq!(
            newest.attempt_records[1].provider_call_engine,
            ProviderCallEngine::SouthV1Buffered
        );
        assert_eq!(newest.running_revision, Some(7));
        assert_eq!(newest.attempts, 2);
        assert_eq!(newest.attempt_records.len(), 2);
        assert_eq!(newest.attempt_records[0].ordinal, 1);
        assert_eq!(newest.conversion_reports.len(), 2);
        assert_eq!(newest.decision.as_ref().unwrap().upstream, "primary");
        assert_eq!(newest.routing.as_ref().unwrap().upstream, "fallback");
        assert_eq!(newest.cost_kind, CostKind::Estimated);
        assert_eq!(newest.cost_micros, Some(11));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn an_explicit_free_price_is_persisted_instead_of_becoming_unknown() {
        let path = scratch("free-cost");
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        let mut record = receipt("req-cost", 1);
        record.cost_kind = CostKind::Estimated;
        record.cost_micros = Some(0);
        record.price_version = Some(9);
        store.record(&record);

        let (kind, cost, version): (String, Option<i64>, Option<u32>) = store
            .connection
            .lock()
            .expect("lock")
            .query_row(
                "SELECT cost_kind, cost_micros, price_version FROM requests",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("reads");
        assert_eq!(kind, "estimated");
        assert_eq!(cost, Some(0));
        assert_eq!(version, Some(9));

        let mut invalid = receipt("req-negative-cost", 2);
        invalid.cost_kind = CostKind::Estimated;
        invalid.cost_micros = Some(-1);
        invalid.price_version = Some(9);
        store.record(&invalid);
        let (kind, cost, version): (String, Option<i64>, Option<u32>) = store
            .connection
            .lock()
            .expect("lock")
            .query_row(
                "SELECT cost_kind, cost_micros, price_version FROM requests WHERE request_id = ?1",
                ["req-negative-cost"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("reads invalid shape");
        assert_eq!((kind.as_str(), cost, version), ("unknown", None, None));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn builtin_prices_backfill_only_previously_unknown_receipts() {
        let path = scratch("price-backfill");
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        let mut unknown = receipt("req-unpriced", 1);
        unknown.routing.as_mut().unwrap().model = "deepseek-v4-pro".to_owned();
        unknown.usage = Some(Usage {
            input_tokens: 1_000_000,
            ..Usage::default()
        });
        unknown.cost_kind = CostKind::Unknown;
        unknown.cost_micros = None;
        unknown.price_version = None;
        store.record(&unknown);

        let mut existing = unknown.clone();
        existing.request_id = "req-existing".to_owned();
        existing.started_at_ms = 2;
        existing.cost_kind = CostKind::Actual;
        existing.cost_micros = Some(123);
        existing.price_version = None;
        store.record(&existing);
        drop(store);

        let updated =
            SqliteStore::backfill_unknown_costs(&path, &crate::pricing::PriceTable::builtin())
                .expect("backfills");
        assert_eq!(updated, 1);

        let store = SqliteStore::open(&path).expect("reopens");
        let rows: Vec<(String, String, Option<i64>, Option<u32>)> = {
            let connection = store.connection.lock().expect("lock");
            let mut statement = connection
                .prepare(
                    "SELECT request_id, cost_kind, cost_micros, price_version
                       FROM requests ORDER BY request_id",
                )
                .expect("prepares");
            statement
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .expect("queries")
                .collect::<Result<_, _>>()
                .expect("decodes")
        };
        assert_eq!(
            rows,
            vec![
                (
                    "req-existing".to_owned(),
                    "actual".to_owned(),
                    Some(123),
                    None
                ),
                (
                    "req-unpriced".to_owned(),
                    "estimated".to_owned(),
                    Some(435_000),
                    Some(1),
                ),
            ],
        );

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn scoped_prices_backfill_same_model_for_each_recorded_upstream() {
        use std::collections::BTreeMap;

        use crate::pricing::{ModelPrice, PriceTable};

        let path = scratch("scoped-price-backfill");
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        for (request_id, upstream) in [
            ("req-provider-a", "provider_a"),
            ("req-provider-b", "provider_b"),
        ] {
            let mut record = receipt(request_id, 1);
            let routing = record.routing.as_mut().expect("fixture has routing");
            routing.upstream = upstream.to_owned();
            routing.model = "shared-model".to_owned();
            record.usage = Some(Usage {
                input_tokens: 1_000_000,
                ..Usage::default()
            });
            record.cost_kind = CostKind::Unknown;
            record.cost_micros = None;
            record.price_version = None;
            store.record(&record);
        }
        drop(store);

        let price = |input_per_mtok| ModelPrice {
            input_per_mtok,
            ..ModelPrice::default()
        };
        let pricing = PriceTable {
            version: 9,
            models: BTreeMap::from([
                ("provider_a/shared-model".to_owned(), price(200_000)),
                ("provider_b/shared-model".to_owned(), price(700_000)),
            ]),
        };

        assert_eq!(
            SqliteStore::backfill_unknown_costs(&path, &pricing).expect("backfills"),
            2
        );
        let rows = SqliteStore::recent_receipts(&path, 5).expect("reads receipts");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].cost_micros, Some(700_000));
        assert_eq!(rows[0].price_version, Some(9));
        assert_eq!(rows[1].cost_micros, Some(200_000));
        assert_eq!(rows[1].price_version, Some(9));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn legacy_receipts_without_upstream_use_the_model_only_price() {
        use std::collections::BTreeMap;

        use crate::pricing::{ModelPrice, PriceTable};

        let path = scratch("legacy-null-upstream-price-backfill");
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        let mut record = receipt("req-legacy", 1);
        record.routing.as_mut().expect("fixture has routing").model = "legacy-model".to_owned();
        record.usage = Some(Usage {
            input_tokens: 1_000_000,
            ..Usage::default()
        });
        record.cost_kind = CostKind::Unknown;
        record.cost_micros = None;
        record.price_version = None;
        store.record(&record);
        store
            .connection
            .lock()
            .expect("lock")
            .execute(
                "UPDATE requests SET upstream = NULL WHERE request_id = ?1",
                ["req-legacy"],
            )
            .expect("represents a legacy row without upstream identity");
        drop(store);

        let pricing = PriceTable {
            version: 4,
            models: BTreeMap::from([(
                "legacy-model".to_owned(),
                ModelPrice {
                    input_per_mtok: 300_000,
                    ..ModelPrice::default()
                },
            )]),
        };

        assert_eq!(
            SqliteStore::backfill_unknown_costs(&path, &pricing).expect("backfills"),
            1
        );
        let rows = SqliteStore::recent_receipts(&path, 5).expect("reads receipts");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cost_micros, Some(300_000));
        assert_eq!(rows[0].price_version, Some(4));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn receipt_page_filters_and_paginates_the_full_ledger() {
        let path = scratch("receipt-page");
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        for index in 0..6 {
            let mut record = receipt(&format!("req-{index}"), 100 + index);
            record.agent_id = Some(
                if index % 2 == 0 {
                    "codex"
                } else {
                    "claude-code"
                }
                .to_owned(),
            );
            record.status = if index == 4 { 500 } else { 200 };
            record.error_code = if index == 4 {
                Some(ErrorCode::UpstreamUnavailable)
            } else {
                None
            };
            record.routing.as_mut().unwrap().model = if index < 3 {
                "deepseek-v4-pro"
            } else {
                "claude-opus-4.8"
            }
            .to_owned();
            store.record(&record);
        }
        drop(store);

        let page = SqliteStore::receipt_page(
            &path,
            &ReceiptQuery {
                agent_id: Some("codex".to_owned()),
                status: Some("success".to_owned()),
                ..ReceiptQuery::default()
            },
            1,
            1,
        )
        .expect("reads page");

        assert_eq!(page.total, 2);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].request_id, "req-0");

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn receipt_schema_has_no_generic_content_or_secret_containers() {
        let path = scratch("privacy-schema");
        std::fs::remove_file(&path).ok();
        let store = SqliteStore::open(&path).expect("creates");
        let connection = store.connection.lock().expect("lock");
        let mut statement = connection
            .prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
            .expect("prepares");
        for table in ["requests", "decisions", "attempts", "conversion_reports"] {
            let columns = statement
                .query_map([table], |row| row.get::<_, String>(0))
                .and_then(Iterator::collect::<Result<Vec<_>, _>>)
                .expect("columns");
            let names = columns.join(" ");
            for forbidden in [
                "prompt",
                "response_body",
                "request_body",
                "raw_header",
                "api_key",
                "secret",
            ] {
                assert!(
                    !names.contains(forbidden),
                    "{table} exposes {forbidden}: {names}"
                );
            }
        }
        drop(statement);
        drop(connection);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn a_database_from_a_newer_schema_is_refused_not_overwritten() {
        let path = scratch("newer");
        std::fs::remove_file(&path).ok();

        {
            let connection = rusqlite::Connection::open(&path).expect("opens");
            connection
                .pragma_update(None, "user_version", 99)
                .expect("stamps");
        }

        let Err(error) = SqliteStore::open(&path) else {
            panic!("version 99 is not ours")
        };
        assert!(error.contains("99"), "{error}");

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn schema_inspection_is_read_only_and_classifies_a_future_database() {
        let path = scratch("inspect-newer");
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .pragma_update(None, "user_version", super::SCHEMA_VERSION + 7)
                .unwrap();
        }
        let before = std::fs::read(&path).unwrap();

        assert_eq!(
            super::inspect_schema(&path).unwrap(),
            super::SchemaCompatibility::Newer {
                found: super::SCHEMA_VERSION + 7,
                supported: super::SCHEMA_VERSION,
            }
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn online_snapshot_copies_a_future_schema_without_migrating_the_source() {
        let path = scratch("snapshot-future");
        let snapshot = path.with_extension("snapshot.sqlite");
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute("CREATE TABLE future_data(value TEXT)", [])
                .unwrap();
            connection
                .execute("INSERT INTO future_data VALUES ('kept')", [])
                .unwrap();
            connection
                .pragma_update(None, "user_version", super::SCHEMA_VERSION + 1)
                .unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        super::snapshot_database(&path, &snapshot).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let copied = rusqlite::Connection::open(&snapshot).unwrap();
        assert_eq!(
            copied
                .query_row("SELECT value FROM future_data", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "kept"
        );
        std::fs::remove_file(path).ok();
        std::fs::remove_file(snapshot).ok();
    }
}

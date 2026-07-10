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

use rusqlite::Connection;
use token_station_metrics::{Recorder, RequestRecord, SCHEMA_VERSION};

/// One row per exchange, flattened from `RequestRecord`.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS requests (
    id                  INTEGER PRIMARY KEY,
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
    -- micro-units; NULL until the pricing table exists (C2#4)
    cost_micros         INTEGER
);
CREATE INDEX IF NOT EXISTS requests_started_at ON requests (started_at_ms);
";

/// The SQLite-backed [`Recorder`].
pub struct SqliteStore {
    connection: Mutex<Connection>,
}

impl SqliteStore {
    /// Opens (or creates) the database and brings the schema up.
    ///
    /// # Errors
    ///
    /// A message for the operator: the store failing to open is a startup
    /// error, unlike a single record failing to write.
    pub fn open(path: &Path) -> Result<Self, String> {
        let connection = Connection::open(path)
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
            other => {
                // A newer client wrote here. Refusing beats silently writing
                // rows a future schema no longer means the same thing by.
                return Err(format!(
                    "metrics store `{}` has schema version {other}, this build knows {SCHEMA_VERSION}",
                    path.display()
                ));
            }
        }

        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn insert(&self, record: &RequestRecord) -> Result<(), rusqlite::Error> {
        // SQLite integers are i64; token counts saturate rather than error.
        fn wide(value: u64) -> i64 {
            i64::try_from(value).unwrap_or(i64::MAX)
        }
        let routing = record.routing.as_ref();
        let features = routing.map(|routing| routing.features);
        let (tier, rule_id, hint_kind, hint_value, score, threshold) =
            match routing.map(|r| &r.decided_by) {
                Some(token_station_router_core::DecidedBy::Rule { rule }) => {
                    ("rule", Some(rule.clone()), None, None, None, None)
                }
                Some(token_station_router_core::DecidedBy::Hint { kind, value }) => (
                    "hint",
                    None,
                    Some(format!("{kind:?}").to_lowercase()),
                    Some(value.clone()),
                    None,
                    None,
                ),
                Some(token_station_router_core::DecidedBy::Heuristic { score, threshold }) => (
                    "heuristic",
                    None,
                    None,
                    None,
                    Some(*score),
                    Some(*threshold),
                ),
                Some(token_station_router_core::DecidedBy::Default) => {
                    ("default", None, None, None, None, None)
                }
                None => ("", None, None, None, None, None),
            };

        let connection = self.connection.lock().expect("store lock");
        connection.execute(
            "INSERT INTO requests (
                started_at_ms, latency_ms, protocol, requested_model, stream, status,
                error_code, attempts,
                upstream, model, pool, tier, rule_id, hint_kind, hint_value,
                heuristic_score, heuristic_threshold, fallbacks,
                est_input_tokens, message_count, tool_count, has_images,
                requires_json_schema, code_block_count, requested_max_output_tokens, hint_count,
                input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                reasoning_tokens, cost_micros
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32
            )",
            rusqlite::params![
                wide(record.started_at_ms),
                wide(record.latency_ms),
                record.protocol,
                record.requested_model,
                record.stream,
                record.status,
                record
                    .error_code
                    .map(|code| format!("{code:?}").to_lowercase()),
                record.attempts,
                routing.map(|r| r.upstream.clone()),
                routing.map(|r| r.model.clone()),
                routing.map(|r| r.pool.clone()),
                routing.map(|_| tier),
                rule_id,
                hint_kind,
                hint_value,
                score,
                threshold,
                routing.map(|r| r.fallbacks),
                features.map(|f| f.estimated_input_tokens),
                features.map(|f| f.message_count),
                features.map(|f| f.tool_count),
                features.map(|f| f.has_images),
                features.map(|f| f.requires_json_schema),
                features.map(|f| f.code_block_count),
                features.and_then(|f| f.requested_max_output_tokens),
                features.map(|f| f.hint_count),
                record.usage.map(|u| wide(u.input_tokens)),
                record.usage.map(|u| wide(u.output_tokens)),
                record.usage.map(|u| wide(u.cache_read_tokens)),
                record.usage.map(|u| wide(u.cache_write_tokens)),
                record.usage.map(|u| wide(u.reasoning_tokens)),
                record.cost_micros,
            ],
        )?;
        Ok(())
    }
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
    use super::SqliteStore;
    use token_station_metrics::{Recorder, RequestRecord};

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ts-store-{}-{name}.sqlite", std::process::id()))
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
}

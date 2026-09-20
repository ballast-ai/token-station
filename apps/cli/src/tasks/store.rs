//! `SQLite` composite effects. No money tables or hidden network calls.
use super::{Binding, TaskRow, terminal};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

pub struct TaskStore {
    pub(crate) connection: Connection,
    #[cfg(test)]
    after_observation_update: Option<Box<dyn FnOnce() + Send>>,
}
fn error(_: rusqlite::Error) -> String {
    "task database operation failed".into()
}
fn db_version(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| "task version exceeds database range".into())
}
fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRow> {
    let json: String = row.get(4)?;
    let binding: Binding = serde_json::from_str(&json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(TaskRow {
        id: row.get(0)?,
        request_hash: row.get(1)?,
        submission_state: row.get(2)?,
        execution_state: row.get(3)?,
        binding,
        upstream_id: row.get(5)?,
        cancel_requested: row.get(6)?,
        version: u64::try_from(row.get::<_, i64>(7)?)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(7, -1))?,
    })
}
const COLUMNS: &str =
    "id,request_hash,submission_state,execution_state,binding,upstream_id,cancel_requested,version";
pub enum Prepared {
    Created(TaskRow),
    Replayed(TaskRow),
    Conflict(TaskRow),
}
impl TaskStore {
    /// Opens a private task database and applies its schema.
    ///
    /// # Errors
    /// Returns an error when storage is unavailable, the row is malformed, or a version is out of range.
    pub fn open(dir: &Path) -> Result<Self, String> {
        token_station_private_fs::ensure_private_dir(dir)
            .map_err(|_| "task directory must be private".to_owned())?;
        let path = dir.join("tasks.sqlite");
        token_station_private_fs::create_private_file(&path, b"")
            .map_err(|_| "task database must be private".to_owned())?;
        let mut connection = Connection::open(path).map_err(error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(2))
            .map_err(error)?;
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(error)?;
        if version > 1 {
            return Err("task database schema is newer than this host".into());
        }
        if version == 0 {
            let tx = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(error)?;
            tx.execute_batch("CREATE TABLE IF NOT EXISTS tasks(id TEXT PRIMARY KEY,idempotency_key TEXT NOT NULL UNIQUE,request_hash TEXT NOT NULL,submission_state TEXT NOT NULL,execution_state TEXT NOT NULL,binding TEXT NOT NULL,upstream_id TEXT,cancel_requested INTEGER NOT NULL DEFAULT 0,version INTEGER NOT NULL DEFAULT 0);CREATE TABLE IF NOT EXISTS task_events(task_id TEXT NOT NULL,kind TEXT NOT NULL,PRIMARY KEY(task_id,kind));PRAGMA user_version=1;").map_err(error)?;
            tx.commit().map_err(error)?;
        }
        Ok(Self {
            connection,
            #[cfg(test)]
            after_observation_update: None,
        })
    }
    /// Reports whether the database already contains durable task identities.
    ///
    /// # Errors
    /// Returns an error when storage is unavailable, the row is malformed, or a version is out of range.
    pub fn has_tasks(&self) -> Result<bool, String> {
        self.connection
            .query_row("SELECT EXISTS(SELECT 1 FROM tasks)", [], |r| r.get(0))
            .map_err(error)
    }
    /// Loads a task by its immutable local identity.
    ///
    /// # Errors
    /// Returns an error when storage is unavailable, the row is malformed, or a version is out of range.
    pub fn get(&self, id: &str) -> Result<TaskRow, String> {
        self.connection
            .query_row(
                &format!("SELECT {COLUMNS} FROM tasks WHERE id=?1"),
                [id],
                row,
            )
            .optional()
            .map_err(error)?
            .ok_or_else(|| "task not found".into())
    }
    /// Looks up the original task for an idempotency key.
    ///
    /// # Errors
    /// Returns an error when storage is unavailable, the row is malformed, or a version is out of range.
    pub fn by_key(&self, key: &str) -> Result<Option<TaskRow>, String> {
        self.connection
            .query_row(
                &format!("SELECT {COLUMNS} FROM tasks WHERE idempotency_key=?1"),
                [key],
                row,
            )
            .optional()
            .map_err(error)
    }
    /// Atomically inserts the prepared task and its event, or returns the existing key owner.
    ///
    /// # Errors
    /// Returns an error when storage is unavailable, the row is malformed, or a version is out of range.
    pub fn prepare(
        &mut self,
        id: &str,
        key: &str,
        hash: &str,
        binding: &Binding,
    ) -> Result<Prepared, String> {
        let binding_json =
            serde_json::to_string(binding).map_err(|_| "invalid task binding".to_owned())?;
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(error)?;
        let existing = tx
            .query_row(
                &format!("SELECT {COLUMNS} FROM tasks WHERE idempotency_key=?1"),
                [key],
                row,
            )
            .optional()
            .map_err(error)?;
        if let Some(existing) = existing {
            return Ok(if existing.request_hash == hash {
                Prepared::Replayed(existing)
            } else {
                Prepared::Conflict(existing)
            });
        }
        tx.execute("INSERT INTO tasks(id,idempotency_key,request_hash,submission_state,execution_state,binding) VALUES(?1,?2,?3,'prepared','queued',?4)",params![id,key,hash,binding_json]).map_err(error)?;
        tx.execute(
            "INSERT INTO task_events(task_id,kind) VALUES(?1,'prepared')",
            [id],
        )
        .map_err(error)?;
        tx.commit().map_err(error)?;
        Ok(Prepared::Created(self.get(id)?))
    }
    /// Claims the prepared task for one submission under its expected version.
    ///
    /// # Errors
    /// Returns an error when storage is unavailable, the row is malformed, or a version is out of range.
    pub fn dispatch(&mut self, task: &TaskRow) -> Result<bool, String> {
        self.connection.execute("UPDATE tasks SET submission_state='submitting',version=version+1 WHERE id=?1 AND version=?2 AND submission_state='prepared' AND execution_state='queued' AND cancel_requested=0",params![task.id,db_version(task.version)?]).map(|n|n==1).map_err(error)
    }
    /// Atomically records the submission fact under the dispatch claim.
    ///
    /// # Errors
    /// Returns an error when storage is unavailable, the row is malformed, or a version is out of range.
    pub fn record_submission(
        &mut self,
        task: &TaskRow,
        state: &str,
        upstream_id: Option<&str>,
    ) -> Result<bool, String> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(error)?;
        let changed=tx.execute("UPDATE tasks SET submission_state=?3,upstream_id=?4,execution_state=?5,version=version+1 WHERE id=?1 AND version=?2 AND submission_state='submitting'",params![task.id,db_version(task.version.checked_add(1).ok_or("task version overflow")?)?,state,upstream_id,if state=="rejected"{"failed"}else if state=="unknown"{"status_unknown"}else{"queued"}]).map_err(error)?;
        if changed == 1 && state == "rejected" {
            tx.execute(
                "INSERT OR IGNORE INTO task_events(task_id,kind) VALUES(?1,'terminal')",
                [&task.id],
            )
            .map_err(error)?;
        }
        tx.commit().map_err(error)?;
        Ok(changed == 1)
    }
    /// Applies an observation and its terminal event under an expected version.
    ///
    /// # Errors
    /// Returns an error when storage is unavailable, the row is malformed, or a version is out of range.
    pub fn apply(&mut self, task: &TaskRow, state: &str) -> Result<bool, String> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(error)?;
        let changed=tx.execute("UPDATE tasks SET execution_state=?3,version=version+1 WHERE id=?1 AND version=?2 AND execution_state NOT IN ('succeeded','failed','cancelled')",params![task.id,db_version(task.version)?,state]).map_err(error)?;
        if changed == 1 && terminal(state) {
            tx.execute(
                "INSERT OR IGNORE INTO task_events(task_id,kind) VALUES(?1,'terminal')",
                [&task.id],
            )
            .map_err(error)?;
        }
        tx.commit().map_err(error)?;
        Ok(changed == 1)
    }
    /// Claims and returns one observation version in the same write transaction.
    ///
    /// # Errors
    /// Returns an error when storage is unavailable, the row is malformed, or a version is out of range.
    pub fn acquire_observation(&mut self, task: &TaskRow) -> Result<Option<TaskRow>, String> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(error)?;
        let changed = tx.execute("UPDATE tasks SET version=version+1 WHERE id=?1 AND version=?2 AND execution_state NOT IN ('succeeded','failed','cancelled')", params![task.id,db_version(task.version)?]).map_err(error)?;
        #[cfg(test)]
        if let Some(hook) = self.after_observation_update.take() {
            hook();
        }
        let claimed = if changed == 1 {
            Some(
                tx.query_row(
                    &format!("SELECT {COLUMNS} FROM tasks WHERE id=?1"),
                    [&task.id],
                    row,
                )
                .map_err(error)?,
            )
        } else {
            None
        };
        tx.commit().map_err(error)?;
        Ok(claimed)
    }
    /// Atomically cancels a task that has not won the dispatch gate.
    ///
    /// # Errors
    /// Returns an error when storage is unavailable, the row is malformed, or a version is out of range.
    pub fn cancel_prepared(&mut self, task: &TaskRow) -> Result<bool, String> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(error)?;
        let changed=tx.execute("UPDATE tasks SET execution_state='cancelled',cancel_requested=1,version=version+1 WHERE id=?1 AND version=?2 AND submission_state='prepared' AND execution_state='queued'",params![task.id,db_version(task.version)?]).map_err(error)?;
        if changed == 1 {
            tx.execute(
                "INSERT OR IGNORE INTO task_events(task_id,kind) VALUES(?1,'terminal')",
                [&task.id],
            )
            .map_err(error)?;
        }
        tx.commit().map_err(error)?;
        Ok(changed == 1)
    }
    /// Records intent without asserting that the upstream stopped execution.
    ///
    /// # Errors
    /// Returns an error when storage is unavailable, the row is malformed, or a version is out of range.
    pub fn cancel_intent(&mut self, id: &str) -> Result<(), String> {
        // Intent does not invalidate the in-flight submission claim or invent a terminal fact.
        self.connection.execute("UPDATE tasks SET cancel_requested=1 WHERE id=?1 AND execution_state NOT IN ('succeeded','failed','cancelled')",[id]).map_err(error)?;
        Ok(())
    }
}

#[cfg(test)]
mod atomic_contract {
    use super::*;
    use south_task_conformance::{AtomicHarness, Completion, Fault, HarnessFuture, Probe};
    struct Driver {
        store: TaskStore,
        root: std::path::PathBuf,
    }
    impl Drop for Driver {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn binding() -> Binding {
        Binding {
            schema_version: 1,
            provider: super::super::Provider {
                endpoint: "https://example.invalid".into(),
                model: "happyhorse-1.0".into(),
                dialect: "bailian_video".into(),
                pin: super::super::ComponentPin {
                    world: "task-adapter-v2".into(),
                    name: "task-bailian-v2".into(),
                    version: "0.31.0".into(),
                    manifest_sha256: "0".repeat(64),
                    wasm_sha256: "0".repeat(64),
                },
                credential: super::super::CredentialRef {
                    upstream: "contract".into(),
                    slot: "provider_api_key".into(),
                },
            },
            locator: serde_json::json!({"schema_version":1,"route":"api/v1/tasks"}),
            credential_mac: "1".repeat(64),
        }
    }
    impl Driver {
        fn trigger(&self, kind: &str) -> Result<(), String> {
            self.store.connection.execute_batch(&format!("CREATE TEMP TRIGGER injected_task_failure BEFORE INSERT ON task_events WHEN NEW.kind='{kind}' BEGIN SELECT RAISE(ABORT,'injected final write failure'); END;")).map_err(error)
        }
        fn clear(&self) -> Result<(), String> {
            self.store
                .connection
                .execute_batch("DROP TRIGGER IF EXISTS injected_task_failure")
                .map_err(error)
        }
    }
    impl AtomicHarness for Driver {
        fn expected_resource_digest(&self, _: Completion) -> Option<[u8; 32]> {
            None
        }
        fn reset(&mut self) -> HarnessFuture<'_, ()> {
            Box::pin(async move {
                self.clear()?;
                self.store
                    .connection
                    .execute_batch("DELETE FROM task_events;DELETE FROM tasks;")
                    .map_err(error)
            })
        }
        fn prepare(
            &mut self,
            different: bool,
            fault: Fault,
        ) -> HarnessFuture<'_, south_task_conformance::Prepared> {
            Box::pin(async move {
                if fault == Fault::BeforePrepareCommit {
                    self.trigger("prepared")?;
                }
                let result = self.store.prepare(
                    "task-id",
                    "key",
                    if different { "different" } else { "same" },
                    &binding(),
                );
                self.clear()?;
                let result = result?;
                if fault == Fault::AfterPrepareCommit {
                    return Err("injected committed reply loss".into());
                }
                Ok(match result {
                    Prepared::Created(_) => south_task_conformance::Prepared::Created,
                    Prepared::Replayed(_) => south_task_conformance::Prepared::Replayed,
                    Prepared::Conflict(_) => south_task_conformance::Prepared::Conflict,
                })
            })
        }
        fn dispatch(&mut self) -> HarnessFuture<'_, bool> {
            Box::pin(async move { self.store.dispatch(&self.store.get("task-id")?) })
        }
        fn guard(&mut self) -> HarnessFuture<'_, u64> {
            Box::pin(async move { Ok(self.store.get("task-id")?.version) })
        }
        fn apply(
            &mut self,
            guard: u64,
            completion: Completion,
            fault: Fault,
        ) -> HarnessFuture<'_, bool> {
            Box::pin(async move {
                if fault == Fault::BeforeApplyCommit {
                    self.trigger("terminal")?;
                }
                let mut task = self.store.get("task-id")?;
                task.version = guard;
                let state = match completion {
                    Completion::Succeeded => "succeeded",
                    Completion::Failed => "failed",
                    Completion::CancelledUnknown | Completion::CancelledNoCharge => "cancelled",
                };
                let result = self.store.apply(&task, state);
                self.clear()?;
                let changed = result?;
                if fault == Fault::AfterApplyCommit {
                    return Err("injected committed reply loss".into());
                }
                Ok(changed)
            })
        }
        fn takeover(&mut self) -> HarnessFuture<'_, ()> {
            Box::pin(async move {
                let task = self.store.get("task-id")?;
                self.store.acquire_observation(&task)?.ok_or("claim lost")?;
                Ok(())
            })
        }
        fn cancel_prepared(&mut self) -> HarnessFuture<'_, bool> {
            Box::pin(async move { self.store.cancel_prepared(&self.store.get("task-id")?) })
        }
        fn snapshot(&mut self) -> HarnessFuture<'_, Probe> {
            Box::pin(async move {
                let tasks = self
                    .store
                    .connection
                    .query_row("SELECT count(*) FROM tasks", [], |r| r.get(0))
                    .map_err(error)?;
                let terminal_events = self
                    .store
                    .connection
                    .query_row(
                        "SELECT count(*) FROM task_events WHERE kind='terminal'",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(error)?;
                let row = self.store.by_key("key")?;
                Ok(Probe {
                    tasks,
                    execution: match row.as_ref().map(|r| r.execution_state.as_str()) {
                        None => south_task_conformance::Execution::Absent,
                        Some("queued") => south_task_conformance::Execution::Queued,
                        Some("running") => south_task_conformance::Execution::Running,
                        Some("succeeded") => south_task_conformance::Execution::Succeeded,
                        Some("failed") => south_task_conformance::Execution::Failed,
                        Some("cancelled") => south_task_conformance::Execution::Cancelled,
                        _ => south_task_conformance::Execution::Unknown,
                    },
                    terminal_events,
                    terminal: row.as_ref().is_some_and(|r| terminal(&r.execution_state)),
                    held: false,
                    version: row.map_or(0, |r| r.version),
                    reserve_effects: None,
                    close_effects: None,
                    resource_digest: None,
                })
            })
        }
    }

    #[test]
    fn observation_claim_returns_only_its_own_committed_fence() {
        let root = std::env::temp_dir().join(format!(
            "ts-task-fence-{}",
            super::super::random_id().unwrap()
        ));
        let mut first = TaskStore::open(&root).unwrap();
        first.prepare("task-id", "key", "hash", &binding()).unwrap();
        let original = first.get("task-id").unwrap();
        let mut second = TaskStore::open(&root).unwrap();
        second
            .connection
            .busy_timeout(std::time::Duration::ZERO)
            .unwrap();
        let interleaved = std::sync::Arc::new(std::sync::Mutex::new(None));
        let result = std::sync::Arc::clone(&interleaved);
        first.after_observation_update = Some(Box::new(move || {
            let latest = second.get("task-id").unwrap();
            *result.lock().unwrap() = Some(second.acquire_observation(&latest));
        }));
        let claimed = first.acquire_observation(&original).unwrap().unwrap();
        assert_eq!(
            claimed.version,
            original.version + 1,
            "caller must not borrow another caller's newer fence"
        );
        assert!(
            interleaved.lock().unwrap().take().unwrap().is_err(),
            "competing writer must not enter UPDATE/read transaction gap"
        );
        let mut later = TaskStore::open(&root).unwrap();
        let takeover = later.acquire_observation(&claimed).unwrap().unwrap();
        assert!(takeover.version > claimed.version);
        assert!(!first.apply(&claimed, "succeeded").unwrap());
        assert!(later.apply(&takeover, "succeeded").unwrap());
        drop(first);
        drop(later);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn community_sqlite_satisfies_shared_atomic_task_contract() {
        let root = std::env::temp_dir().join(format!(
            "ts-task-atomic-{}",
            super::super::random_id().unwrap()
        ));
        let mut driver = Driver {
            store: TaskStore::open(&root).unwrap(),
            root,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let reports = runtime
            .block_on(south_task_conformance::run_atomic_suite(&mut driver))
            .unwrap();
        assert_eq!(reports.len(), 11);
    }
}

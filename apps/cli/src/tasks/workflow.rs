//! Host policy adapters for the shared task workflows.
use super::{
    Binding, Outcome, TaskCommand, TaskConfig, TaskRow, TaskStore, digest, executor, random_id,
    read_bounded, store, terminal,
};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use south_component_conformance::{SubmitOutcomeV2, task_v2_json};
use south_contracts::{TaskFailureKindV1, TaskObservationV2};
use south_task_core::{
    self as core, ApplyResult, CancelPlan, HostFuture, ObservationPlan, PrepareResult,
    SubmissionFact, WaitState,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct Host {
    config: TaskConfig,
    data: PathBuf,
    store: TaskStore,
    mac_key: Option<Vec<u8>>,
    pending: Option<(executor::Executor, task_protocol::HttpRequestDescriptor)>,
    fresh: Option<TaskObservationV2>,
    restored_executor: Option<(String, std::sync::Arc<executor::Executor>)>,
}
pub struct Input {
    provider: String,
    key: String,
    hash: String,
    request: Value,
}
pub struct Mapped {
    state: &'static str,
    observation: Option<TaskObservationV2>,
}
impl Host {
    pub fn new(config: TaskConfig, data: PathBuf) -> Result<Self, String> {
        let store = TaskStore::open(&config.state_dir)?;
        let key_path = config.state_dir.join("credential-hmac.key");
        if !key_path.exists() && !store.has_tasks()? {
            let mut bytes = [0; 32];
            getrandom::fill(&mut bytes).map_err(|_| "credential entropy unavailable")?;
            token_station_private_fs::create_private_file(&key_path, &bytes)
                .map_err(|_| "cannot initialize private credential guard")?;
        }
        let mac_key = if key_path.exists() {
            token_station_private_fs::verify_private_file(&key_path)
                .map_err(|_| "credential guard is not private")?;
            let bytes = read_bounded(&key_path, 32)?;
            if bytes.len() != 32 {
                return Err("invalid credential guard".into());
            }
            Some(bytes)
        } else {
            None
        };

        Ok(Self {
            config,
            data,
            store,
            mac_key,
            pending: None,
            fresh: None,
            restored_executor: None,
        })
    }
    fn secret(&self, provider: &super::Provider) -> Result<String, String> {
        let reference = &provider.credential;
        let value = crate::secrets::store_get(&self.data, &reference.upstream, &reference.slot)?;
        if value.trim() != value
            || value.is_empty()
            || value.len() > 8192
            || value.chars().any(char::is_control)
        {
            return Err("task credential material is invalid".into());
        }
        Ok(value)
    }
    fn credential_hasher(
        &self,
        provider: &super::Provider,
        secret: &str,
    ) -> Result<Hmac<Sha256>, String> {
        let mut mac = Hmac::<Sha256>::new_from_slice(
            self.mac_key
                .as_deref()
                .ok_or("original credential guard is missing")?,
        )
        .map_err(|_| "invalid credential guard")?;
        mac.update(
            &serde_json::to_vec(&(&provider.credential, secret))
                .map_err(|_| "invalid credential reference")?,
        );
        Ok(mac)
    }
    fn credential_mac(&self, provider: &super::Provider, secret: &str) -> Result<String, String> {
        Ok(super::hex(
            &self
                .credential_hasher(provider, secret)?
                .finalize()
                .into_bytes(),
        ))
    }
    fn verify_credential(&self, binding: &Binding, secret: &str) -> Result<(), String> {
        let encoded = binding.credential_mac.as_bytes();
        if encoded.len() != 64 {
            return Err("invalid saved credential guard".into());
        }
        let mut expected = [0u8; 32];
        for (chunk, value) in encoded.chunks_exact(2).zip(expected.iter_mut()) {
            let text = std::str::from_utf8(chunk).map_err(|_| "invalid saved credential guard")?;
            *value = u8::from_str_radix(text, 16).map_err(|_| "invalid saved credential guard")?;
        }
        self.credential_hasher(&binding.provider, secret)?
            .verify_slice(&expected)
            .map_err(|_| "saved task credential material has changed".into())
    }
    fn restored(
        &mut self,
        row: &TaskRow,
    ) -> Result<(std::sync::Arc<executor::Executor>, String), String> {
        if row.binding.schema_version != 1 {
            return Err("unsupported saved task binding".into());
        }
        let secret = self.secret(&row.binding.provider)?;
        self.verify_credential(&row.binding, &secret)?;
        let executor = match &self.restored_executor {
            Some((id, executor)) if id == &row.id => std::sync::Arc::clone(executor),
            _ => {
                let executor = std::sync::Arc::new(executor::Executor::load(
                    &self.config.components_dir,
                    &row.binding.provider,
                )?);
                self.restored_executor = Some((row.id.clone(), std::sync::Arc::clone(&executor)));
                executor
            }
        };
        Ok((executor, secret))
    }
    fn outcome(&self, row: TaskRow) -> Outcome {
        let succeeded = row.execution_state == "succeeded";
        let mut output = Outcome::from(row);
        if succeeded && matches!(self.fresh, Some(TaskObservationV2::Succeeded { .. })) {
            output.observation.clone_from(&self.fresh);
        }
        output
    }
    async fn fetch(&mut self, id: &str, output: &std::path::Path) -> Result<Outcome, String> {
        let observed = core::observe(self, id).await?;
        if observed.execution_state != "succeeded" {
            return Err("task artifact is not deliverable".into());
        }
        let observation = observed
            .observation
            .as_ref()
            .ok_or("fresh task artifact is unavailable")?;
        let row = self.store.get(id)?;
        let (executor, secret) = self.restored(&row)?;
        let body = executor
            .render(
                &row.binding,
                id,
                row.upstream_id
                    .as_deref()
                    .ok_or("task has no confirmed upstream id")?,
                observation,
                &secret,
            )
            .await?;
        let url = body
            .get("data")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(|a| a.get("url"))
            .and_then(Value::as_str)
            .ok_or("component returned no video artifact")?;
        executor::fetch(url, output)?;
        Ok(observed)
    }
    pub async fn command(&mut self, command: TaskCommand) -> Result<Value, String> {
        let outcome = match command {
            TaskCommand::Submit {
                provider,
                request,
                idempotency_key,
            } => {
                if idempotency_key.is_empty()
                    || idempotency_key.len() > 256
                    || idempotency_key.chars().any(char::is_control)
                {
                    return Err("invalid idempotency key".into());
                }
                let request: Value = serde_json::from_slice(&read_bounded(&request, 1024 * 1024)?)
                    .map_err(|_| "invalid task request JSON")?;
                let hash = digest(
                    &serde_json::to_vec(&(&provider, &request))
                        .map_err(|_| "invalid task request")?,
                );
                let result = core::submit(
                    self,
                    &Input {
                        provider,
                        key: idempotency_key,
                        hash,
                        request,
                    },
                )
                .await?;
                if result.disposition == core::Disposition::Conflict {
                    return Err("idempotency key belongs to another request".into());
                }
                result.outcome
            }
            TaskCommand::Inspect { id } => self.outcome(self.store.get(&id)?),
            TaskCommand::Observe { id } => core::observe(self, id.as_str()).await?,
            TaskCommand::Cancel { id } => core::cancel(self, id.as_str()).await?,
            TaskCommand::Wait {
                id,
                timeout_seconds,
            } => {
                if timeout_seconds == 0 || timeout_seconds > 3600 {
                    return Err("task wait timeout must be within 1..=3600 seconds".into());
                }
                // Package verification/JIT is setup, performed before the waiting clock starts.
                // The HTTP future itself stays cancellable under the shared deadline.
                let row = self.store.get(&id)?;
                if row.upstream_id.is_some() && !terminal(&row.execution_state) {
                    let _ = self.restored(&row);
                }
                let clock = Clock(Instant::now());
                match core::wait(
                    self,
                    id.as_str(),
                    &clock,
                    Duration::from_secs(timeout_seconds),
                    Duration::from_millis(250),
                )
                .await
                .map_err(|_| "task wait failed")?
                {
                    core::WaitResult::Ready(value) => value,
                    core::WaitResult::TimedOut => {
                        let mut value = serde_json::to_value(self.outcome(self.store.get(&id)?))
                            .map_err(|_| "invalid task output")?;
                        value["wait_outcome"] = json!("timed_out");
                        return Ok(value);
                    }
                    core::WaitResult::Cancelled => return Err("task waiter cancelled".into()),
                }
            }
            TaskCommand::Fetch { id, output } => self.fetch(&id, &output).await?,
        };
        serde_json::to_value(outcome).map_err(|_| "invalid task output".into())
    }
}
impl core::SubmissionEffects for Host {
    type Input = Input;
    type Task = TaskRow;
    type Accepted = String;
    type Rejected = ();
    type Outcome = Outcome;
    type Error = String;
    fn prepare<'a>(
        &'a mut self,
        input: &'a Input,
    ) -> HostFuture<'a, PrepareResult<TaskRow, Outcome>, String> {
        Box::pin(async move {
            if let Some(row) = self.store.by_key(&input.key)? {
                return Ok(if row.request_hash == input.hash {
                    PrepareResult::Replayed(self.outcome(row))
                } else {
                    PrepareResult::Conflict(self.outcome(row))
                });
            }
            let provider = self
                .config
                .providers
                .get(&input.provider)
                .ok_or("task provider is not configured")?
                .clone();
            let id = random_id()?;
            let secret = self.secret(&provider)?;
            let executor = executor::Executor::load(&self.config.components_dir, &provider)?;
            let mut request = input
                .request
                .as_object()
                .cloned()
                .ok_or("task request must be an object")?;
            request.insert("model".into(), json!(provider.model));
            let prepared = executor.prepare(&Value::Object(request), &id)?;
            let binding = Binding {
                schema_version: 1,
                credential_mac: self.credential_mac(&provider, &secret)?,
                provider,
                locator: task_v2_json::locator_json(&prepared.locator),
            };
            match self.store.prepare(&id, &input.key, &input.hash, &binding)? {
                store::Prepared::Created(row) => {
                    self.pending = Some((executor, prepared.descriptor));
                    Ok(PrepareResult::Created(row))
                }
                store::Prepared::Replayed(row) => Ok(PrepareResult::Replayed(self.outcome(row))),
                store::Prepared::Conflict(row) => Ok(PrepareResult::Conflict(self.outcome(row))),
            }
        })
    }
    fn begin_dispatch<'a>(&'a mut self, task: &'a TaskRow) -> HostFuture<'a, bool, String> {
        Box::pin(async move { self.store.dispatch(task) })
    }
    fn send<'a>(
        &'a mut self,
        task: &'a TaskRow,
    ) -> HostFuture<'a, SubmissionFact<String, ()>, String> {
        Box::pin(async move {
            let secret = self.secret(&task.binding.provider)?;
            self.verify_credential(&task.binding, &secret)?;
            let (executor, descriptor) = self
                .pending
                .take()
                .ok_or("task submission preparation is unavailable")?;
            Ok(match executor.submit(&descriptor, &secret).await? {
                SubmitOutcomeV2::Accepted(id) => SubmissionFact::Accepted(id),
                SubmitOutcomeV2::Rejected(_) => SubmissionFact::Rejected(()),
                SubmitOutcomeV2::Unknown | SubmitOutcomeV2::AcceptedTerminal(_) => {
                    SubmissionFact::Unknown
                }
            })
        })
    }
    fn record_submission<'a>(
        &'a mut self,
        task: &'a TaskRow,
        fact: SubmissionFact<String, ()>,
    ) -> HostFuture<'a, ApplyResult<Outcome>, String> {
        Box::pin(async move {
            let (state, id) = match fact {
                SubmissionFact::Accepted(id) => ("accepted", Some(id)),
                SubmissionFact::Rejected(()) => ("rejected", None),
                SubmissionFact::Unknown => ("unknown", None),
            };
            if self.store.record_submission(task, state, id.as_deref())? {
                Ok(ApplyResult::Applied(
                    self.outcome(self.store.get(&task.id)?),
                ))
            } else {
                Ok(ApplyResult::Conflict)
            }
        })
    }
    fn reload<'a>(&'a mut self, task: &'a TaskRow) -> HostFuture<'a, Outcome, String> {
        Box::pin(async move { Ok(self.outcome(self.store.get(&task.id)?)) })
    }
}
impl core::ObservationEffects for Host {
    type Key = str;
    type Task = TaskRow;
    type Raw = TaskObservationV2;
    type Observation = Mapped;
    type Outcome = Outcome;
    type Error = String;
    fn load<'a>(
        &'a mut self,
        key: &'a str,
    ) -> HostFuture<'a, ObservationPlan<TaskRow, Outcome>, String> {
        Box::pin(async move {
            self.fresh = None;
            let row = self.store.get(key)?;
            Ok(
                if row.upstream_id.is_some()
                    && !matches!(row.execution_state.as_str(), "failed" | "cancelled")
                {
                    if terminal(&row.execution_state) {
                        ObservationPlan::Query(row)
                    } else if let Some(claimed) = self.store.acquire_observation(&row)? {
                        ObservationPlan::Query(claimed)
                    } else {
                        ObservationPlan::Return(self.outcome(self.store.get(key)?))
                    }
                } else {
                    ObservationPlan::Return(self.outcome(row))
                },
            )
        })
    }
    fn query<'a>(&'a mut self, task: &'a TaskRow) -> HostFuture<'a, TaskObservationV2, String> {
        Box::pin(async move {
            let (executor, secret) = self.restored(task)?;
            executor
                .observe(
                    &task.binding,
                    task.upstream_id
                        .as_deref()
                        .ok_or("task upstream identity missing")?,
                    &secret,
                )
                .await
        })
    }
    fn normalize(
        &mut self,
        _task: &TaskRow,
        raw: Result<TaskObservationV2, String>,
    ) -> Result<Mapped, String> {
        let observation = raw.unwrap_or_else(|_| TaskObservationV2::Unknown {
            reason: "original task execution is unavailable".into(),
        });
        let state = match &observation {
            TaskObservationV2::Progress { running, .. } => {
                if *running {
                    "running"
                } else {
                    "queued"
                }
            }
            TaskObservationV2::Succeeded { .. } => "succeeded",
            TaskObservationV2::Failed {
                kind: TaskFailureKindV1::Cancelled,
                ..
            } => "cancelled",
            TaskObservationV2::Failed { .. } => "failed",
            TaskObservationV2::Unknown { .. } => "status_unknown",
        };
        Ok(Mapped {
            state,
            observation: Some(observation),
        })
    }
    fn apply<'a>(
        &'a mut self,
        task: &'a TaskRow,
        observation: Mapped,
    ) -> HostFuture<'a, ApplyResult<Outcome>, String> {
        Box::pin(async move {
            // A terminal refresh is authorized by the state read before querying. It never
            // updates task facts or emits another terminal event.
            if task.execution_state == "succeeded" {
                let current = self.store.get(&task.id)?;
                if current.version == task.version && current.execution_state == "succeeded" {
                    self.fresh = observation.observation;
                    return Ok(ApplyResult::Applied(self.outcome(current)));
                }
                self.fresh = None;
                return Ok(ApplyResult::Conflict);
            }
            if self.store.apply(task, observation.state)? {
                self.fresh = observation.observation;
                Ok(ApplyResult::Applied(
                    self.outcome(self.store.get(&task.id)?),
                ))
            } else {
                // A losing pending observation cannot borrow the winner's successful state
                // to expose its own stale artifact or usage facts.
                self.fresh = None;
                Ok(ApplyResult::Conflict)
            }
        })
    }
    fn reload<'a>(&'a mut self, task: &'a TaskRow) -> HostFuture<'a, Outcome, String> {
        Box::pin(async move { Ok(self.outcome(self.store.get(&task.id)?)) })
    }
}
impl core::CancelEffects for Host {
    type Key = str;
    type Task = TaskRow;
    type Outcome = Outcome;
    type Error = String;
    fn load_for_cancel<'a>(
        &'a mut self,
        key: &'a str,
    ) -> HostFuture<'a, CancelPlan<TaskRow, Outcome>, String> {
        Box::pin(async move {
            let row = self.store.get(key)?;
            Ok(if terminal(&row.execution_state) {
                CancelPlan::Return(self.outcome(row))
            } else if row.submission_state == "prepared" {
                CancelPlan::Prepared(row)
            } else {
                CancelPlan::Dispatched(row)
            })
        })
    }
    fn cancel_prepared<'a>(
        &'a mut self,
        task: &'a TaskRow,
    ) -> HostFuture<'a, ApplyResult<Outcome>, String> {
        Box::pin(async move {
            if self.store.cancel_prepared(task)? {
                Ok(ApplyResult::Applied(
                    self.outcome(self.store.get(&task.id)?),
                ))
            } else {
                Ok(ApplyResult::Conflict)
            }
        })
    }
    fn record_cancel_intent<'a>(
        &'a mut self,
        task: &'a TaskRow,
    ) -> HostFuture<'a, Outcome, String> {
        Box::pin(async move {
            self.store.cancel_intent(&task.id)?;
            Ok(self.outcome(self.store.get(&task.id)?))
        })
    }
}
impl core::WaitEffects for Host {
    type Key = str;
    type Outcome = Outcome;
    type Error = String;
    fn inspect<'a>(&'a mut self, key: &'a str) -> HostFuture<'a, WaitState<Outcome>, String> {
        Box::pin(async move {
            let outcome = core::observe(self, key).await?;
            Ok(if terminal(&outcome.execution_state) {
                WaitState::Ready(outcome)
            } else {
                WaitState::Pending
            })
        })
    }
}
struct Clock(Instant);
impl core::WaitControl for Clock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&self, duration: Duration) -> core::ControlFuture<'_> {
        Box::pin(tokio::time::sleep(duration))
    }
    fn cancelled(&self) -> core::ControlFuture<'_> {
        Box::pin(async {
            let _ = tokio::signal::ctrl_c().await;
        })
    }
}

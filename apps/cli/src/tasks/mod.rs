//! Local generation task effects; shared workflow ordering lives in south-task-core.
mod executor;
mod store;
mod workflow;

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use store::TaskStore;

#[derive(Args)]
pub struct TaskArgs {
    #[arg(long)]
    pub task_config: PathBuf,
    #[command(subcommand)]
    pub command: TaskCommand,
}
#[derive(Subcommand)]
pub enum TaskCommand {
    /// Submit once and return a durable local task identity.
    Submit {
        #[arg(long)]
        provider: String,
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        idempotency_key: String,
    },
    /// Read metadata without contacting a provider.
    Inspect { id: String },
    /// Query the original provider once.
    Observe { id: String },
    /// Wait within a bounded time budget; expiration does not cancel the task.
    Wait {
        id: String,
        #[arg(long, default_value_t = 30)]
        timeout_seconds: u64,
    },
    /// Record cancellation intent; does not promise upstream cancellation.
    Cancel { id: String },
    /// Download a successful task artifact without forwarding provider credentials.
    Fetch {
        id: String,
        #[arg(long)]
        output: PathBuf,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ComponentPin {
    pub world: String,
    pub name: String,
    pub version: String,
    pub manifest_sha256: String,
    pub wasm_sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialRef {
    pub upstream: String,
    pub slot: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub endpoint: String,
    pub model: String,
    pub dialect: String,
    pub pin: ComponentPin,
    pub credential: CredentialRef,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskConfig {
    pub state_dir: PathBuf,
    pub components_dir: PathBuf,
    pub providers: BTreeMap<String, Provider>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub schema_version: u16,
    pub provider: Provider,
    pub locator: Value,
    pub credential_mac: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskRow {
    pub id: String,
    pub request_hash: String,
    pub submission_state: String,
    pub execution_state: String,
    pub binding: Binding,
    pub upstream_id: Option<String>,
    pub cancel_requested: bool,
    pub version: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Outcome {
    pub id: String,
    pub submission_state: String,
    pub execution_state: String,
    pub cancel_requested: bool,
    pub billing_effect: &'static str,
    pub version: u64,
    #[serde(skip)]
    pub observation: Option<south_contracts::TaskObservationV2>,
}
impl From<TaskRow> for Outcome {
    fn from(row: TaskRow) -> Self {
        Self {
            id: row.id,
            submission_state: row.submission_state,
            execution_state: row.execution_state,
            cancel_requested: row.cancel_requested,
            billing_effect: "not_applicable",
            version: row.version,
            observation: None,
        }
    }
}
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
pub(crate) fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
pub(crate) fn random_id() -> Result<String, String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| "task entropy unavailable".to_owned())?;
    Ok(hex(&bytes))
}
pub(crate) fn terminal(state: &str) -> bool {
    matches!(state, "succeeded" | "failed" | "cancelled")
}
pub(crate) fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| format!("cannot read input: {e}"))?;
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > max {
        return Err("input exceeds size limit".into());
    }
    Ok(bytes)
}

/// Executes a task command without constructing the text gateway or its plugin registry.
/// # Errors
/// Returns a credential-free diagnostic for invalid configuration or an unavailable effect.
pub fn run(config_path: &Path, args: TaskArgs) -> Result<(), String> {
    let config = crate::config::ClientConfig::load(config_path).map_err(|e| e.to_string())?;
    let tasks: TaskConfig = serde_json::from_slice(&read_bounded(&args.task_config, 1024 * 1024)?)
        .map_err(|e| format!("invalid task config: {e}"))?;
    let mut host = workflow::Host::new(tasks, config.data.dir)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let output = runtime.block_on(host.command(args.command))?;
    println!(
        "{}",
        serde_json::to_string(&output).map_err(|e| e.to_string())?
    );
    Ok(())
}

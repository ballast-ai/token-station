//! Optional local classifier orchestration. The gateway owns precedence; this
//! module owns a bounded, content-free observation stream and one offline child.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak, mpsc};
use std::time::{Duration, Instant};

use crate::request_context::RequestContext;
use serde::{Deserialize, Serialize};
use token_station_metrics::{
    ClassifierInputDiagnostic, ClassifierInputHandling, ClassifierInputReason, ClassifierKind,
};
use token_station_protocol::{ChatRequest, Content, ContentPart, Role};

const MAX_TEXT_BYTES: usize = 16 * 1024;
const TIMEOUT: Duration = Duration::from_millis(400);
const HARD_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Off,
    Observe,
    Route,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Low,
    Medium,
    High,
}
impl Tier {
    #[must_use]
    pub const fn pool(self) -> &'static str {
        match self {
            Self::Low => "tier_low",
            Self::Medium => "tier_mid",
            Self::High => "tier_high",
        }
    }
    #[must_use]
    pub fn from_pool(pool: &str) -> Option<Self> {
        match pool {
            "tier_low" => Some(Self::Low),
            "tier_mid" => Some(Self::Medium),
            "tier_high" => Some(Self::High),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Observed,
    Applied,
    Overridden,
    Timeout,
    Busy,
    Loading,
    Unavailable,
    Unsupported,
    Invalid,
    Cancelled,
    NoRoute,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct Observation {
    pub id: u64,
    pub mode: Mode,
    pub baseline_tier: Option<Tier>,
    pub suggested_tier: Option<Tier>,
    pub applied: bool,
    pub latency_ms: Option<u64>,
    pub outcome: Outcome,
}
#[derive(Debug, Default, Clone, Serialize)]
pub struct Counts {
    pub classified: u64,
    pub disagreements: u64,
    pub fallbacks: u64,
}
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub available: bool,
    pub enabled: bool,
    pub mode: Mode,
    pub state: String,
    pub error: Option<String>,
    pub model_ready: bool,
    pub timeout_ms: u64,
    pub observations: Vec<Observation>,
    pub counts: Counts,
}
impl Status {
    #[must_use]
    pub fn unavailable() -> Self {
        Self {
            available: false,
            enabled: false,
            mode: Mode::Off,
            state: "off".into(),
            error: None,
            model_ready: false,
            timeout_ms: 400,
            observations: Vec::new(),
            counts: Counts::default(),
        }
    }
}

struct Process {
    child: Mutex<Option<Child>>,
    stopped: AtomicBool,
    busy: AtomicBool,
}
impl Process {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            child: Mutex::new(None),
            stopped: AtomicBool::new(false),
            busy: AtomicBool::new(false),
        })
    }
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(mut child) = self.child.lock().expect("semantic child lock").take() {
            stop_child(&mut child);
        }
    }
}
fn stop_child(child: &mut Child) {
    #[cfg(unix)]
    let _ = Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{}", child.id())])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

struct Worker {
    process: Arc<Process>,
    sender: mpsc::SyncSender<Job>,
}
struct Inner {
    enabled: bool,
    mode: Mode,
    state: &'static str,
    error: Option<String>,
    generation: u64,
    worker: Option<Worker>,
    preparation: Option<Arc<Process>>,
    preparation_mode: Mode,
    observations: VecDeque<Observation>,
    counts: Counts,
}
pub struct SemanticController {
    root: PathBuf,
    settings_path: PathBuf,
    inner: Mutex<Inner>,
    ids: AtomicU64,
    #[cfg(test)]
    preparation_script: Mutex<Option<PathBuf>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    enabled: bool,
}

/// A result retained only until the gateway applies or rejects the suggested pool.
pub struct Suggestion {
    generation: u64,
    observation: Observation,
    pub tier: Tier,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prediction {
    Tier(Tier),
    TokenLimit,
}

struct Job {
    generation: u64,
    observation: Observation,
    text: String,
    reply: Option<mpsc::SyncSender<Result<Prediction, Outcome>>>,
    started: Instant,
}

impl SemanticController {
    /// Shares one classifier across desktop gateway replacements.
    ///
    /// # Panics
    /// Panics if the process-wide registry lock is poisoned.
    #[must_use]
    pub fn shared(data_dir: &Path) -> Arc<Self> {
        static REGISTRY: OnceLock<Mutex<BTreeMap<PathBuf, Weak<SemanticController>>>> =
            OnceLock::new();
        let mut registry = REGISTRY
            .get_or_init(Mutex::default)
            .lock()
            .expect("semantic registry lock");
        if let Some(existing) = registry.get(data_dir).and_then(Weak::upgrade) {
            return existing;
        }
        let controller = Arc::new(Self {
            root: data_dir.join("semantic-runtime"),
            settings_path: data_dir.join("semantic-settings.json"),
            inner: Mutex::new(Inner {
                enabled: false,
                mode: Mode::Off,
                state: "off",
                error: None,
                generation: 0,
                worker: None,
                preparation: None,
                preparation_mode: Mode::Off,
                observations: VecDeque::new(),
                counts: Counts::default(),
            }),
            ids: AtomicU64::new(1),
            #[cfg(test)]
            preparation_script: Mutex::new(None),
        });
        registry.retain(|_, value| value.strong_count() > 0);
        registry.insert(data_dir.to_path_buf(), Arc::downgrade(&controller));
        // Shared controllers stay Off until the host selects a startup policy.
        controller
    }

    fn model_ready(&self) -> bool {
        [
            ".venv/bin/python",
            "worker.py",
            "models/scx/model.safetensors",
            "models/scx/config.json",
            "models/scx/tokenizer.json",
            "models/scx/tokenizer_config.json",
            "models/scx/chat_template.jinja",
            "models/scx/README.md",
            "assets.json",
            "prepared.json",
        ]
        .iter()
        .all(|path| self.root.join(path).is_file())
    }
    /// Content-free UI snapshot.
    ///
    /// # Panics
    /// Panics if the state lock is poisoned.
    #[must_use]
    pub fn status(&self) -> Status {
        let inner = self.inner.lock().expect("semantic state lock");
        let ready = self.model_ready();
        Status {
            available: true,
            enabled: inner.enabled,
            mode: inner.mode,
            state: if inner.state == "off" && !ready {
                "unprepared"
            } else {
                inner.state
            }
            .into(),
            error: inner.error.clone(),
            model_ready: ready,
            timeout_ms: 400,
            observations: inner.observations.iter().cloned().collect(),
            counts: inner.counts.clone(),
        }
    }

    /// Applies the saved startup preference once without replacing later choices.
    /// Missing assets are prepared in the background before the worker starts.
    /// The desktop host owns platform support and startup admission.
    ///
    /// # Errors
    /// Background preparation and worker failures are reported in status.
    /// # Panics
    /// Panics if the state lock is poisoned.
    pub fn start_automatic_route(self: &Arc<Self>) -> Result<Status, String> {
        let mut inner = self.inner.lock().expect("semantic state lock");
        if inner.generation == 0 {
            match self.read_enabled() {
                Ok(enabled) => self.apply_enabled(&mut inner, enabled),
                Err(error) => {
                    inner.generation += 1;
                    inner.state = "error";
                    inner.error = Some(error);
                }
            }
        }
        drop(inner);
        Ok(self.status())
    }

    fn read_enabled(&self) -> Result<bool, String> {
        const READ_ERROR: &str = "Could not read local SCX settings. Classification remains off. Save a new switch setting to retry.";
        const INVALID: &str = "Local SCX settings are invalid. Classification remains off. Save a new switch setting to retry.";
        let metadata = match std::fs::symlink_metadata(&self.settings_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err(READ_ERROR.into()),
        };
        if !metadata.is_file() || metadata.len() > 1024 {
            return Err(INVALID.into());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&self.settings_path)
            .map_err(|_| READ_ERROR.to_owned())?
            .take(1025)
            .read_to_end(&mut bytes)
            .map_err(|_| READ_ERROR.to_owned())?;
        serde_json::from_slice::<Settings>(&bytes)
            .map(|settings| settings.enabled)
            .map_err(|_| INVALID.to_owned())
    }

    /// Saves user intent before changing the runtime. Shutdown uses `set_mode`
    /// instead, so releasing model memory cannot overwrite the saved choice.
    ///
    /// # Errors
    /// Returns a static diagnostic if the private preference cannot be saved.
    /// # Panics
    /// Panics if the state lock is poisoned.
    pub fn set_enabled(self: &Arc<Self>, enabled: bool) -> Result<Status, String> {
        let mut inner = self.inner.lock().expect("semantic state lock");
        let bytes: &[u8] = if enabled {
            b"{\"enabled\":true}\n"
        } else {
            b"{\"enabled\":false}\n"
        };
        crate::private_fs::write_atomic_private(&self.settings_path, bytes).map_err(|_| {
            "Could not save local SCX settings. The previous choice remains active.".to_owned()
        })?;
        self.apply_enabled(&mut inner, enabled);
        drop(inner);
        Ok(self.status())
    }

    fn apply_enabled(self: &Arc<Self>, inner: &mut Inner, enabled: bool) {
        inner.enabled = enabled;
        if !enabled {
            self.change_mode(inner, Mode::Off);
        } else if inner.state == "preparing" {
            inner.preparation_mode = Mode::Route;
        } else if inner.mode == Mode::Route && matches!(inner.state, "ready" | "loading") {
            // Repeated On preserves the current worker and its pending requests.
        } else if self.model_ready() {
            self.change_mode(inner, Mode::Route);
        } else {
            self.start_preparation(inner, Mode::Route);
        }
    }

    /// Changes the process lifetime without restarting the gateway.
    ///
    /// # Errors
    /// Returns a static diagnostic if model files are not prepared.
    /// # Panics
    /// Panics if the state lock is poisoned.
    pub fn set_mode(self: &Arc<Self>, mode: Mode) -> Result<Status, String> {
        if mode != Mode::Off && !self.model_ready() {
            return Err("Prepare the local SCX model first.".into());
        }
        let mut inner = self.inner.lock().expect("semantic state lock");
        if inner.mode == mode && matches!(inner.state, "ready" | "loading" | "off") {
            drop(inner);
            return Ok(self.status());
        }
        if inner.state == "preparing" && mode != Mode::Off {
            return Err("Wait for preparation to finish.".into());
        }
        self.change_mode(&mut inner, mode);
        drop(inner);
        Ok(self.status())
    }

    fn change_mode(self: &Arc<Self>, inner: &mut Inner, mode: Mode) {
        inner.generation += 1;
        if let Some(preparation) = inner.preparation.take() {
            preparation.stop();
        }
        inner.mode = mode;
        inner.error = None;
        let previous = inner.worker.take();
        if let Some(worker) = previous {
            worker.process.stop();
        }
        if mode == Mode::Off {
            inner.state = "off";
        } else {
            self.start_worker(inner);
        }
    }

    fn start_worker(self: &Arc<Self>, inner: &mut Inner) {
        let generation = inner.generation;
        let (sender, receiver) = mpsc::sync_channel(1);
        let process = Process::new();
        inner.worker = Some(Worker {
            process: Arc::clone(&process),
            sender,
        });
        inner.state = "loading";
        let root = self.root.clone();
        let controller = Arc::downgrade(self);
        std::thread::spawn(move || run_worker(&controller, &root, generation, &process, &receiver));
    }

    /// Installs the pinned runtime in a background process; this is the only
    /// classifier operation allowed to download files.
    ///
    /// # Errors
    /// Returns a static error if the private runtime directory cannot be written.
    /// # Panics
    /// Panics if the state lock is poisoned.
    pub fn prepare(self: &Arc<Self>) -> Result<Status, String> {
        let mut inner = self.inner.lock().expect("semantic state lock");
        if inner.state == "preparing" {
            drop(inner);
            return Ok(self.status());
        }
        self.start_preparation(&mut inner, Mode::Off);
        drop(inner);
        Ok(self.status())
    }

    fn start_preparation(self: &Arc<Self>, inner: &mut Inner, mode_after: Mode) {
        inner.generation += 1;
        let generation = inner.generation;
        if let Some(worker) = inner.worker.take() {
            worker.process.stop();
        }
        inner.mode = Mode::Off;
        inner.state = "preparing";
        inner.preparation_mode = mode_after;
        inner.error = None;
        let process = Process::new();
        inner.preparation = Some(Arc::clone(&process));
        let root = self.root.clone();
        let controller = Arc::downgrade(self);
        #[cfg(test)]
        let preparation_script = self.preparation_script.lock().unwrap().clone();
        std::thread::spawn(move || {
            #[cfg(not(test))]
            let result = prepare_runtime(&root, &process);
            #[cfg(test)]
            let result = if let Some(script) = preparation_script {
                let mut command = Command::new("/usr/bin/python3");
                command.arg(script).arg(&root);
                run_preparation_command(command, &process)
            } else {
                prepare_runtime(&root, &process)
            };
            process.stop();
            if let Some(controller) = controller.upgrade() {
                let mut inner = controller.inner.lock().expect("semantic state lock");
                if inner.generation == generation {
                    inner.preparation = None;
                    if result.is_ok() && controller.model_ready() {
                        let mode_after = inner.preparation_mode;
                        inner.mode = mode_after;
                        if mode_after == Mode::Off {
                            inner.state = "off";
                        } else {
                            // Keep the generation check and startup under one lock.
                            // Off must not race with a delayed preparation result.
                            controller.start_worker(&mut inner);
                        }
                    } else {
                        inner.state = "error";
                        inner.error = Some("SCX preparation failed. Install uv and verify network access, then retry. Existing routes remain available.".into());
                    }
                }
            }
        });
    }

    fn record(&self, generation: u64, observation: Observation) {
        let mut inner = self.inner.lock().expect("semantic state lock");
        if generation != inner.generation || inner.mode == Mode::Off {
            return;
        }
        if observation.suggested_tier.is_some() {
            inner.counts.classified += 1;
            if observation.baseline_tier.is_some()
                && observation.baseline_tier != observation.suggested_tier
            {
                inner.counts.disagreements += 1;
            }
        }
        if !matches!(
            observation.outcome,
            Outcome::Applied | Outcome::Observed | Outcome::Overridden
        ) {
            inner.counts.fallbacks += 1;
        }
        if inner.observations.len() == 64 {
            inner.observations.pop_front();
        }
        inner.observations.push_back(observation);
    }

    /// Records a bypass without projecting or retaining request content.
    ///
    /// # Panics
    /// Panics if the state lock is poisoned.
    pub fn bypass(&self, baseline: Option<Tier>, outcome: Outcome) {
        let inner = self.inner.lock().expect("semantic state lock");
        if inner.mode == Mode::Off {
            return;
        }
        let generation = inner.generation;
        let observation = self.observation(inner.mode, baseline, outcome);
        drop(inner);
        self.record(generation, observation);
    }
    fn observation(&self, mode: Mode, baseline: Option<Tier>, outcome: Outcome) -> Observation {
        Observation {
            id: self.ids.fetch_add(1, Ordering::Relaxed),
            mode,
            baseline_tier: baseline,
            suggested_tier: None,
            applied: false,
            latency_ms: None,
            outcome,
        }
    }

    /// Observe never waits; Route waits at most 400 ms and respects cancellation.
    ///
    /// # Panics
    /// Panics if the state lock is poisoned.
    #[must_use]
    pub fn classify(
        &self,
        request: &ChatRequest,
        baseline: Option<Tier>,
        ctx: &RequestContext,
    ) -> Option<Suggestion> {
        let inner = self.inner.lock().expect("semantic state lock");
        if inner.mode == Mode::Off {
            return None;
        }
        let mode = inner.mode;
        let generation = inner.generation;
        let mut observation = self.observation(mode, baseline, Outcome::Unavailable);
        let worker = inner.worker.as_ref();
        let early = if inner.state == "loading" {
            Some(Outcome::Loading)
        } else if inner.state != "ready" || worker.is_none() {
            Some(Outcome::Unavailable)
        } else {
            None
        };
        if let Some(outcome) = early {
            observation.outcome = outcome;
            drop(inner);
            self.record(generation, observation);
            return None;
        }
        let worker = worker.expect("ready worker");
        let text = match project(request, ctx) {
            Ok(text) => text,
            Err(outcome) => {
                observation.outcome = outcome;
                drop(inner);
                self.record(generation, observation);
                return None;
            }
        };
        if worker.process.busy.swap(true, Ordering::AcqRel) {
            observation.outcome = Outcome::Busy;
            drop(inner);
            self.record(generation, observation);
            return None;
        }
        let started = Instant::now();
        let (sender, receiver) = mpsc::sync_channel(1);
        let job = Job {
            generation,
            observation: observation.clone(),
            text,
            reply: (mode == Mode::Route).then_some(sender),
            started,
        };
        if worker.sender.try_send(job).is_err() {
            worker.process.busy.store(false, Ordering::Release);
            drop(inner);
            self.record(generation, observation);
            return None;
        }
        drop(inner);
        if mode == Mode::Observe {
            return None;
        }
        let result = loop {
            if ctx.is_cancelled() {
                break Err(Outcome::Cancelled);
            }
            if started.elapsed() >= TIMEOUT || ctx.remaining().is_zero() {
                break Err(Outcome::Timeout);
            }
            if self.inner.lock().expect("semantic state lock").generation != generation {
                break Err(Outcome::Cancelled);
            }
            match receiver.recv_timeout(Duration::from_millis(5)) {
                Ok(result) => break result,
                Err(mpsc::RecvTimeoutError::Disconnected) => break Err(Outcome::Error),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        };
        observation.latency_ms = Some(elapsed_ms(started));
        match result {
            Ok(_) if started.elapsed() > TIMEOUT => {
                observation.outcome = Outcome::Timeout;
                self.record(generation, observation);
                None
            }
            Ok(Prediction::TokenLimit) => {
                record_input(ctx, Some(ClassifierInputReason::TokenLimit));
                observation.outcome = Outcome::Unsupported;
                self.record(generation, observation);
                None
            }
            Ok(Prediction::Tier(tier)) => {
                record_input(ctx, None);
                observation.suggested_tier = Some(tier);
                Some(Suggestion {
                    generation,
                    observation,
                    tier,
                })
            }
            Err(outcome) => {
                observation.outcome = outcome;
                self.record(generation, observation);
                None
            }
        }
    }

    /// Commits an observation only after the normal capability/recovery checks.
    ///
    /// # Panics
    /// Panics if the state lock is poisoned.
    pub fn finish(&self, mut suggestion: Suggestion, applied: bool) {
        suggestion.observation.applied = applied;
        suggestion.observation.outcome = if applied {
            Outcome::Applied
        } else {
            Outcome::NoRoute
        };
        self.record(suggestion.generation, suggestion.observation);
    }
    /// Whether a suggestion still belongs to the active mode.
    ///
    /// # Panics
    /// Panics if the state lock is poisoned.
    #[must_use]
    pub fn is_current(&self, suggestion: &Suggestion) -> bool {
        let inner = self.inner.lock().expect("semantic state lock");
        inner.mode == Mode::Route && inner.generation == suggestion.generation
    }
}
impl Drop for SemanticController {
    fn drop(&mut self) {
        if let Ok(inner) = self.inner.get_mut() {
            if let Some(worker) = inner.worker.take() {
                worker.process.stop();
            }
            if let Some(preparation) = inner.preparation.take() {
                preparation.stop();
            }
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
fn set_worker_state(controller: &Weak<SemanticController>, generation: u64, state: &'static str) {
    if let Some(controller) = controller.upgrade() {
        let mut inner = controller.inner.lock().expect("semantic state lock");
        if inner.generation == generation {
            inner.state = state;
            inner.error = (state == "error").then(|| {
                "Local SCX worker stopped. Turn Off and retry, or prepare the model again.".into()
            });
        }
    }
}
fn run_worker(
    controller: &Weak<SemanticController>,
    root: &Path,
    generation: u64,
    process: &Process,
    receiver: &mpsc::Receiver<Job>,
) {
    let result = worker_loop(controller, root, generation, process, receiver);
    process.stop();
    if result.is_err() {
        set_worker_state(controller, generation, "error");
    }
}
fn worker_loop(
    controller: &Weak<SemanticController>,
    root: &Path,
    generation: u64,
    process: &Process,
    receiver: &mpsc::Receiver<Job>,
) -> Result<(), ()> {
    // Prepared environments survive App updates. Refresh only the managed
    // protocol script before launch, without invoking setup or downloads.
    crate::private_fs::write_atomic_private(
        &root.join("worker.py"),
        include_bytes!("../../../scripts/scx-runtime/worker.py"),
    )
    .map_err(|_| ())?;
    let mut command = Command::new(root.join(".venv/bin/python"));
    command
        .arg("-u")
        .arg(root.join("worker.py"))
        .arg("--model")
        .arg(root.join("models/scx"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .env("TOKENIZERS_PARALLELISM", "false")
        .env("PYTHONNOUSERSITE", "1")
        .env("OMP_NUM_THREADS", "2")
        .env("MKL_NUM_THREADS", "2")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // HOME is required by Python/Metal, but credentials and arbitrary environment
    // hooks are intentionally not inherited by this offline process.
    if let Some(home) = std::env::var_os("HOME") {
        command.env("HOME", home);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|_| ())?;
    let stdin = child.stdin.take().ok_or(())?;
    let stdout = child.stdout.take().ok_or(())?;
    {
        let mut slot = process.child.lock().expect("semantic child lock");
        if process.stopped.load(Ordering::Acquire) {
            stop_child(&mut child);
            return Ok(());
        }
        *slot = Some(child);
    }
    let input = write_worker_input(stdin);
    let output = read_worker_lines(stdout);
    let ready = output
        .recv_timeout(Duration::from_secs(90))
        .map_err(|_| ())?;
    if serde_json::from_str::<serde_json::Value>(&ready)
        .ok()
        .and_then(|v| v.get("event").cloned())
        != Some(serde_json::json!("ready"))
    {
        return Err(());
    }
    set_worker_state(controller, generation, "ready");
    while let Ok(mut job) = receiver.recv() {
        if process.stopped.load(Ordering::Acquire) {
            break;
        }
        let wire = serde_json::json!({"id":job.observation.id,"text":job.text});
        let deadline = Instant::now() + HARD_TIMEOUT;
        let (written, acknowledgement) = mpsc::sync_channel(1);
        let result = input
            .try_send((wire, written))
            .map_err(|_| Outcome::Error)
            .and_then(|()| {
                acknowledgement
                    .recv_timeout(HARD_TIMEOUT)
                    .map_err(|_| Outcome::Timeout)
            })
            .and_then(std::convert::identity)
            .and_then(|()| {
                output
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .map_err(|_| Outcome::Timeout)
            })
            .and_then(|prediction| parse_prediction(&prediction, job.observation.id));
        job.text.clear();
        let fatal = matches!(result, Err(Outcome::Timeout | Outcome::Error));
        process.busy.store(false, Ordering::Release);
        if let Some(reply) = job.reply {
            let _ = reply.send(result);
        } else if let Some(controller) = controller.upgrade() {
            job.observation.latency_ms = Some(elapsed_ms(job.started));
            job.observation.outcome = match result {
                Ok(Prediction::Tier(tier)) if job.started.elapsed() <= TIMEOUT => {
                    job.observation.suggested_tier = Some(tier);
                    Outcome::Observed
                }
                Ok(Prediction::TokenLimit) => Outcome::Unsupported,
                Ok(Prediction::Tier(_)) => Outcome::Timeout,
                Err(outcome) => outcome,
            };
            controller.record(job.generation, job.observation);
        }
        if fatal {
            return Err(());
        }
    }
    Ok(())
}
type WorkerInput = (serde_json::Value, mpsc::SyncSender<Result<(), Outcome>>);

fn write_worker_input(mut stdin: std::process::ChildStdin) -> mpsc::SyncSender<WorkerInput> {
    let (input, receiver) = mpsc::sync_channel::<WorkerInput>(1);
    std::thread::spawn(move || {
        while let Ok((wire, acknowledgement)) = receiver.recv() {
            let result = serde_json::to_writer(&mut stdin, &wire)
                .map_err(|_| Outcome::Error)
                .and_then(|()| {
                    stdin
                        .write_all(b"\n")
                        .and_then(|()| stdin.flush())
                        .map_err(|_| Outcome::Error)
                });
            let failed = result.is_err();
            let _ = acknowledgement.send(result);
            if failed {
                break;
            }
        }
    });
    input
}

fn read_worker_lines(stdout: std::process::ChildStdout) -> mpsc::Receiver<String> {
    let (lines, output) = mpsc::sync_channel(2);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            if reader.by_ref().take(4097).read_line(&mut line).is_err()
                || line.is_empty()
                || line.len() > 4096
            {
                break;
            }
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    output
}

fn parse_prediction(line: &str, id: u64) -> Result<Prediction, Outcome> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(|_| Outcome::Invalid)?;
    if value.get("id").and_then(serde_json::Value::as_u64) != Some(id) {
        return Err(Outcome::Invalid);
    }
    match value.get("status").and_then(serde_json::Value::as_str) {
        Some("unsupported")
            if value.get("reason").and_then(serde_json::Value::as_str) == Some("token_limit") =>
        {
            Ok(Prediction::TokenLimit)
        }
        Some("unsupported") => Err(Outcome::Unsupported),
        Some("ok") => serde_json::from_value(value.get("tier").cloned().unwrap_or_default())
            .map(Prediction::Tier)
            .map_err(|_| Outcome::Invalid),
        _ => Err(Outcome::Error),
    }
}
fn record_input(context: &RequestContext, reason: Option<ClassifierInputReason>) {
    context.set_classifier_input(ClassifierInputDiagnostic {
        classifier: ClassifierKind::Scx,
        handling: if reason.is_some() {
            ClassifierInputHandling::Skipped
        } else {
            ClassifierInputHandling::Full
        },
        reason,
    });
}

fn project(request: &ChatRequest, context: &RequestContext) -> Result<String, Outcome> {
    // Validate all visible turns before assigning a length reason. In particular,
    // an empty latest user or hidden media is not a size-limit rejection.
    let mut latest_user_nonempty = false;
    for message in &request.messages {
        if !matches!(message.role, Role::User | Role::Assistant) {
            continue;
        }
        let nonempty = match &message.content {
            None => false,
            Some(Content::Text(text)) => !text.trim().is_empty(),
            Some(Content::Parts(parts)) => {
                let mut nonempty = false;
                for part in parts {
                    match part {
                        ContentPart::Text { text } => nonempty |= !text.trim().is_empty(),
                        ContentPart::Thinking { .. } | ContentPart::RedactedThinking { .. } => {}
                        _ => return Err(Outcome::Unsupported),
                    }
                }
                nonempty
            }
        };
        if message.role == Role::User {
            latest_user_nonempty = nonempty;
        }
    }
    if !latest_user_nonempty {
        return Err(Outcome::Unsupported);
    }

    let mut turns = Vec::new();
    let mut bytes = 0;
    for message in &request.messages {
        if !matches!(message.role, Role::User | Role::Assistant) {
            continue;
        }
        let text = match &message.content {
            None => String::new(),
            Some(Content::Text(text)) => text.clone(),
            Some(Content::Parts(parts)) => {
                let mut text = String::new();
                for part in parts {
                    match part {
                        ContentPart::Text { text: part } => {
                            text.push_str(part);
                            text.push('\n');
                        }
                        ContentPart::Thinking { .. } | ContentPart::RedactedThinking { .. } => {}
                        _ => return Err(Outcome::Unsupported),
                    }
                }
                text
            }
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        bytes += text.len() + 12;
        if bytes > MAX_TEXT_BYTES {
            record_input(context, Some(ClassifierInputReason::ByteLimit));
            return Err(Outcome::Unsupported);
        }
        turns.push(format!(
            "{}: {text}",
            if message.role == Role::User {
                "User"
            } else {
                "Assistant"
            }
        ));
    }
    Ok(turns.join("\n\n"))
}

#[cfg(test)]
mod tests;

fn prepare_runtime(root: &Path, process: &Process) -> Result<(), ()> {
    crate::private_fs::ensure_private_dir(root).map_err(|_| ())?;
    for (name, content) in [
        (
            "setup.py",
            include_str!("../../../scripts/scx-runtime/setup.py"),
        ),
        (
            "worker.py",
            include_str!("../../../scripts/scx-runtime/worker.py"),
        ),
        (
            "assets.json",
            include_str!("../../../scripts/scx-runtime/assets.json"),
        ),
        (
            "requirements-lock.txt",
            include_str!("../../../scripts/scx-runtime/requirements-lock.txt"),
        ),
    ] {
        crate::private_fs::write_atomic_private(&root.join(name), content.as_bytes())
            .map_err(|_| ())?;
    }
    let mut command = Command::new(if cfg!(target_os = "macos") {
        "/usr/bin/python3"
    } else {
        "python3"
    });
    command
        .arg(root.join("setup.py"))
        .arg("--runtime-dir")
        .arg(root);
    run_preparation_command(command, process)
}

fn run_preparation_command(mut command: Command, process: &Process) -> Result<(), ()> {
    command
        .env_remove("PYTHONPATH")
        .env_remove("PYTHONHOME")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|_| ())?;
    {
        let mut slot = process.child.lock().expect("semantic child lock");
        if process.stopped.load(Ordering::Acquire) {
            stop_child(&mut child);
            return Err(());
        }
        *slot = Some(child);
    }
    let started = Instant::now();
    loop {
        if process.stopped.load(Ordering::Acquire) || started.elapsed() > Duration::from_hours(1) {
            return Err(());
        }
        let mut slot = process.child.lock().expect("semantic child lock");
        let child = slot.as_mut().ok_or(())?;
        if let Some(status) = child.try_wait().map_err(|_| ())? {
            return if status.success() { Ok(()) } else { Err(()) };
        }
        drop(slot);
        std::thread::sleep(Duration::from_millis(100));
    }
}

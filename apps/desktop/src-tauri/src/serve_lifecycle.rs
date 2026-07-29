//! Heavy proxy preparation and supervised listener ownership.

use std::net::{SocketAddr, TcpListener as StdTcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use token_station_cli::config::ClientConfig;
use token_station_cli::filelog::{FileLog, Recorders};
use token_station_cli::gateway::Gateway;
use token_station_cli::store::SqliteStore;
use token_station_cli::{server, virtual_key};
use token_station_metrics::Recorder;

const ERROR_DETAIL_LIMIT: usize = 512;
const DRAIN_GRACE: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
const LISTENER_PROBE_TIMEOUT: Duration = Duration::from_millis(200);
const BIND_HANDOFF_TIMEOUT: Duration = Duration::from_secs(1);

fn timed_stage<T, E>(
    stage: &'static str,
    operation: impl FnOnce() -> Result<T, E>,
) -> Result<T, StartFailure>
where
    E: ToString,
{
    let started = Instant::now();
    let result = operation().map_err(|error| StartFailure::new(stage, error));
    eprintln!(
        "proxy startup stage={stage} elapsed_ms={}",
        started.elapsed().as_millis()
    );
    result
}

/// A fully prepared server that has not occupied its public listener yet.
pub(crate) struct PreparedServer {
    runtime: tokio::runtime::Runtime,
    app_state: server::AppState,
    listen: String,
    virtual_key: Option<String>,
}

/// A prepared server whose public socket is already reserved. Slow bind/retry
/// work happens before this value is committed under the App state lock.
pub(crate) struct BoundPreparedServer {
    runtime: tokio::runtime::Runtime,
    app_state: server::AppState,
    listener: StdTcpListener,
    listen: String,
    virtual_key: Option<String>,
}

/// The published proxy instance owned by the desktop Runtime Supervisor.
pub(crate) struct RunningServer {
    runtime: tokio::runtime::Runtime,
    app_state: server::AppState,
    listen: String,
    virtual_key: Option<String>,
    running_revision: u64,
    instance_id: String,
    serve_task: tokio::task::JoinHandle<std::io::Result<()>>,
    retired_controls: Vec<server::ServerControl>,
}

impl RunningServer {
    pub(crate) fn listen(&self) -> &str {
        &self.listen
    }

    pub(crate) fn virtual_key(&self) -> Option<&str> {
        self.virtual_key.as_deref()
    }

    pub(crate) const fn running_revision(&self) -> u64 {
        self.running_revision
    }

    pub(crate) fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub(crate) fn plugins(&self) -> &token_station_cli::config::PluginsConfig {
        &self.app_state.admin.plugins
    }

    pub(crate) fn is_task_alive(&self) -> bool {
        !self.serve_task.is_finished()
    }

    /// Hot-swap one Agent's route on the running gateway — no full restart, so
    /// other Agents' traffic is untouched. `router` is `None` to clear a custom
    /// route (inherit Home) or `Some` to install one.
    pub(crate) fn reload_agent_router(
        &self,
        agent_id: &str,
        router: Option<token_station_router_core::RouterConfig>,
    ) -> Result<(), String> {
        self.app_state.gateway.reload_agent_router(agent_id, router)
    }

    #[cfg(test)]
    pub(crate) fn abort_task(&self) {
        self.serve_task.abort();
    }

    pub(crate) fn listener_reachable(&self) -> bool {
        let Ok(address) = self.listen.parse::<SocketAddr>() else {
            return false;
        };
        TcpStream::connect_timeout(&address, LISTENER_PROBE_TIMEOUT).is_ok()
    }

    pub(crate) fn stop_accepting(&self) {
        self.app_state.control.stop_accepting();
    }

    /// Restores this instance after a same-port candidate failed to bind.
    pub(crate) fn resume_accepting(
        &mut self,
        listener: StdTcpListener,
    ) -> Result<(), StartFailure> {
        let listener = into_tokio_listener(&self.runtime, listener)?;
        self.retired_controls.push(self.app_state.control.clone());
        self.app_state = server::AppState::new(
            Arc::clone(&self.app_state.gateway),
            self.app_state.virtual_key.clone(),
            Arc::clone(&self.app_state.admin),
        )
        .with_running_revision(self.running_revision);
        let served_state = self.app_state.clone();
        self.serve_task = self
            .runtime
            .spawn(async move { server::serve(served_state, listener).await });
        Ok(())
    }

    /// Runs on Tauri's blocking pool. Existing requests get the approved five
    /// second grace; only the remainder is cancelled.
    pub(crate) fn drain_and_shutdown(self) {
        self.stop_accepting();
        let deadline = Instant::now() + DRAIN_GRACE;
        let in_flight = || {
            self.app_state.control.in_flight()
                + self
                    .retired_controls
                    .iter()
                    .map(server::ServerControl::in_flight)
                    .sum::<usize>()
        };
        while in_flight() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if in_flight() > 0 {
            self.app_state.control.cancel_in_flight();
            for control in &self.retired_controls {
                control.cancel_in_flight();
            }
        }
        self.runtime.shutdown_timeout(SHUTDOWN_TIMEOUT);
    }
}

impl PreparedServer {
    pub(crate) fn listen(&self) -> &str {
        &self.listen
    }

    /// Reserves the configured listener. This can retry for up to one second,
    /// so callers must execute it without holding the global App state lock.
    pub(crate) fn bind(self) -> Result<BoundPreparedServer, StartFailure> {
        let Self {
            runtime,
            app_state,
            listen,
            virtual_key,
        } = self;
        let listener = bind_with_retry(&listen)?;
        Ok(BoundPreparedServer {
            runtime,
            app_state,
            listener,
            listen,
            virtual_key,
        })
    }

    pub(crate) fn bind_listener(listen: &str) -> Result<StdTcpListener, StartFailure> {
        bind_with_retry(listen)
    }
}

impl BoundPreparedServer {
    pub(crate) fn listen(&self) -> &str {
        &self.listen
    }

    /// Publishes an immutable revision from an already reserved socket. No
    /// caller may update `running_revision` before this succeeds.
    pub(crate) fn publish(self, revision: u64) -> Result<RunningServer, StartFailure> {
        let Self {
            runtime,
            app_state,
            listener,
            listen,
            virtual_key,
        } = self;
        let listener = into_tokio_listener(&runtime, listener)?;
        let app_state = app_state.with_running_revision(revision);
        let published_listen = listener
            .local_addr()
            .map(|address| address.to_string())
            .unwrap_or(listen);
        let instance_id = instance_id()?;
        let served_state = app_state.clone();
        let serve_task = runtime.spawn(async move { server::serve(served_state, listener).await });
        Ok(RunningServer {
            runtime,
            app_state,
            listen: published_listen,
            virtual_key,
            running_revision: revision,
            instance_id,
            serve_task,
            retired_controls: Vec::new(),
        })
    }

    pub(crate) fn discard(self) {
        self.runtime.shutdown_background();
    }
}

fn bind_with_retry(listen: &str) -> Result<StdTcpListener, StartFailure> {
    let deadline = Instant::now() + BIND_HANDOFF_TIMEOUT;
    loop {
        match StdTcpListener::bind(listen) {
            Ok(listener) => {
                listener
                    .set_nonblocking(true)
                    .map_err(|error| StartFailure::new("listen_nonblocking", error))?;
                return Ok(listener);
            }
            Err(error) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
                drop(error);
            }
            Err(error) => return Err(StartFailure::new("listen_bind", error)),
        }
    }
}

fn into_tokio_listener(
    runtime: &tokio::runtime::Runtime,
    listener: StdTcpListener,
) -> Result<tokio::net::TcpListener, StartFailure> {
    let _entered = runtime.enter();
    tokio::net::TcpListener::from_std(listener)
        .map_err(|error| StartFailure::new("listen_publish", error))
}

fn instance_id() -> Result<String, StartFailure> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| StartFailure::new("instance_id", error))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    ))
}

#[derive(Clone, Debug)]
pub(crate) struct StartFailure {
    stage: &'static str,
    detail: String,
}

impl StartFailure {
    pub(crate) fn new(stage: &'static str, detail: impl ToString) -> Self {
        let detail = detail.to_string();
        let detail = if detail.chars().count() > ERROR_DETAIL_LIMIT {
            let mut truncated = detail.chars().take(ERROR_DETAIL_LIMIT).collect::<String>();
            truncated.push('…');
            truncated
        } else {
            detail
        };
        Self { stage, detail }
    }

    pub(crate) fn public_message(&self) -> String {
        format!("{}: {}", self.stage, self.detail)
    }
}

/// Performs every potentially blocking preflight step without binding the
/// listener, so a running instance keeps serving throughout preparation.
pub(crate) fn prepare_server(config: ClientConfig) -> Result<PreparedServer, StartFailure> {
    let mut sinks: Vec<Box<dyn Recorder>> = vec![Box::new(timed_stage("log_open", || {
        FileLog::open(&config.data.dir)
    })?)];
    if config.data.metrics {
        sinks.push(Box::new(timed_stage("metrics_open", || {
            SqliteStore::open(&config.data.dir.join("metrics.sqlite"))
        })?));
    }

    let gateway = Arc::new(timed_stage("gateway_init", || {
        Gateway::new(&config, Arc::new(Recorders(sinks)))
    })?);
    let virtual_key = if config.server.auth {
        let (key, _created) = timed_stage("virtual_key", || {
            virtual_key::load_or_create(&config.data.dir)
        })?;
        Some(key)
    } else {
        None
    };

    let runtime = timed_stage("runtime_init", || {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
    })?;
    let app_state = server::AppState::new(
        gateway,
        virtual_key.clone().map(Arc::from),
        Arc::new(token_station_cli::admin::AdminContext {
            data_dir: config.data.dir.clone(),
            router: config.router.clone(),
            plugins: config.plugins.clone(),
        }),
    );

    Ok(PreparedServer {
        runtime,
        app_state,
        listen: config.server.listen,
        virtual_key,
    })
}

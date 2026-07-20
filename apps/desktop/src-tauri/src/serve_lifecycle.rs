//! Heavy proxy preparation kept outside the desktop control-plane mutex.

use std::sync::Arc;
use std::time::{Duration, Instant};

use token_station_cli::config::ClientConfig;
use token_station_cli::filelog::{FileLog, Recorders};
use token_station_cli::gateway::Gateway;
use token_station_cli::store::SqliteStore;
use token_station_cli::{server, virtual_key};
use token_station_metrics::Recorder;

const ERROR_DETAIL_LIMIT: usize = 512;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

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

/// A fully prepared server that has not yet been published to global state.
pub(crate) struct PreparedServer {
    runtime: tokio::runtime::Runtime,
    listener: tokio::net::TcpListener,
    app_state: server::AppState,
    listen: String,
    virtual_key: Option<String>,
}

/// The published proxy instance owned by the desktop lifecycle state machine.
pub(crate) struct RunningServer {
    runtime: tokio::runtime::Runtime,
    listen: String,
    virtual_key: Option<String>,
}

impl RunningServer {
    pub(crate) fn listen(&self) -> &str {
        &self.listen
    }

    pub(crate) fn virtual_key(&self) -> Option<&str> {
        self.virtual_key.as_deref()
    }

    /// Runs on Tauri's blocking pool. Waiting here must never hold AppInner's mutex.
    pub(crate) fn shutdown(self) {
        self.runtime.shutdown_timeout(SHUTDOWN_TIMEOUT);
    }
}

impl PreparedServer {
    pub(crate) fn publish(self) -> RunningServer {
        let Self {
            runtime,
            listener,
            app_state,
            listen,
            virtual_key,
        } = self;
        runtime.spawn(async move {
            let _ = server::serve(app_state, listener).await;
        });
        RunningServer {
            runtime,
            listen,
            virtual_key,
        }
    }

    /// A cancelled preparation was never published. Drop its listener first and
    /// let Tokio finish tearing down without blocking the async coordinator.
    pub(crate) fn discard(self) {
        let Self {
            runtime, listener, ..
        } = self;
        drop(listener);
        runtime.shutdown_background();
    }
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

/// Performs every potentially blocking startup step without touching AppInner.
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
    let listen = config.server.listen.clone();
    let listener = timed_stage("listen_bind", || {
        runtime.block_on(async { tokio::net::TcpListener::bind(&listen).await })
    })?;
    let app_state = server::AppState {
        gateway,
        virtual_key: virtual_key.clone().map(Arc::from),
        admin: Arc::new(token_station_cli::admin::AdminContext {
            data_dir: config.data.dir.clone(),
            router: config.router.clone(),
            plugins: config.plugins.clone(),
        }),
    };

    Ok(PreparedServer {
        runtime,
        listener,
        app_state,
        listen,
        virtual_key,
    })
}

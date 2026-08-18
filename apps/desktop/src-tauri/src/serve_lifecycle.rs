//! Heavy proxy preparation and supervised listener ownership.

use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpListener as StdTcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use token_station_cli::bodylog::BodyLog;
use token_station_cli::config::ClientConfig;
use token_station_cli::filelog::{FileLog, Recorders};
use token_station_cli::gateway::{Gateway, PrevalidatedAgentRouter};
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
    serving_config: ClientConfig,
}

/// A prepared server whose public socket is already reserved. Slow bind/retry
/// work happens before this value is committed under the App state lock.
pub(crate) struct BoundPreparedServer {
    runtime: tokio::runtime::Runtime,
    app_state: server::AppState,
    listener: StdTcpListener,
    listen: String,
    virtual_key: Option<String>,
    serving_config: ClientConfig,
}

/// The published proxy instance owned by the desktop Runtime Supervisor.
pub(crate) struct RunningServer {
    runtime: tokio::runtime::Runtime,
    app_state: server::AppState,
    listen: String,
    virtual_key: Option<String>,
    running_revision: u64,
    instance_id: String,
    serving_config: ClientConfig,
    /// Exact per-Agent router documents hot-swapped after this server started.
    /// `None` means that Agent was explicitly returned to the serving Home
    /// router. Keeping this separately avoids pretending unrelated draft
    /// provider/model/pricing edits reached the running Gateway.
    agent_router_overrides: BTreeMap<String, Option<token_station_router_core::RouterConfig>>,
    serve_task: tokio::task::JoinHandle<std::io::Result<()>>,
    retired_controls: Vec<server::ServerControl>,
}

/// A one-shot hot-reload transaction prepared against one published server.
/// Both the serving-snapshot boundary and router-core construction have passed;
/// installing it after durable config save has no recoverable failure path.
pub(crate) struct PreparedAgentRouterReload {
    agent_id: String,
    router: Option<token_station_router_core::RouterConfig>,
    gateway_plan: PrevalidatedAgentRouter,
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

    pub(crate) fn agent_adapter_ready(&self, package: &str) -> bool {
        self.app_state.gateway.agent_adapter_ready(package)
    }

    pub(crate) fn serving_config(&self) -> &ClientConfig {
        &self.serving_config
    }

    pub(crate) fn agent_router_override(
        &self,
        agent_id: &str,
    ) -> Option<Option<&token_station_router_core::RouterConfig>> {
        self.agent_router_overrides
            .get(agent_id)
            .map(Option::as_ref)
    }

    /// Proves that every target in a candidate Agent router belongs to this
    /// published server snapshot. Draft-only Providers and models cannot be
    /// hot-swapped into a Gateway that was built before they existed.
    pub(crate) fn validate_agent_router_targets(
        &self,
        router: &token_station_router_core::RouterConfig,
    ) -> Result<(), String> {
        for target in router
            .pools
            .values()
            .flatten()
            .chain(router.quota_accounts.iter())
        {
            let upstream = self
                .serving_config
                .upstreams
                .get(target.upstream.as_str())
                .ok_or_else(|| {
                    format!(
                        "当前运行实例不包含路由目标 `{target}`：Provider `{}` 尚未发布；请先全量应用配置再重试",
                        target.upstream
                    )
                })?;
            if !upstream
                .models
                .iter()
                .any(|capability| capability.model == target.model)
            {
                return Err(format!(
                    "当前运行实例不包含路由目标 `{target}`：模型 `{}` 尚未在 Provider `{}` 发布；请先全量应用配置再重试",
                    target.model, target.upstream
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn prepare_agent_router_reload(
        &self,
        agent_id: &str,
        router: Option<token_station_router_core::RouterConfig>,
    ) -> Result<PreparedAgentRouterReload, String> {
        if let Some(router) = router.as_ref() {
            self.validate_agent_router_targets(router)?;
        }
        let applied_router = router.clone();
        let gateway_plan = Gateway::prepare_agent_router_reload(agent_id, router)?;
        Ok(PreparedAgentRouterReload {
            agent_id: agent_id.to_owned(),
            router: applied_router,
            gateway_plan,
        })
    }

    pub(crate) fn is_task_alive(&self) -> bool {
        !self.serve_task.is_finished()
    }

    pub(crate) fn install_prevalidated_agent_router(
        &mut self,
        prepared: PreparedAgentRouterReload,
    ) {
        self.app_state
            .gateway
            .install_prevalidated_agent_router(prepared.gateway_plan);
        self.agent_router_overrides
            .insert(prepared.agent_id, prepared.router);
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

    #[cfg(target_os = "macos")]
    pub(crate) fn is_accepting(&self) -> bool {
        self.app_state.control.is_accepting()
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
            serving_config,
        } = self;
        let listener = bind_with_retry(&listen)?;
        Ok(BoundPreparedServer {
            runtime,
            app_state,
            listener,
            listen,
            virtual_key,
            serving_config,
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
            serving_config,
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
            serving_config,
            agent_router_overrides: BTreeMap::new(),
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
            Err(error) => {
                return Err(if error.kind() == std::io::ErrorKind::AddrInUse {
                    StartFailure::new(
                        "listen_bind",
                        format!(
                            "监听地址 {listen} 已被占用。请关闭另一个 Token Station CLI/桌面实例或占用该端口的程序，或在设置中更换监听端口"
                        ),
                    )
                } else {
                    StartFailure::new("listen_bind", format!("无法监听 {listen}：{error}"))
                });
            }
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
    let runtime = timed_stage("runtime_init", || {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
    })?;
    let mut sinks: Vec<Box<dyn Recorder>> = vec![Box::new(timed_stage("log_open", || {
        FileLog::open(&config.data.dir)
    })?)];
    if config.data.metrics {
        sinks.push(Box::new(timed_stage("metrics_open", || {
            SqliteStore::open(&config.data.dir.join("metrics.sqlite"))
        })?));
    }

    let body_log = Arc::new(timed_stage("body_log_open", || {
        BodyLog::open(&config.data.dir)
    })?);
    let gateway = Arc::new(timed_stage("gateway_init", || {
        Gateway::new_with_provider_runtime(
            &config,
            Arc::new(Recorders(sinks)),
            runtime.handle().clone(),
        )
        .map(|gateway| gateway.with_body_log(body_log))
    })?);
    let admin = Arc::new(timed_stage("admin_snapshot", || {
        token_station_cli::admin::AdminContext::from_config(&config)
    })?);
    let virtual_key = if config.server.auth {
        let (key, _created) = timed_stage("virtual_key", || {
            virtual_key::load_or_create(&config.data.dir)
        })?;
        Some(key)
    } else {
        None
    };

    let app_state = server::AppState::new(gateway, virtual_key.clone().map(Arc::from), admin);

    Ok(PreparedServer {
        runtime,
        app_state,
        listen: config.server.listen.clone(),
        virtual_key,
        serving_config: config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_config() -> ClientConfig {
        let mut config: ClientConfig = serde_json::from_str(token_station_cli::EXAMPLE_CONFIG)
            .expect("example config is valid");
        config.plugins.dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("plugins-dist");
        config
    }

    #[test]
    fn occupied_listener_error_names_the_address_and_recovery() {
        let occupied = StdTcpListener::bind("127.0.0.1:0").unwrap();
        let listen = occupied.local_addr().unwrap().to_string();
        let error = bind_with_retry(&listen).unwrap_err().public_message();

        assert!(error.contains(&listen), "{error}");
        assert!(error.contains("已被占用"), "{error}");
        assert!(error.contains("更换监听端口"), "{error}");
    }

    #[test]
    fn hot_agent_router_records_only_the_applied_router_not_unserved_draft_metadata() {
        let mut serving = test_config();
        serving.server.listen = "127.0.0.1:0".to_owned();
        serving.server.auth = false;
        serving.data.metrics = false;
        let mut running = prepare_server(serving.clone())
            .unwrap()
            .bind()
            .unwrap()
            .publish(1)
            .unwrap();
        let mut latest = serving.clone();
        latest.pricing.version = 9;
        latest.upstreams.get_mut("openai_personal").unwrap().models[0].context_window = 1;
        latest.agent_routes.insert(
            "opencode".to_owned(),
            serde_json::from_value(json!({
                "mode": "inherit",
                "routing_mode": "direct",
                "direct_target": {"upstream": "ollama_local", "model": "llama3.3"}
            }))
            .unwrap(),
        );
        let router = latest
            .custom_router_for_agent("opencode")
            .unwrap()
            .expect("the draft compiles a per-Agent router");

        let prepared = running
            .prepare_agent_router_reload("opencode", Some(router.clone()))
            .unwrap();
        running.install_prevalidated_agent_router(prepared);

        assert_eq!(running.serving_config().pricing, serving.pricing);
        assert_eq!(
            running.serving_config().upstreams["openai_personal"].models[0].context_window,
            serving.upstreams["openai_personal"].models[0].context_window
        );
        assert_eq!(
            running.agent_router_override("opencode"),
            Some(Some(&router))
        );
        running.drain_and_shutdown();
    }

    #[test]
    fn hot_agent_router_rejects_targets_missing_from_the_running_serving_snapshot() {
        let mut serving = test_config();
        serving.server.listen = "127.0.0.1:0".to_owned();
        serving.server.auth = false;
        serving.data.metrics = false;
        let running = prepare_server(serving)
            .unwrap()
            .bind()
            .unwrap()
            .publish(1)
            .unwrap();
        let cases = [
            (
                "new_provider/new-model",
                json!({
                    "version": 1,
                    "pools": {
                        "direct": [{"upstream": "new_provider", "model": "new-model"}]
                    },
                    "default_pool": "direct"
                }),
            ),
            (
                "ollama_local/new-model",
                json!({
                    "version": 1,
                    "pools": {},
                    "default_pool": "",
                    "routing_mode": "quota_first",
                    "quota_accounts": [{"upstream": "ollama_local", "model": "new-model"}]
                }),
            ),
        ];

        for (missing_target, document) in cases {
            let router: token_station_router_core::RouterConfig =
                serde_json::from_value(document).unwrap();
            let error = running.validate_agent_router_targets(&router).expect_err(
                "a hot route cannot refer to a target absent from this running instance",
            );

            assert!(error.contains(missing_target), "{error}");
            assert!(error.contains("当前运行实例不包含"), "{error}");
            assert!(error.contains("全量应用"), "{error}");
        }
        running.drain_and_shutdown();
    }
}

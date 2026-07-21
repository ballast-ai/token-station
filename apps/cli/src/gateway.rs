//! The synchronous data plane: one inbound request, end to end.
//!
//! ```text
//! inbound JSON ──▶ agent plugin (normalize + hints)   [sync wasm]
//!             ──▶ router.route()                       [pure]
//!             ──▶ provider plugin (build request)      [sync wasm]
//!             ──▶ ProviderConfig::authorize            [the exfiltration gate]
//!             ──▶ inject credential, send via ureq     [sync IO]
//!             ──▶ parse / stream-parse + render        [sync wasm]
//! ```
//!
//! Everything here blocks; the axum layer runs it on a blocking thread and
//! bridges streams back through a channel. That is the architectural choice: the wasm
//! runtime is synchronous, so the data plane is too, and async stops at the
//! server facade.
//!
//! # What this module never does
//!
//! Log content. The one line it prints per request is the routing decision,
//! which is content-free by `router-core`'s construction.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use token_station_conformance::{AgentAdapter, ProviderAdapter};
use token_station_metrics::{Recorder, RequestRecord, RoutingRecord};
use token_station_plugin_runtime::{AgentPlugin, NoSecrets, PluginRuntime, ProviderPlugin};
use token_station_protocol::{
    AgentRequestEnvelope, Auth, ChatRequest, ErrorCode, ErrorEnvelope, HeaderDigest, HttpMethod,
    HttpRequestDescriptor, HttpResponseParts, Principal, ProviderConfig, SecretRef, StreamChunk,
    StreamEvent, StreamOutcome,
};
use token_station_router_core::{
    Candidate, Decision, HealthPolicy, HealthTracker, Router, UpstreamModel, UpstreamRef,
};

use crate::request_context::RequestContext;

use crate::config::ClientConfig;
use crate::secrets::SecretStore;

/// Caps on what crosses the proxy, applied by the host per architecture section 6.
const MAX_INBOUND_BODY: usize = 10 * 1024 * 1024;
const MAX_UPSTREAM_BODY: u64 = 32 * 1024 * 1024;
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(120);

/// Default overall budget for one request when the caller supplies no context.
/// Long enough for slow generations, finite enough to bound an abandoned one.
const REQUEST_DEADLINE: Duration = Duration::from_secs(600);
/// `upstream test` answers "is it alive"; it should not take the data plane's
/// word-count-sized timeout to say no.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// One socket read's worth of stream body, fed to the parser as-is — split
/// points are the network's, which is exactly what conformance drilled the
/// parser on.
const STREAM_READ: usize = 8 * 1024;
static NEXT_STREAM_ID: AtomicU64 = AtomicU64::new(1);

/// One configured upstream, resolved and ready to serve.
struct Upstream {
    config: ProviderConfig,
    plugin: Arc<ProviderPlugin>,
}

/// One loaded inbound adapter and the protocol its manifest declares. The
/// gateway holds several and asks each `match_inbound` which claims a request.
struct LoadedAgent {
    plugin: AgentPlugin,
    protocol: String,
}

/// The assembled data plane.
pub struct Gateway {
    /// Inbound adapters in match-priority order. Each request is dispatched to
    /// the first whose `match_inbound` claims it — that is `match_inbound`, the
    /// multi-inbound multiplexing, done here in the host orchestrator.
    agents: Vec<LoadedAgent>,
    home_router: Router,
    /// Only custom routes are materialized. Missing/inherit entries use the
    /// home router, so old configurations allocate no duplicate routers.
    agent_routers: BTreeMap<String, Router>,
    upstreams: BTreeMap<String, Upstream>,
    /// What each upstream serves; health is applied per request from the
    /// tracker, because it changes and this does not.
    catalog: Vec<(UpstreamModel, token_station_protocol::ModelCapability)>,
    health: std::sync::Mutex<HealthTracker>,
    secrets: SecretStore,
    http: ureq::Agent,
    recorder: Arc<dyn Recorder>,
    /// `/v1/models`, rendered once: it changes only with the config.
    models_document: String,
}

/// A finished non-streaming exchange, ready to be an HTTP response.
pub struct JsonReply {
    pub status: u16,
    pub body: String,
}

/// One `upstream test` probe: which model was asked, and either how fast it
/// answered or why it did not (a value-free, operator-facing reason).
#[derive(Debug)]
pub struct ProbeOutcome {
    pub model: String,
    pub latency_ms: Result<u64, String>,
}

/// What the blocking worker sends the async facade, in order: exactly one
/// `Begin*`, then zero or more `Chunk`s if streaming began.
pub enum Reply {
    BeginJson(JsonReply),
    BeginStream,
    Chunk(String),
}

/// Loads one provider plugin per dialect the configured upstreams speak,
/// resolving each through the registry: builtin bytes or a package directory.
fn load_provider_plugins(
    runtime: &PluginRuntime,
    registry: &crate::plugins::PluginRegistry,
    config: &ClientConfig,
) -> Result<BTreeMap<String, Arc<ProviderPlugin>>, String> {
    let mut provider_plugins: BTreeMap<String, Arc<ProviderPlugin>> = BTreeMap::new();
    for (name, entry) in &config.upstreams {
        if provider_plugins.contains_key(&entry.provider) {
            continue;
        }
        let binding = registry.provider_binding(&entry.provider).ok_or_else(|| {
            format!(
                "upstream `{name}` speaks `{}`, but no plugin provides that dialect; \
                 available: [{}] (scanned {})",
                entry.provider,
                registry.provider_dialects().join(", "),
                config.plugins.dir.display(),
            )
        })?;
        let plugin = match &binding.source {
            crate::plugins::PackageSource::Builtin {
                manifest_source,
                wasm,
            } => ProviderPlugin::load_embedded(runtime, manifest_source, wasm, NoSecrets),
            crate::plugins::PackageSource::Dir(dir) => {
                ProviderPlugin::load(runtime, dir, NoSecrets)
            }
        }
        .map_err(|error| {
            format!(
                "provider plugin for `{}` (package `{}`): {error}",
                entry.provider, binding.package,
            )
        })?;
        provider_plugins.insert(entry.provider.clone(), Arc::new(plugin));
    }
    Ok(provider_plugins)
}

impl Gateway {
    /// Loads plugins, probes capabilities, validates the routing table, and
    /// renders the model catalog.
    ///
    /// # Errors
    ///
    /// A human-readable reason this configuration cannot serve. Startup
    /// errors are for the operator; they are as specific as possible.
    ///
    /// # Panics
    ///
    /// Never for a [`ClientConfig`] that came through [`ClientConfig::load`]:
    /// the `expect`s below restate what its validation already proved.
    pub fn new(config: &ClientConfig, recorder: Arc<dyn Recorder>) -> Result<Self, String> {
        let runtime = PluginRuntime::new(token_station_plugin_runtime::RuntimeLimits::default())
            .map_err(|error| format!("wasm engine: {error}"))?;

        // Resolve both plugin kinds through the discovery registry: builtin
        // packages load from embedded bytes, everything else from its
        // directory. Only packages configured upstreams actually speak are
        // loaded — a broken package no upstream uses is `plugin list`'s
        // business, not startup's.
        let registry = crate::plugins::PluginRegistry::for_config(config)?;

        // Load every configured inbound adapter. Order is match priority: the
        // first whose match_inbound claims a request serves it.
        let mut agents = Vec::new();
        for package in config.plugins.effective_agents() {
            let plugin = match registry.agent_source(&package) {
                crate::plugins::PackageSource::Builtin {
                    manifest_source,
                    wasm,
                } => AgentPlugin::load_embedded(&runtime, manifest_source, wasm),
                crate::plugins::PackageSource::Dir(dir) => AgentPlugin::load(&runtime, &dir),
            }
            .map_err(|error| format!("agent plugin `{package}`: {error}"))?;
            let protocol = plugin
                .manifest()
                .agent_protocols
                .first()
                .cloned()
                .ok_or_else(|| format!("agent plugin `{package}` declares no protocol"))?;
            agents.push(LoadedAgent { plugin, protocol });
        }
        if agents.is_empty() {
            return Err("no agent adapters configured (plugins.agents is empty)".to_owned());
        }

        let provider_plugins = load_provider_plugins(&runtime, &registry, config)?;

        let mut upstreams = BTreeMap::new();
        let mut catalog = Vec::new();
        let mut models_document: Vec<Value> = Vec::new();
        let mut seen_models = std::collections::BTreeSet::new();

        for (name, entry) in &config.upstreams {
            let plugin = Arc::clone(
                provider_plugins
                    .get(&entry.provider)
                    .expect("the loop above loaded a plugin for every configured dialect"),
            );

            let mut provider_config =
                ProviderConfig::new(entry.provider.clone(), entry.base_url.clone());
            provider_config.auth = entry.auth.as_ref().map(|auth| SecretRef::new(&auth.slot));
            provider_config.models.clone_from(&entry.models);

            // The adapter may refine the declared catalog; it cannot invent one,
            // having no network. This is `model_capabilities`' production call.
            let capabilities = plugin
                .model_capabilities(&provider_config)
                .map_err(|error| format!("upstream `{name}` capabilities: {}", error.message))?;

            let reference = token_station_router_core::UpstreamRef::new(name.clone())
                .expect("config validation checked the shape");
            for capability in &capabilities {
                catalog.push((
                    UpstreamModel::new(reference.clone(), capability.model.clone()),
                    capability.clone(),
                ));
                if seen_models.insert(capability.model.clone()) {
                    models_document.push(json!({
                        "id": capability.model,
                        "object": "model",
                        "owned_by": name,
                    }));
                }
            }

            upstreams.insert(
                name.clone(),
                Upstream {
                    config: provider_config,
                    plugin,
                },
            );
        }

        let home_router = Router::new(config.router.clone()).map_err(|error| error.to_string())?;
        let mut agent_routers = BTreeMap::new();
        for agent_id in config.agent_routes.keys() {
            if let Some(router) = config.custom_router_for_agent(agent_id)? {
                let router = Router::new(router)
                    .map_err(|error| format!("Agent `{agent_id}` route: {error}"))?;
                agent_routers.insert(agent_id.clone(), router);
            }
        }

        Ok(Self {
            agents,
            home_router,
            agent_routers,
            upstreams,
            catalog,
            health: std::sync::Mutex::new(HealthTracker::new(HealthPolicy {
                eject_after: config.health.eject_after,
                cooldown: Duration::from_millis(config.health.cooldown_ms),
            })),
            secrets: SecretStore::from_config(config),
            recorder,
            http: ureq::Agent::new_with_config(
                ureq::Agent::config_builder()
                    .timeout_global(Some(UPSTREAM_TIMEOUT))
                    // The pipeline maps upstream errors itself; a non-2xx is
                    // an answer, not a transport failure.
                    .http_status_as_error(false)
                    .build(),
            ),
            models_document: json!({ "object": "list", "data": models_document }).to_string(),
        })
    }

    /// `GET /v1/models`.
    #[must_use]
    pub fn models(&self) -> &str {
        &self.models_document
    }

    /// How many distinct models the catalog advertises.
    #[must_use]
    pub fn catalog_size(&self) -> usize {
        self.catalog.len()
    }

    /// `upstream test <name>`: one minimal real completion per declared model.
    ///
    /// Runs the production southbound path — provider adapter, exfiltration
    /// gate, credential injection — but neither the agent plugin nor the
    /// router, because what is under test is the upstream, not the routing
    /// table. Nothing is recorded and no health state is fed: a probe is
    /// operator diagnostics the operator explicitly asked for, not served
    /// traffic.
    ///
    /// # Errors
    ///
    /// An unknown upstream or model — per-model failures are data, returned
    /// inside [`ProbeOutcome`], so one dead model does not hide the others.
    pub fn probe(
        &self,
        upstream_name: &str,
        only_model: Option<&str>,
    ) -> Result<Vec<ProbeOutcome>, String> {
        let upstream = self.upstreams.get(upstream_name).ok_or_else(|| {
            format!(
                "no upstream `{upstream_name}`; configured: {}",
                self.upstreams
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;

        let mut models: Vec<&str> = self
            .catalog
            .iter()
            .filter(|(target, _)| target.upstream.as_str() == upstream_name)
            .map(|(target, _)| target.model.as_str())
            .collect();
        if let Some(only) = only_model {
            if !models.contains(&only) {
                return Err(format!(
                    "upstream `{upstream_name}` does not serve `{only}`; it serves: {}",
                    models.join(", ")
                ));
            }
            models = vec![only];
        }
        if models.is_empty() {
            return Err(format!("upstream `{upstream_name}` declares no models"));
        }

        let http = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(PROBE_TIMEOUT))
                .http_status_as_error(false)
                .build(),
        );

        Ok(models
            .into_iter()
            .map(|model| ProbeOutcome {
                model: model.to_owned(),
                latency_ms: self.probe_model(upstream_name, upstream, model, &http),
            })
            .collect())
    }

    /// One probe exchange. The error string is operator-facing and, like every
    /// error out of this module, value-free.
    fn probe_model(
        &self,
        upstream_name: &str,
        upstream: &Upstream,
        model: &str,
        http: &ureq::Agent,
    ) -> Result<u64, String> {
        let mut request = ChatRequest::new(
            model,
            vec![token_station_protocol::Message::text(
                token_station_protocol::Role::User,
                "ping",
            )],
        );
        request.sampling.max_output_tokens = Some(1);

        let describe =
            |envelope: ErrorEnvelope| format!("{} ({:?})", envelope.message, envelope.code);

        let descriptor = upstream
            .plugin
            .build_http_request(&request, &upstream.config)
            .map_err(describe)?;
        upstream
            .config
            .authorize(&descriptor)
            .map_err(|refusal| refusal.to_string())?;

        let clock = std::time::Instant::now();
        let response = self
            .send_with(http, &descriptor, upstream_name)
            .map_err(describe)?;
        let status = response.status;
        let parts: HttpResponseParts = response.into();
        if status >= 400 {
            let envelope = upstream
                .plugin
                .map_provider_error(&parts)
                .map_err(describe)?;
            return Err(format!("HTTP {status}: {}", describe(envelope)));
        }
        upstream.plugin.parse_response(&parts).map_err(describe)?;
        Ok(u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX))
    }

    /// The routing candidates as of this instant: the static catalog with the
    /// tracker's current verdict applied.
    fn candidates(&self, now: std::time::Instant) -> Vec<Candidate> {
        let health = self.health.lock().expect("health lock");
        self.catalog
            .iter()
            .map(|(target, capability)| {
                Candidate::new(
                    target.clone(),
                    capability.clone(),
                    health.health_of(&target.upstream, now),
                )
            })
            .collect()
    }

    /// Feeds one attempt's outcome to the tracker; logs a transition.
    fn observe(&self, upstream: &UpstreamRef, outcome: Result<(), &ErrorEnvelope>) {
        let mut health = self.health.lock().expect("health lock");
        match outcome {
            Ok(()) => health.observe_success(upstream),
            Err(envelope) => {
                if health.observe_failure(upstream, envelope.code, std::time::Instant::now()) {
                    eprintln!(
                        "upstream {upstream} ejected from rotation ({:?})",
                        envelope.code
                    );
                }
            }
        }
    }

    /// `POST /v1/chat/completions`, start to finish.
    ///
    /// `emit` receives exactly one `Reply::Begin*`; streaming chunks follow if
    /// the exchange streams. When `emit` returns `false` (client gone) the
    /// exchange stops and the upstream connection drops with it.
    ///
    /// Exactly one [`RequestRecord`] lands per call, whatever the exit path —
    /// including the ones the caller never sees finish.
    pub fn chat(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
        emit: &mut dyn FnMut(Reply) -> bool,
    ) {
        // No supervised context supplied: a detached one still bounds the
        // request by deadline; the server layer replaces it with a drain-child
        // that also carries the client-disconnect signal.
        let ctx = RequestContext::detached(REQUEST_DEADLINE, UPSTREAM_TIMEOUT);
        self.chat_scoped(&ctx, None, method, path, headers, body, emit);
    }

    /// The normal request pipeline with a host-validated Agent routing scope.
    /// `None` is the backward-compatible, unnamespaced home route.
    #[allow(clippy::too_many_arguments)] // the request pipeline's real surface
    pub fn chat_scoped(
        &self,
        ctx: &RequestContext,
        agent_id: Option<&str>,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
        emit: &mut dyn FnMut(Reply) -> bool,
    ) {
        let clock = std::time::Instant::now();
        let started_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |epoch| {
                u64::try_from(epoch.as_millis()).unwrap_or(u64::MAX)
            });

        let router = match agent_id {
            None => &self.home_router,
            Some(agent_id) if crate::config::ClientConfig::is_known_agent_id(agent_id) => self
                .agent_routers
                .get(agent_id)
                .unwrap_or(&self.home_router),
            Some(agent_id) => {
                let mut record = RequestRecord::begin(started_at_ms, String::new());
                let refusal = ErrorEnvelope::new(
                    ErrorCode::InvalidRequest,
                    404,
                    format!("unknown Agent namespace `{agent_id}`"),
                );
                record.status = refusal.http_status;
                record.error_code = Some(refusal.code);
                emit(Reply::BeginJson(JsonReply {
                    status: refusal.http_status,
                    body: json!({
                        "error": {
                            "message": refusal.message,
                            "type": "invalid_request",
                            "code": "invalid_request"
                        }
                    })
                    .to_string(),
                }));
                record.latency_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
                self.recorder.record(&record);
                return;
            }
        };

        // Ask each inbound adapter which claims this request. The winner's
        // protocol tags the record; no winner is a request in a dialect nothing
        // here serves, and no adapter exists to phrase the refusal in.
        let selected = self.select_agent(method, path, headers);
        let protocol = selected.map_or_else(String::new, |agent| agent.protocol.clone());
        let mut record = RequestRecord::begin(started_at_ms, protocol);

        if let Some(agent) = selected {
            match self.chat_inner(ctx, agent, router, headers, body, emit, &mut record) {
                Ok((upstream, outcome)) => self.settle(&mut record, &upstream, outcome),
                Err(refusal) => {
                    // Failed before any upstream served — a whole error response
                    // the client can still receive; no upstream health verdict.
                    record.status = refusal.http_status;
                    record.error_code = Some(refusal.code);
                    let rendered = Self::render_error(agent, &refusal);
                    emit(Reply::BeginJson(rendered));
                }
            }
        } else {
            let refusal = ErrorEnvelope::new(
                ErrorCode::InvalidRequest,
                404,
                format!("no inbound adapter claims {method} {path}"),
            );
            record.status = refusal.http_status;
            record.error_code = Some(refusal.code);
            emit(Reply::BeginJson(JsonReply {
                status: refusal.http_status,
                body: json!({
                    "error": {
                        "message": refusal.message,
                        "type": "invalid_request",
                        "code": "invalid_request"
                    }
                })
                .to_string(),
            }));
        }

        record.latency_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.recorder.record(&record);
    }

    /// The `match_inbound` step: the first loaded adapter that claims
    /// `{ method, path, headers }` wins. Headers are redacted to a
    /// `HeaderDigest` before an adapter ever sees them, exactly as the request
    /// envelope is. An adapter that traps while matching is logged and skipped,
    /// so one broken adapter cannot veto the rest.
    fn select_agent(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
    ) -> Option<&LoadedAgent> {
        let request_head = json!({
            "method": method,
            "path": path,
            "headers": HeaderDigest::redacting(headers.iter().cloned()),
        });
        for agent in &self.agents {
            match agent.plugin.match_inbound(&request_head) {
                Ok(outcome) if outcome.matched => return Some(agent),
                Ok(_) => {}
                Err(envelope) => eprintln!(
                    "agent `{}` match_inbound errored, skipping: {} ({:?})",
                    agent.protocol, envelope.message, envelope.code
                ),
            }
        }
        None
    }

    /// The pipeline. Returns `Err` only before anything was emitted, so the
    /// caller can still shape a whole error response.
    #[allow(clippy::too_many_arguments)] // the request pipeline's real surface
    fn chat_inner(
        &self,
        ctx: &RequestContext,
        agent: &LoadedAgent,
        router: &Router,
        headers: &[(String, String)],
        body: &[u8],
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<(UpstreamRef, StreamOutcome), ErrorEnvelope> {
        if body.len() > MAX_INBOUND_BODY {
            return Err(ErrorEnvelope::new(
                ErrorCode::InvalidRequest,
                413,
                "request body exceeds the local proxy's limit",
            ));
        }
        let body: Value = serde_json::from_slice(body).map_err(|error| {
            ErrorEnvelope::new(
                ErrorCode::InvalidRequest,
                400,
                format!("body is not JSON: {error}"),
            )
        })?;

        // The envelope an agent adapter is allowed to see: headers already
        // redacted, principal already decided. (Inbound auth itself is C1#4.)
        let header_digest = HeaderDigest::redacting(headers.iter().cloned());
        let envelope = AgentRequestEnvelope {
            protocol: agent.protocol.clone(),
            agent_tool: match agent.protocol.as_str() {
                "openai-responses" => Some("codex".to_owned()),
                "anthropic-messages" if header_digest.contains("x-claude-code-session-id") => {
                    Some("claude-code".to_owned())
                }
                _ => None,
            },
            headers: header_digest,
            principal: Principal {
                subject: "local".to_owned(),
                tenant: None,
            },
            hints: Vec::new(),
            body,
            extensions: token_station_protocol::Extensions::new(),
        };

        let request = agent.plugin.normalize_inbound(&envelope)?;
        let hints = agent.plugin.extract_agent_hint(&envelope)?;
        record.requested_model.clone_from(&request.model);
        record.stream = request.stream;

        let candidates = self.candidates(std::time::Instant::now());
        let decision = router
            .route(&request, &hints, &candidates)
            .map_err(|no_route| {
                ErrorEnvelope::new(no_route.error_code(), 503, no_route.to_string())
            })?;
        eprintln!(
            "route {} -> {} ({:?}), {} fallback(s)",
            request.model,
            decision.chosen,
            decision.decided_by,
            decision.fallbacks.len()
        );
        record.routing = Some(RoutingRecord::from(&decision));

        self.dispatch(ctx, agent, &request, &decision, emit, record)
    }

    /// Tries the decision's targets in order; moves on only while the error
    /// says another upstream is worth trying, and only before first byte out.
    fn dispatch(
        &self,
        ctx: &RequestContext,
        agent: &LoadedAgent,
        request: &ChatRequest,
        decision: &Decision,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<(UpstreamRef, StreamOutcome), ErrorEnvelope> {
        let mut last_error = None;

        for target in std::iter::once(&decision.chosen).chain(&decision.fallbacks) {
            // A client that already hung up (or a fired drain) gets no further
            // upstreams tried on its behalf.
            if ctx.is_cancelled() {
                return Ok((target.upstream.clone(), StreamOutcome::ClientCancelled));
            }
            record.attempts += 1;
            if let Some(routing) = record.routing.as_mut() {
                // The record names who actually served (or last refused), not
                // only who was chosen first.
                target.upstream.as_str().clone_into(&mut routing.upstream);
                routing.model.clone_from(&target.model);
            }
            match self.try_upstream(ctx, agent, request, target, emit, record) {
                // The terminal health verdict and status are decided exactly
                // once, in `settle`; here we only report who served and how the
                // exchange ended. Per-attempt failures below still trip health so
                // the fallback sweep can eject a bad upstream mid-flight.
                Ok(outcome) => return Ok((target.upstream.clone(), outcome)),
                Err(error) => {
                    self.observe(&target.upstream, Err(&error));
                    let retriable = error.code.is_retriable_elsewhere();
                    eprintln!(
                        "upstream {target} failed: {} ({:?})",
                        error.message, error.code
                    );
                    last_error = Some(error);
                    if !retriable {
                        break;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            ErrorEnvelope::new(ErrorCode::Internal, 500, "no upstream was tried")
        }))
    }

    /// The single place a finished exchange's status and upstream health are
    /// written. Only [`StreamOutcome::Complete`] settles as success; every other
    /// outcome records a truthful failure (or a client cancel that is nobody's
    /// fault). Nothing outside this function may set `record.status = 200`.
    fn settle(&self, record: &mut RequestRecord, upstream: &UpstreamRef, outcome: StreamOutcome) {
        match outcome {
            StreamOutcome::Complete => {
                record.status = 200;
                self.observe(upstream, Ok(()));
            }
            StreamOutcome::FailedAfterPartial | StreamOutcome::FailedBeforeOutput => {
                let code = record.error_code.unwrap_or(ErrorCode::UpstreamUnavailable);
                record.error_code = Some(code);
                if record.status < 400 {
                    record.status = 502;
                }
                self.observe(upstream, Err(&ErrorEnvelope::new(code, record.status, "")));
            }
            StreamOutcome::ClientCancelled => {
                // A cancel is not the upstream's failure: no health penalty.
                record.status = 499;
            }
        }
    }

    /// One upstream attempt: build, authorize, inject, send, translate back.
    fn try_upstream(
        &self,
        ctx: &RequestContext,
        agent: &LoadedAgent,
        request: &ChatRequest,
        target: &UpstreamModel,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<StreamOutcome, ErrorEnvelope> {
        let upstream = self
            .upstreams
            .get(target.upstream.as_str())
            .ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCode::Internal,
                    500,
                    format!("upstream `{}` vanished from configuration", target.upstream),
                )
            })?;

        // Routing may have picked a different model than the caller named.
        let mut request = request.clone();
        request.model.clone_from(&target.model);

        let descriptor = upstream
            .plugin
            .build_http_request(&request, &upstream.config)?;

        // The exfiltration gate, in the real request path: the plugin chose the
        // URL and named the credential; nothing is resolved until this passes.
        upstream
            .config
            .authorize(&descriptor)
            .map_err(|refusal| ErrorEnvelope::new(ErrorCode::Internal, 500, refusal.to_string()))?;

        let response = self.send(&descriptor, target.upstream.as_str())?;

        if response.status >= 400 {
            // The adapter classifies; the catalog decides retriability.
            let parts: HttpResponseParts = response.into();
            return Err(upstream.plugin.map_provider_error(&parts)?);
        }

        if request.stream {
            let sequence = NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed);
            let render_context = json!({
                "protocol": record.protocol,
                "stream_id": format!("stream-{sequence}"),
                "response_id": format!("msg_token_station_{sequence}"),
                "model": target.model,
            });
            Ok(Self::relay_stream(
                ctx,
                agent,
                upstream,
                response,
                &render_context,
                emit,
                record,
            ))
        } else {
            let parts: HttpResponseParts = response.into();
            let chat_response = upstream.plugin.parse_response(&parts)?;
            record.usage = Some(chat_response.usage);
            let render_context = json!({
                "protocol": record.protocol,
                "response_id": chat_response.id,
                "model": chat_response.model,
            });
            let rendered = agent
                .plugin
                .render_response(&chat_response, &render_context)?;
            emit(Reply::BeginJson(JsonReply {
                status: 200,
                body: rendered.to_string(),
            }));
            // A fully collected non-stream body is a complete exchange.
            Ok(StreamOutcome::Complete)
        }
    }

    /// Resolves the descriptor into a real HTTP exchange.
    ///
    /// The credential is read here, written into one header, and goes out of
    /// scope with the request. It never touches a log, an error, or the guest.
    fn send(
        &self,
        descriptor: &HttpRequestDescriptor,
        upstream_name: &str,
    ) -> Result<UpstreamResponse, ErrorEnvelope> {
        self.send_with(&self.http, descriptor, upstream_name)
    }

    /// [`Gateway::send`] over a caller-chosen agent — the probe path brings
    /// its own, with a timeout sized for diagnostics rather than generation.
    fn send_with(
        &self,
        http: &ureq::Agent,
        descriptor: &HttpRequestDescriptor,
        upstream_name: &str,
    ) -> Result<UpstreamResponse, ErrorEnvelope> {
        let auth_header = descriptor
            .auth
            .as_ref()
            .map(|auth| self.resolve_auth(auth, upstream_name))
            .transpose()?;

        let sent = match descriptor.method {
            HttpMethod::Get => {
                let mut request = http.get(&descriptor.url);
                for (name, value) in descriptor.headers.iter() {
                    request = request.header(name, value);
                }
                if let Some((header, value)) = &auth_header {
                    request = request.header(header, value);
                }
                request.call()
            }
            HttpMethod::Post => {
                let mut request = http.post(&descriptor.url);
                for (name, value) in descriptor.headers.iter() {
                    request = request.header(name, value);
                }
                if let Some((header, value)) = &auth_header {
                    request = request.header(header, value);
                }
                match &descriptor.body {
                    // Serialized here rather than via ureq's json feature: the
                    // descriptor's own headers already carry the content type
                    // the plugin chose.
                    Some(body) => match serde_json::to_string(body) {
                        Ok(encoded) => request.send(&encoded),
                        Err(error) => {
                            return Err(ErrorEnvelope::new(
                                ErrorCode::Internal,
                                500,
                                format!("descriptor body does not serialize: {error}"),
                            ));
                        }
                    },
                    None => request.send_empty(),
                }
            }
        };
        let response = sent.map_err(|error| {
            ErrorEnvelope::new(
                ErrorCode::UpstreamUnavailable,
                502,
                format!("upstream transport: {error}"),
            )
        })?;

        Ok(UpstreamResponse::from(response))
    }

    /// `protocol::Auth` dialect -> one concrete header.
    fn resolve_auth(
        &self,
        auth: &Auth,
        upstream_name: &str,
    ) -> Result<(String, String), ErrorEnvelope> {
        let unauthorized = |detail: String| ErrorEnvelope::new(ErrorCode::Auth, 401, detail);

        match auth {
            Auth::Bearer { secret } => {
                let value = self
                    .secrets
                    .resolve(upstream_name, secret.as_str())
                    .map_err(unauthorized)?;
                Ok(("authorization".to_owned(), format!("Bearer {value}")))
            }
            Auth::Header { name, secret } => {
                let value = self
                    .secrets
                    .resolve(upstream_name, secret.as_str())
                    .map_err(unauthorized)?;
                Ok((name.clone(), value))
            }
            Auth::OAuth { .. } => Err(ErrorEnvelope::new(
                ErrorCode::Capability,
                501,
                "OAuth upstreams arrive with the platform account (C2)",
            )),
        }
    }

    /// Streams the upstream body through the parse/render pair, chunk by
    /// chunk, with the split points the network chose.
    fn relay_stream(
        ctx: &RequestContext,
        agent: &LoadedAgent,
        upstream: &Upstream,
        response: UpstreamResponse,
        render_context: &Value,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> StreamOutcome {
        let mut parser = upstream.plugin.stream_parser();
        let mut reader = response.reader;

        if !emit(Reply::BeginStream) {
            return StreamOutcome::ClientCancelled;
        }

        // `committed` flips once assistant output has left for the client; it
        // decides whether a later break is FailedAfterPartial (a truncated
        // answer) or FailedBeforeOutput (nothing sent, a candidate may retry).
        let mut committed = false;
        let mut saw_done = false;

        let mut buffer = [0u8; STREAM_READ];
        loop {
            // Between reads is where a mid-stream cancel actually lands: the
            // client hung up or the deadline passed, so stop paying for output
            // nobody will read instead of draining the upstream to its end.
            if ctx.is_cancelled() {
                Self::clear_stream_state(agent, render_context);
                return StreamOutcome::ClientCancelled;
            }
            let read = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) => {
                    // Mid-stream failure: already committed to SSE, so it goes
                    // out as a rendered error event rather than a status code —
                    // and into the outcome, where status 200 alone would lie.
                    let envelope = ErrorEnvelope::new(
                        ErrorCode::UpstreamUnavailable,
                        502,
                        format!("upstream stream broke: {error}"),
                    );
                    record.error_code = Some(envelope.code);
                    let rendered = Self::render_stream_error(agent, &envelope, render_context);
                    emit(Reply::Chunk(rendered));
                    Self::clear_stream_state(agent, render_context);
                    return Self::stream_failure(committed);
                }
            };

            let data = String::from_utf8_lossy(&buffer[..read]).into_owned();
            let events = match parser.parse_chunk(&StreamChunk { data }) {
                Ok(events) => events,
                Err(envelope) => {
                    record.error_code = Some(envelope.code);
                    let rendered = Self::render_stream_error(agent, &envelope, render_context);
                    emit(Reply::Chunk(rendered));
                    Self::clear_stream_state(agent, render_context);
                    return Self::stream_failure(committed);
                }
            };

            for event in events {
                match &event {
                    StreamEvent::Usage { usage } => record.usage = Some(*usage),
                    StreamEvent::Done { .. } => saw_done = true,
                    // An upstream error surfaced mid-stream is a failed exchange,
                    // not a completed one: settle must not read it as success
                    // even when bytes are already out.
                    StreamEvent::Error { error } => {
                        record.error_code = Some(error.code);
                        let rendered = Self::render_stream_error(agent, error, render_context);
                        emit(Reply::Chunk(rendered));
                        Self::clear_stream_state(agent, render_context);
                        return Self::stream_failure(committed);
                    }
                    _ => {}
                }
                let chunk = match agent.plugin.render_stream_event(&event, render_context) {
                    Ok(chunk) => chunk,
                    Err(envelope) => {
                        record.error_code = Some(envelope.code);
                        let rendered = Self::render_stream_error(agent, &envelope, render_context);
                        emit(Reply::Chunk(rendered));
                        Self::clear_stream_state(agent, render_context);
                        return Self::stream_failure(committed);
                    }
                };
                let data = chunk
                    .get("data")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if !emit(Reply::Chunk(data)) {
                    Self::clear_stream_state(agent, render_context);
                    return StreamOutcome::ClientCancelled; // Client hung up.
                }
                if matches!(
                    event,
                    StreamEvent::Delta { .. } | StreamEvent::ToolCallDelta { .. }
                ) {
                    committed = true;
                }
            }
        }

        Self::clear_stream_state(agent, render_context);
        // Clean EOF: a terminal `done` is the only honest success. A stream that
        // stops without one left the answer half-said, whatever it already sent.
        if saw_done {
            StreamOutcome::Complete
        } else {
            Self::stream_failure(committed)
        }
    }

    /// A broken stream maps to a pre/post-output failure so `settle` can decide
    /// retriability without ever recording success.
    const fn stream_failure(committed: bool) -> StreamOutcome {
        if committed {
            StreamOutcome::FailedAfterPartial
        } else {
            StreamOutcome::FailedBeforeOutput
        }
    }

    /// An error, rendered the way the matched inbound protocol spells it.
    fn render_error(agent: &LoadedAgent, envelope: &ErrorEnvelope) -> JsonReply {
        let body = agent
            .plugin
            .map_inbound_error(envelope, &Value::Null)
            .map_or_else(
                |_| {
                    json!({ "error": { "message": envelope.message, "type": "internal" } })
                        .to_string()
                },
                |value| value.to_string(),
            );
        JsonReply {
            status: envelope.http_status,
            body,
        }
    }

    fn render_stream_error(
        agent: &LoadedAgent,
        envelope: &ErrorEnvelope,
        context: &Value,
    ) -> String {
        agent
            .plugin
            .render_stream_event(
                &token_station_protocol::StreamEvent::Error {
                    error: envelope.clone(),
                },
                context,
            )
            .ok()
            .and_then(|chunk| chunk.get("data").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| {
                let body = agent
                    .plugin
                    .map_inbound_error(envelope, context)
                    .map_or_else(
                        |_| json!({"error": {"message": envelope.message}}).to_string(),
                        |value| value.to_string(),
                    );
                format!("data: {body}\n\n")
            })
    }

    fn clear_stream_state(agent: &LoadedAgent, context: &Value) {
        let _ = agent.plugin.render_stream_event(
            &token_station_protocol::StreamEvent::Error {
                error: ErrorEnvelope::new(ErrorCode::Internal, 499, "client disconnected"),
            },
            context,
        );
    }
}

/// An upstream's answer, with the body still unread so streams stay streams.
struct UpstreamResponse {
    status: u16,
    headers: BTreeMap<String, String>,
    reader: Box<dyn Read + Send>,
}

impl UpstreamResponse {
    fn from(response: ureq::http::Response<ureq::Body>) -> Self {
        let status = response.status().as_u16();
        let mut headers = BTreeMap::new();
        for (name, value) in response.headers() {
            if let Ok(value) = value.to_str() {
                headers.insert(name.as_str().to_ascii_lowercase(), value.to_owned());
            }
        }
        let reader = response
            .into_body()
            .into_with_config()
            .limit(MAX_UPSTREAM_BODY)
            .reader();
        Self {
            status,
            headers,
            reader: Box::new(reader),
        }
    }
}

impl From<UpstreamResponse> for HttpResponseParts {
    fn from(mut response: UpstreamResponse) -> Self {
        let mut body = String::new();
        // A read failure mid-body surfaces as truncated JSON, which the
        // adapter reports as a parse error attributed to the upstream.
        let _ = response.reader.read_to_string(&mut body);
        Self {
            status: response.status,
            headers: response.headers,
            body,
            extensions: token_station_protocol::Extensions::new(),
        }
    }
}

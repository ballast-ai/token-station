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
use std::time::Duration;

use serde_json::{Value, json};
use token_station_conformance::{AgentAdapter, ProviderAdapter};
use token_station_metrics::{Recorder, RequestRecord, RoutingRecord};
use token_station_plugin_runtime::{AgentPlugin, NoSecrets, PluginRuntime, ProviderPlugin};
use token_station_protocol::{
    AgentRequestEnvelope, Auth, ChatRequest, ErrorCode, ErrorEnvelope, HeaderDigest, HttpMethod,
    HttpRequestDescriptor, HttpResponseParts, Principal, ProviderConfig, SecretRef, StreamChunk,
};
use token_station_router_core::{
    Candidate, Decision, HealthPolicy, HealthTracker, Router, UpstreamModel, UpstreamRef,
};

use crate::config::ClientConfig;
use crate::secrets::SecretStore;

/// Caps on what crosses the proxy, applied by the host per architecture section 6.
const MAX_INBOUND_BODY: usize = 10 * 1024 * 1024;
const MAX_UPSTREAM_BODY: u64 = 32 * 1024 * 1024;
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(120);
/// One socket read's worth of stream body, fed to the parser as-is — split
/// points are the network's, which is exactly what conformance drilled the
/// parser on.
const STREAM_READ: usize = 8 * 1024;

/// One configured upstream, resolved and ready to serve.
struct Upstream {
    config: ProviderConfig,
    plugin: Arc<ProviderPlugin>,
}

/// The assembled data plane.
pub struct Gateway {
    agent: AgentPlugin,
    /// The inbound protocol this deployment serves, from the agent manifest.
    protocol: String,
    router: Router,
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

/// What the blocking worker sends the async facade, in order: exactly one
/// `Begin*`, then zero or more `Chunk`s if streaming began.
pub enum Reply {
    BeginJson(JsonReply),
    BeginStream,
    Chunk(String),
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

        let agent_dir = config.plugins.dir.join(&config.plugins.agent);
        let agent = AgentPlugin::load(&runtime, &agent_dir)
            .map_err(|error| format!("agent plugin `{}`: {error}", config.plugins.agent))?;
        let protocol = agent
            .manifest()
            .agent_protocols
            .first()
            .cloned()
            .ok_or("agent plugin declares no protocol")?;

        let mut provider_plugins: BTreeMap<String, Arc<ProviderPlugin>> = BTreeMap::new();
        for (provider, package) in &config.plugins.providers {
            let plugin =
                ProviderPlugin::load(&runtime, &config.plugins.dir.join(package), NoSecrets)
                    .map_err(|error| format!("provider plugin `{package}`: {error}"))?;
            provider_plugins.insert(provider.clone(), Arc::new(plugin));
        }

        let mut upstreams = BTreeMap::new();
        let mut catalog = Vec::new();
        let mut models_document: Vec<Value> = Vec::new();
        let mut seen_models = std::collections::BTreeSet::new();

        for (name, entry) in &config.upstreams {
            let plugin = Arc::clone(
                provider_plugins
                    .get(&entry.provider)
                    .expect("config validation checked the mapping"),
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

        let router = Router::new(config.router.clone()).map_err(|error| error.to_string())?;

        Ok(Self {
            agent,
            protocol,
            router,
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
        let mut record = RequestRecord::begin(started_at_ms, self.protocol.clone());

        let reply = self.chat_inner(headers, body, emit, &mut record);
        if let Err(refusal) = reply {
            record.status = refusal.http_status;
            record.error_code = Some(refusal.code);
            let rendered = self.render_error(&refusal);
            emit(Reply::BeginJson(rendered));
        }

        record.latency_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.recorder.record(&record);
    }

    /// The pipeline. Returns `Err` only before anything was emitted, so the
    /// caller can still shape a whole error response.
    fn chat_inner(
        &self,
        headers: &[(String, String)],
        body: &[u8],
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<(), ErrorEnvelope> {
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
        let envelope = AgentRequestEnvelope {
            protocol: self.protocol.clone(),
            agent_tool: None,
            headers: HeaderDigest::redacting(headers.iter().cloned()),
            principal: Principal {
                subject: "local".to_owned(),
                tenant: None,
            },
            hints: Vec::new(),
            body,
            extensions: token_station_protocol::Extensions::new(),
        };

        let request = self.agent.normalize_inbound(&envelope)?;
        let hints = self.agent.extract_agent_hint(&envelope)?;
        record.requested_model.clone_from(&request.model);
        record.stream = request.stream;

        let candidates = self.candidates(std::time::Instant::now());
        let decision = self
            .router
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

        self.dispatch(&request, &decision, emit, record)
    }

    /// Tries the decision's targets in order; moves on only while the error
    /// says another upstream is worth trying, and only before first byte out.
    fn dispatch(
        &self,
        request: &ChatRequest,
        decision: &Decision,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<(), ErrorEnvelope> {
        let mut last_error = None;

        for target in std::iter::once(&decision.chosen).chain(&decision.fallbacks) {
            record.attempts += 1;
            if let Some(routing) = record.routing.as_mut() {
                // The record names who actually served (or last refused), not
                // only who was chosen first.
                target.upstream.as_str().clone_into(&mut routing.upstream);
                routing.model.clone_from(&target.model);
            }
            match self.try_upstream(request, target, emit, record) {
                Ok(()) => {
                    self.observe(&target.upstream, Ok(()));
                    record.status = 200;
                    return Ok(());
                }
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

    /// One upstream attempt: build, authorize, inject, send, translate back.
    fn try_upstream(
        &self,
        request: &ChatRequest,
        target: &UpstreamModel,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<(), ErrorEnvelope> {
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
            self.relay_stream(upstream, response, emit, record)
        } else {
            let parts: HttpResponseParts = response.into();
            let chat_response = upstream.plugin.parse_response(&parts)?;
            record.usage = Some(chat_response.usage);
            let rendered = self.agent.render_response(&chat_response, &Value::Null)?;
            emit(Reply::BeginJson(JsonReply {
                status: 200,
                body: rendered.to_string(),
            }));
            Ok(())
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
        let auth_header = descriptor
            .auth
            .as_ref()
            .map(|auth| self.resolve_auth(auth, upstream_name))
            .transpose()?;

        let sent = match descriptor.method {
            HttpMethod::Get => {
                let mut request = self.http.get(&descriptor.url);
                for (name, value) in descriptor.headers.iter() {
                    request = request.header(name, value);
                }
                if let Some((header, value)) = &auth_header {
                    request = request.header(header, value);
                }
                request.call()
            }
            HttpMethod::Post => {
                let mut request = self.http.post(&descriptor.url);
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
        &self,
        upstream: &Upstream,
        response: UpstreamResponse,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<(), ErrorEnvelope> {
        let mut parser = upstream.plugin.stream_parser();
        let mut reader = response.reader;

        if !emit(Reply::BeginStream) {
            return Ok(());
        }

        let mut buffer = [0u8; STREAM_READ];
        loop {
            let read = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) => {
                    // Mid-stream failure: already committed to SSE, so it goes
                    // out as a rendered error event rather than a status code —
                    // and into the record, where status 200 alone would lie.
                    let envelope = ErrorEnvelope::new(
                        ErrorCode::UpstreamUnavailable,
                        502,
                        format!("upstream stream broke: {error}"),
                    );
                    record.error_code = Some(envelope.code);
                    let rendered = self.render_stream_error(&envelope);
                    emit(Reply::Chunk(rendered));
                    return Ok(());
                }
            };

            let data = String::from_utf8_lossy(&buffer[..read]).into_owned();
            let events = match parser.parse_chunk(&StreamChunk { data }) {
                Ok(events) => events,
                Err(envelope) => {
                    record.error_code = Some(envelope.code);
                    let rendered = self.render_stream_error(&envelope);
                    emit(Reply::Chunk(rendered));
                    return Ok(());
                }
            };

            for event in events {
                if let token_station_protocol::StreamEvent::Usage { usage } = &event {
                    record.usage = Some(*usage);
                }
                let chunk = self.agent.render_stream_event(&event, &Value::Null)?;
                let data = chunk
                    .get("data")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if !emit(Reply::Chunk(data)) {
                    return Ok(()); // Client hung up; drop the upstream too.
                }
            }
        }

        Ok(())
    }

    /// An error, rendered the way this deployment's inbound protocol spells it.
    fn render_error(&self, envelope: &ErrorEnvelope) -> JsonReply {
        let body = self
            .agent
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

    fn render_stream_error(&self, envelope: &ErrorEnvelope) -> String {
        let body = self
            .agent
            .map_inbound_error(envelope, &Value::Null)
            .map_or_else(
                |_| json!({"error": {"message": envelope.message}}).to_string(),
                |value| value.to_string(),
            );
        format!("data: {body}\n\n")
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

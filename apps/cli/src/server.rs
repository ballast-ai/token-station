//! The async facade: axum frames the HTTP, the gateway does the work.
//!
//! Every request body is handed to the synchronous pipeline on a blocking
//! thread; replies come back over a channel. The first message decides the
//! response shape — a whole JSON reply, or the start of an SSE stream whose
//! chunks follow on the same channel. Async stops here; nothing below this
//! module awaits.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc, watch};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use std::time::Duration;

use crate::admin::AdminContext;
use crate::cancel::CancelToken;
use crate::gateway::{Gateway, MAX_INBOUND_BODY, Reply};
use crate::request_context::RequestContext;
use crate::virtual_key;

/// How many rendered SSE chunks may sit between the worker and a slow client
/// before the worker blocks — which in turn stops reading the upstream:
/// backpressure end to end.
const STREAM_BACKLOG: usize = 32;

/// Default overall budget and per-attempt cap for a supervised request.
const REQUEST_DEADLINE: Duration = Duration::from_secs(600);
const PER_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(120);

struct ServerControlInner {
    drain: CancelToken,
    stop_accepting: watch::Sender<bool>,
    in_flight: AtomicUsize,
}

/// Lifecycle handle shared by the listener owner and every request handler.
/// Stopping accepts is separate from cancelling work so a supervisor can grant
/// old requests a bounded grace period during a live configuration handoff.
#[derive(Clone)]
pub struct ServerControl {
    inner: Arc<ServerControlInner>,
}

impl Default for ServerControl {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerControl {
    #[must_use]
    pub fn new() -> Self {
        let (stop_accepting, _) = watch::channel(false);
        Self {
            inner: Arc::new(ServerControlInner {
                drain: CancelToken::root(),
                stop_accepting,
                in_flight: AtomicUsize::new(0),
            }),
        }
    }

    /// Releases the listener while allowing existing requests to finish.
    pub fn stop_accepting(&self) {
        self.inner.stop_accepting.send_replace(true);
    }

    /// Cancels every request context created under this server instance.
    pub fn cancel_in_flight(&self) {
        self.inner.drain.cancel();
    }

    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.inner.in_flight.load(Ordering::SeqCst)
    }

    fn request_context(&self) -> RequestContext {
        RequestContext::new(&self.inner.drain, REQUEST_DEADLINE, PER_ATTEMPT_TIMEOUT)
    }

    fn begin_request(&self) -> InFlightGuard {
        self.inner.in_flight.fetch_add(1, Ordering::SeqCst);
        InFlightGuard {
            inner: Arc::clone(&self.inner),
        }
    }

    async fn wait_for_stop(&self) {
        let mut receiver = self.inner.stop_accepting.subscribe();
        if *receiver.borrow() {
            return;
        }
        while receiver.changed().await.is_ok() {
            if *receiver.borrow() {
                return;
            }
        }
    }
}

struct InFlightGuard {
    inner: Arc<ServerControlInner>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.inner.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

struct CancelOnDrop(CancelToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Everything a handler needs: the data plane, and the door key.
#[derive(Clone)]
pub struct AppState {
    pub gateway: Arc<Gateway>,
    /// `None` when the operator turned inbound auth off.
    pub virtual_key: Option<Arc<str>>,
    /// The read-only `/admin/*` data plane: a snapshot of the running config.
    pub admin: Arc<AdminContext>,
    /// The immutable Desktop Runtime revision this server instance published.
    /// Standalone CLI serve has no Desktop revision ledger and keeps `None`.
    pub running_revision: Option<u64>,
    /// Server-owned accept/drain/request lifecycle.
    pub control: ServerControl,
    /// Bounds blocking workers before request bodies are consumed.
    worker_slots: Arc<Semaphore>,
}

impl AppState {
    #[must_use]
    pub fn new(
        gateway: Arc<Gateway>,
        virtual_key: Option<Arc<str>>,
        admin: Arc<AdminContext>,
    ) -> Self {
        let worker_limit = gateway.global_concurrency_limit();
        Self {
            gateway,
            virtual_key,
            admin,
            running_revision: None,
            control: ServerControl::new(),
            worker_slots: Arc::new(Semaphore::new(worker_limit)),
        }
    }

    #[must_use]
    pub const fn with_running_revision(mut self, revision: u64) -> Self {
        self.running_revision = Some(revision);
        self
    }
}

/// Serves until `ctrl_c`.
///
/// Takes the already-bound listener rather than an address so the caller (and
/// the tests) decide the port and learn it before anything serves.
///
/// # Errors
///
/// Only from the accept loop itself; per-request failures are responses.
pub async fn serve(state: AppState, listener: TcpListener) -> std::io::Result<()> {
    let control = state.control.clone();
    // `/v1/models` stays an explicit GET; everything else is a fallback so any
    // inbound path an adapter might claim (OpenAI `/v1/chat/completions`,
    // Anthropic `/v1/messages`, …) reaches the gateway, which asks each
    // adapter's `match_inbound` who owns it. The host enumerates no protocol
    // paths — adding an inbound protocol is zero change here.
    let app = Router::new()
        .route("/v1/models", get(models))
        // The read-only data plane. Explicit routes, not a nest: these
        // endpoints are a surface small enough to enumerate, and enumerating
        // them keeps `/admin/anything-else` falling through to the gateway's
        // 404 rather than growing an implicit namespace.
        .route("/admin/stats", get(admin_stats).options(admin_preflight))
        .route(
            "/admin/receipts",
            get(admin_receipts).options(admin_preflight),
        )
        .route(
            "/admin/router-table",
            get(admin_router_table).options(admin_preflight),
        )
        .route(
            "/admin/plugins",
            get(admin_plugins).options(admin_preflight),
        )
        .route("/admin/egress", get(admin_egress).options(admin_preflight))
        .fallback(chat)
        .with_state(state);

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                () = control.wait_for_stop() => {}
            }
        })
        .await
}

/// The inbound gate. Loopback keeps the network out; this keeps out every
/// other process on the machine that can open a socket to 127.0.0.1.
/// Clients place the virtual key differently, so extract every supported candidate from the request:
/// - OpenAI/Anthropic:`Authorization: Bearer <key>`
/// - Native Gemini (Gemini CLI): `x-goog-api-key: <key>` header or `?key=<key>` query parameter
///
/// Bearer-only authentication would ignore Gemini keys and report them missing. Kept pure for testing.
fn presented_virtual_keys<'a>(headers: &'a HeaderMap, query: Option<&'a str>) -> Vec<&'a str> {
    let mut keys = Vec::new();
    if let Some(bearer) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    {
        keys.push(bearer);
    }
    if let Some(goog) = headers
        .get("x-goog-api-key")
        .and_then(|value| value.to_str().ok())
    {
        keys.push(goog);
    }
    for presented in query
        .into_iter()
        .flat_map(|query| query.split('&'))
        .filter_map(|pair| pair.strip_prefix("key="))
    {
        keys.push(presented);
    }
    keys
}

/// The inbound gate. Loopback keeps the network out; this keeps out every
/// other process on the machine that can open a socket to 127.0.0.1.
fn key_admitted(state: &AppState, headers: &HeaderMap, query: Option<&str>) -> bool {
    let Some(expected) = &state.virtual_key else {
        return true; // The operator switched auth off.
    };
    presented_virtual_keys(headers, query)
        .into_iter()
        .any(|presented| virtual_key::matches(presented, expected))
}

fn admitted(state: &AppState, headers: &HeaderMap) -> bool {
    key_admitted(state, headers, None)
}

fn unauthorized(path: &str) -> Response {
    let body = match path {
        "/v1/messages" => {
            r#"{"type":"error","error":{"type":"authentication_error","message":"missing or invalid local virtual key"}}"#
        }
        "/v1/responses" => {
            r#"{"error":{"type":"error","code":"authentication_error","message":"missing or invalid local virtual key"}}"#
        }
        _ => {
            r#"{"error":{"message":"missing or invalid local virtual key","type":"auth","code":"auth"}}"#
        }
    };
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("a literal response builds")
}

async fn models(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !admitted(&state, &headers) {
        return unauthorized("/v1/models");
    }
    models_response(&state)
}

fn models_response(state: &AppState) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(state.gateway.models().to_owned()))
        .expect("a literal response builds")
}

#[derive(Debug, PartialEq, Eq)]
struct ScopedInboundPath<'a> {
    agent_id: Option<&'a str>,
    canonical_path: &'a str,
}

fn parse_inbound_path(path: &str) -> Result<ScopedInboundPath<'_>, String> {
    let Some(rest) = path.strip_prefix("/agents/") else {
        return Ok(ScopedInboundPath {
            agent_id: None,
            canonical_path: path,
        });
    };
    let Some((agent_id, _suffix)) = rest.split_once('/') else {
        return Err("Agent namespace is missing a protocol path".to_owned());
    };
    if !crate::config::ClientConfig::is_valid_agent_id(agent_id) {
        return Err(format!("unknown Agent namespace `{agent_id}`"));
    }
    let canonical_path = &rest[agent_id.len()..];
    if !(canonical_path.starts_with("/v1/") || canonical_path.starts_with("/v1beta/"))
        || canonical_path.contains("//")
        || canonical_path.contains('%')
    {
        return Err("Agent namespace has an invalid protocol path".to_owned());
    }
    Ok(ScopedInboundPath {
        agent_id: Some(agent_id),
        canonical_path,
    })
}

fn invalid_namespace(detail: &str) -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({
                "error": {
                    "message": detail,
                    "type": "invalid_request",
                    "code": "invalid_request"
                }
            })
            .to_string(),
        ))
        .expect("a json response builds")
}

fn bounded_refusal(path: &str, status: StatusCode, code: &str, message: &str) -> Response {
    let body = match path {
        "/v1/messages" => serde_json::json!({
            "type": "error",
            "error": { "type": code, "message": message }
        }),
        _ => serde_json::json!({
            "error": { "message": message, "type": code, "code": code }
        }),
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("a json response builds")
}

fn concurrency_refusal(path: &str) -> Response {
    bounded_refusal(
        path,
        StatusCode::TOO_MANY_REQUESTS,
        "concurrency_limit",
        "the local proxy is at its configured concurrency limit",
    )
}

fn payload_too_large(path: &str) -> Response {
    bounded_refusal(
        path,
        StatusCode::PAYLOAD_TOO_LARGE,
        "invalid_request",
        "request body exceeds the local proxy's limit",
    )
}

/// The CORS allowance for `/admin/*`: echo the origin back — but only a
/// loopback one. The data plane exists for local UIs (a browser running the
/// frontend dev server, the desktop shell); a web origin never qualifies, so
/// anything else gets no CORS headers and the browser refuses the response.
fn loopback_origin(headers: &HeaderMap) -> Option<String> {
    let origin = headers.get(header::ORIGIN)?.to_str().ok()?;
    let host = origin.strip_prefix("http://")?;
    let host = host.rsplit_once(':').map_or(host, |(name, _port)| name);
    (host == "localhost" || host == "127.0.0.1").then(|| origin.to_owned())
}

fn with_cors(mut response: Response, origin: Option<String>) -> Response {
    if let Some(origin) = origin {
        if let Ok(value) = origin.parse() {
            let headers = response.headers_mut();
            headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                header::HeaderValue::from_static("authorization"),
            );
        }
    }
    response
}

/// Answers the browser's preflight before auth on purpose: a preflight
/// carries no `Authorization` header by design, exposes no data, and refusing
/// it would only break the browser path while any non-browser client skips
/// preflights entirely.
async fn admin_preflight(headers: HeaderMap) -> Response {
    let response = Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .expect("a literal response builds");
    with_cors(response, loopback_origin(&headers))
}

fn admin_reply(result: Result<serde_json::Value, String>, origin: Option<String>) -> Response {
    let response = match result {
        Ok(view) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(view.to_string()))
            .expect("a json body builds"),
        Err(detail) => Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({"error": {"message": detail, "type": "invalid_request"}})
                    .to_string(),
            ))
            .expect("a json body builds"),
    };
    with_cors(response, origin)
}

#[derive(Debug, Default, Deserialize)]
struct AdminStatsQuery {
    #[serde(default = "default_stats_since")]
    since: String,
    by: Option<String>,
    agent: Option<String>,
    source: Option<String>,
    upstream: Option<String>,
    model: Option<String>,
}

fn default_stats_since() -> String {
    "all".to_owned()
}

async fn admin_stats(
    State(state): State<AppState>,
    Query(query): Query<AdminStatsQuery>,
    headers: HeaderMap,
) -> Response {
    if !admitted(&state, &headers) {
        return with_cors(unauthorized("/admin/stats"), loopback_origin(&headers));
    }
    let origin = loopback_origin(&headers);
    let admin = Arc::clone(&state.admin);
    let result = tokio::task::spawn_blocking(move || {
        admin.stats(
            &query.since,
            query.by.as_deref(),
            query.agent.as_deref(),
            query.source.as_deref(),
            query.upstream.as_deref(),
            query.model.as_deref(),
        )
    })
    .await
    .unwrap_or_else(|_| Err("admin stats worker failed".to_owned()));
    admin_reply(result, origin)
}

async fn admin_receipts(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !admitted(&state, &headers) {
        return with_cors(unauthorized("/admin/receipts"), loopback_origin(&headers));
    }
    let origin = loopback_origin(&headers);
    let admin = Arc::clone(&state.admin);
    let result = tokio::task::spawn_blocking(move || admin.recent_receipts())
        .await
        .unwrap_or_else(|_| Err("admin receipts worker failed".to_owned()));
    admin_reply(result, origin)
}

async fn admin_router_table(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !admitted(&state, &headers) {
        return with_cors(
            unauthorized("/admin/router-table"),
            loopback_origin(&headers),
        );
    }
    admin_reply(Ok(state.admin.router_table()), loopback_origin(&headers))
}

async fn admin_plugins(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !admitted(&state, &headers) {
        return with_cors(unauthorized("/admin/plugins"), loopback_origin(&headers));
    }
    admin_reply(state.admin.plugins(), loopback_origin(&headers))
}

async fn admin_egress(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !admitted(&state, &headers) {
        return with_cors(unauthorized("/admin/egress"), loopback_origin(&headers));
    }
    admin_reply(Ok(state.gateway.egress_routes()), loopback_origin(&headers))
}

async fn chat(State(state): State<AppState>, request: Request<Body>) -> Response {
    let (parts, body) = request.into_parts();
    let method = parts.method;
    let uri = parts.uri;
    let headers = parts.headers;
    let scoped = match parse_inbound_path(uri.path()) {
        Ok(scoped) => scoped,
        Err(detail) => return invalid_namespace(&detail),
    };
    if !key_admitted(&state, &headers, uri.query()) {
        return unauthorized(scoped.canonical_path);
    }
    if method == Method::GET && scoped.canonical_path == "/v1/models" {
        return models_response(&state);
    }
    let Ok(worker_permit) = Arc::clone(&state.worker_slots).try_acquire_owned() else {
        return concurrency_refusal(scoped.canonical_path);
    };
    let Ok(body) = to_bytes(body, MAX_INBOUND_BODY).await else {
        return payload_too_large(scoped.canonical_path);
    };
    let gateway = Arc::clone(&state.gateway);
    // The method and path feed the gateway's `match_inbound` step, which picks
    // the inbound adapter. Owned copies cross into the blocking thread.
    let method = method.as_str().to_owned();
    let path = scoped.canonical_path.to_owned();
    let agent_id = scoped.agent_id.map(str::to_owned);
    let running_revision = state.running_revision;
    // Owned copies for the blocking thread. Values that are not UTF-8 keep
    // their name and lose their value — same rule HeaderDigest applies.
    let headers: Vec<(String, String)> = headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                value.to_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();

    let (tx, mut rx) = mpsc::channel::<Reply>(STREAM_BACKLOG);

    // The pipeline owns its thread for the whole exchange; `blocking_send`
    // makes a slow reader slow the upstream read down, not buffer it.
    //
    let ctx = state.control.request_context();
    let mut cancel_on_drop = Some(CancelOnDrop(ctx.token()));
    let in_flight = state.control.begin_request();
    let worker = tokio::task::spawn_blocking(move || {
        let _worker_permit = worker_permit;
        let _in_flight = in_flight;
        gateway.chat_scoped(
            &ctx,
            agent_id.as_deref(),
            running_revision,
            &method,
            &path,
            &headers,
            &body,
            &mut |reply| tx.blocking_send(reply).is_ok(),
        );
    });

    let first = rx.recv().await;
    match first {
        Some(Reply::BeginJson(reply)) => {
            drop(worker);
            Response::builder()
                .status(StatusCode::from_u16(reply.status).unwrap_or(StatusCode::BAD_GATEWAY))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(reply.body))
                .expect("a rendered response builds")
        }
        Some(Reply::BeginStream) => {
            let cancel_on_drop = cancel_on_drop
                .take()
                .expect("request cancellation guard is present");
            let chunks = ReceiverStream::new(rx).map(move |reply| {
                let _keep_guard_alive = &cancel_on_drop;
                Ok::<Bytes, Infallible>(match reply {
                    Reply::Chunk(data) => Bytes::from(data),
                    // A second Begin* would be a pipeline bug; starve it
                    // rather than corrupt the stream.
                    Reply::BeginJson(_) | Reply::BeginStream => Bytes::new(),
                })
            });
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Body::from_stream(chunks))
                .expect("a stream response builds")
        }
        // The worker died before answering; its panic is in the logs.
        Some(Reply::Chunk(_)) | None => Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"error":{"message":"the request pipeline failed to answer","type":"internal"}}"#,
            ))
            .expect("a literal response builds"),
    }
}

#[cfg(test)]
mod tests {
    use super::{ScopedInboundPath, ServerControl, parse_inbound_path, presented_virtual_keys};
    use axum::http::HeaderMap;

    #[test]
    fn presented_virtual_keys_reads_bearer_goog_header_and_query() {
        // Bearer(OpenAI/Anthropic)。
        let mut bearer = HeaderMap::new();
        bearer.insert("authorization", "Bearer vk-abc".parse().unwrap());
        assert_eq!(presented_virtual_keys(&bearer, None), vec!["vk-abc"]);

        // Native Gemini x-goog-api-key header.
        let mut goog = HeaderMap::new();
        goog.insert("x-goog-api-key", "vk-gem".parse().unwrap());
        assert_eq!(presented_virtual_keys(&goog, None), vec!["vk-gem"]);

        // Native Gemini fallback: a ?key= query parameter among other parameters.
        assert_eq!(
            presented_virtual_keys(&HeaderMap::new(), Some("alt=1&key=vk-q&x=2")),
            vec!["vk-q"]
        );

        // Collect all three when present; any matching candidate grants access.
        let mut all = HeaderMap::new();
        all.insert("authorization", "Bearer vk-b".parse().unwrap());
        all.insert("x-goog-api-key", "vk-g".parse().unwrap());
        assert_eq!(
            presented_virtual_keys(&all, Some("key=vk-q")),
            vec!["vk-b", "vk-g", "vk-q"]
        );

        // Return an empty list when no key is present.
        assert!(presented_virtual_keys(&HeaderMap::new(), None).is_empty());
        assert!(presented_virtual_keys(&HeaderMap::new(), Some("model=x")).is_empty());
    }

    #[test]
    fn accept_stop_and_request_drain_are_separate_lifecycle_steps() {
        let control = ServerControl::new();
        let context = control.request_context();
        let request = control.begin_request();
        assert_eq!(control.in_flight(), 1);
        assert!(!context.is_cancelled());

        control.stop_accepting();
        assert!(!context.is_cancelled());
        control.cancel_in_flight();
        assert!(context.is_cancelled());

        drop(request);
        assert_eq!(control.in_flight(), 0);
    }

    #[test]
    fn agent_namespaces_are_stripped_to_canonical_protocol_paths() {
        let cases = [
            ("claude-code", "/v1/messages"),
            ("codex", "/v1/responses"),
            ("opencode", "/v1/chat/completions"),
            ("openclaw", "/v1/chat/completions"),
            ("nous-hermes-agent", "/v1/chat/completions"),
            (
                "gemini-cli",
                "/v1beta/models/gemini-2.5-pro:generateContent",
            ),
        ];
        for (agent_id, canonical_path) in cases {
            let path = format!("/agents/{agent_id}{canonical_path}");
            assert_eq!(
                parse_inbound_path(&path),
                Ok(ScopedInboundPath {
                    agent_id: Some(agent_id),
                    canonical_path,
                })
            );
        }
    }

    #[test]
    fn unnamespaced_paths_remain_backward_compatible() {
        assert_eq!(
            parse_inbound_path("/v1/responses"),
            Ok(ScopedInboundPath {
                agent_id: None,
                canonical_path: "/v1/responses",
            })
        );
    }

    #[test]
    fn future_agent_ids_are_accepted_but_malformed_namespaces_fail_closed() {
        assert_eq!(
            parse_inbound_path("/agents/future/v1/responses"),
            Ok(ScopedInboundPath {
                agent_id: Some("future"),
                canonical_path: "/v1/responses",
            })
        );
        for path in [
            "/agents/codex",
            "/agents//v1/responses",
            "/agents/codex//v1/responses",
            "/agents/codex/v2/responses",
            "/agents/codex/v1/%72esponses",
        ] {
            assert!(parse_inbound_path(path).is_err(), "{path}");
        }
    }
}

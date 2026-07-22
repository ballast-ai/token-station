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
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::Response;
use axum::routing::get;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use std::time::Duration;

use crate::admin::AdminContext;
use crate::cancel::CancelToken;
use crate::gateway::{Gateway, Reply};
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
}

impl AppState {
    #[must_use]
    pub fn new(
        gateway: Arc<Gateway>,
        virtual_key: Option<Arc<str>>,
        admin: Arc<AdminContext>,
    ) -> Self {
        Self {
            gateway,
            virtual_key,
            admin,
            running_revision: None,
            control: ServerControl::new(),
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
fn admitted(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(expected) = &state.virtual_key else {
        return true; // The operator switched auth off.
    };
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|presented| virtual_key::matches(presented, expected))
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
    if !crate::config::ClientConfig::is_known_agent_id(agent_id) {
        return Err(format!("unknown Agent namespace `{agent_id}`"));
    }
    let canonical_path = &rest[agent_id.len()..];
    if !canonical_path.starts_with("/v1/")
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

async fn admin_stats(State(state): State<AppState>, uri: Uri, headers: HeaderMap) -> Response {
    if !admitted(&state, &headers) {
        return with_cors(unauthorized("/admin/stats"), loopback_origin(&headers));
    }
    // `since` and `by` are single tokens (`all`, `24h`, `upstream`, …), so a
    // split on `&`/`=` is a full parser here — no percent-decoding to get wrong.
    let mut since = "all".to_owned();
    let mut by = None;
    for pair in uri.query().unwrap_or_default().split('&') {
        match pair.split_once('=') {
            Some(("since", value)) if !value.is_empty() => value.clone_into(&mut since),
            Some(("by", value)) if !value.is_empty() => by = Some(value.to_owned()),
            _ => {}
        }
    }
    admin_reply(
        state.admin.stats(&since, by.as_deref()),
        loopback_origin(&headers),
    )
}

async fn admin_receipts(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !admitted(&state, &headers) {
        return with_cors(unauthorized("/admin/receipts"), loopback_origin(&headers));
    }
    admin_reply(state.admin.recent_receipts(), loopback_origin(&headers))
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

async fn chat(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let scoped = match parse_inbound_path(uri.path()) {
        Ok(scoped) => scoped,
        Err(detail) => return invalid_namespace(&detail),
    };
    if !admitted(&state, &headers) {
        return unauthorized(scoped.canonical_path);
    }
    if method == Method::GET && scoped.canonical_path == "/v1/models" {
        return models_response(&state);
    }
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
    use super::{ScopedInboundPath, ServerControl, parse_inbound_path};

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
    fn malformed_or_unknown_agent_namespaces_fail_closed() {
        for path in [
            "/agents/codex",
            "/agents/future/v1/responses",
            "/agents//v1/responses",
            "/agents/codex//v1/responses",
            "/agents/codex/v2/responses",
            "/agents/codex/v1/%72esponses",
        ] {
            assert!(parse_inbound_path(path).is_err(), "{path}");
        }
    }
}

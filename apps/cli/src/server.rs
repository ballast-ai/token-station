//! The async facade: axum frames the HTTP, the gateway does the work.
//!
//! Every request body is handed to the synchronous pipeline on a blocking
//! thread; replies come back over a channel. The first message decides the
//! response shape — a whole JSON reply, or the start of an SSE stream whose
//! chunks follow on the same channel. Async stops here; nothing below this
//! module awaits.

use std::convert::Infallible;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::Response;
use axum::routing::{get, post};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use crate::gateway::{Gateway, Reply};
use crate::virtual_key;

/// How many rendered SSE chunks may sit between the worker and a slow client
/// before the worker blocks — which in turn stops reading the upstream:
/// backpressure end to end.
const STREAM_BACKLOG: usize = 32;

/// Everything a handler needs: the data plane, and the door key.
#[derive(Clone)]
pub struct AppState {
    pub gateway: Arc<Gateway>,
    /// `None` when the operator turned inbound auth off.
    pub virtual_key: Option<Arc<str>>,
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
    let app = Router::new()
        .route("/v1/chat/completions", post(chat))
        .route("/v1/messages", post(chat))
        .route("/v1/models", get(models))
        .with_state(state);

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
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
    let body = if path == "/v1/messages" {
        r#"{"type":"error","error":{"type":"authentication_error","message":"missing or invalid local virtual key"}}"#
    } else {
        r#"{"error":{"message":"missing or invalid local virtual key","type":"auth","code":"auth"}}"#
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
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(state.gateway.models().to_owned()))
        .expect("a literal response builds")
}

async fn chat(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !admitted(&state, &headers) {
        return unauthorized(uri.path());
    }
    let gateway = Arc::clone(&state.gateway);
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
    let path = uri.path().to_owned();
    let worker = tokio::task::spawn_blocking(move || {
        gateway.chat("POST", &path, &headers, &body, &mut |reply| {
            tx.blocking_send(reply).is_ok()
        });
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
            let chunks = ReceiverStream::new(rx).map(|reply| {
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

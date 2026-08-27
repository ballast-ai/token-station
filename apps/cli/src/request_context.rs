//! Per-request lifecycle: the cancel signal, the overall deadline and the
//! per-attempt timeout that let the blocking pipeline stop instead of running
//! an abandoned request to completion.
//!
//! The server layer builds one of these per request as a child of the running
//! server's drain token, then hands it to [`crate::gateway::Gateway::chat`]. The
//! gateway consumes it: it polls [`is_cancelled`](RequestContext::is_cancelled)
//! between upstream reads, so a client that hangs up (or a drain that fires)
//! stops the exchange rather than paying for output nobody will read.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::bodylog::{
    HttpHeaderSnapshot, HttpRequestSnapshot, HttpResponseSnapshot, HttpTraceSnapshot,
    MAX_BODY_BYTES, UpstreamHttpExchange,
};
use crate::cancel::{CancelReason, CancelToken};
use token_station_protocol::{Auth, HttpRequestDescriptor};

/// The lifetime and stop conditions of a single in-flight request.
pub struct RequestContext {
    cancel: CancelToken,
    deadline: Instant,
    per_attempt_timeout: Duration,
    upstream_response_limit: Option<u64>,
    http_trace: Mutex<Option<HttpTraceCapture>>,
}

const MAX_HTTP_TRACE_BODY_BYTES: usize = 1024 * 1024;

struct HttpTraceCapture {
    snapshot: HttpTraceSnapshot,
    remaining_body_bytes: usize,
}

struct CappedJsonWriter {
    bytes: Vec<u8>,
    limit: usize,
    truncated: bool,
}

impl CappedJsonWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(8 * 1024)),
            limit,
            truncated: false,
        }
    }
}

impl Write for CappedJsonWriter {
    fn write(&mut self, value: &[u8]) -> io::Result<usize> {
        let remaining = self.limit.saturating_sub(self.bytes.len());
        if value.len() <= remaining {
            self.bytes.extend_from_slice(value);
            return Ok(value.len());
        }
        self.bytes.extend_from_slice(&value[..remaining]);
        self.truncated = true;
        Err(io::Error::other("HTTP trace body limit reached"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl HttpTraceCapture {
    fn new() -> Self {
        Self {
            snapshot: HttpTraceSnapshot::default(),
            remaining_body_bytes: MAX_HTTP_TRACE_BODY_BYTES,
        }
    }

    fn take_body(&mut self, value: &[u8]) -> (String, bool) {
        let mut body = String::new();
        let mut truncated = false;
        Self::append_body(
            &mut self.remaining_body_bytes,
            &mut body,
            &mut truncated,
            value,
        );
        (body, truncated)
    }

    fn append_body(
        remaining_body_bytes: &mut usize,
        body: &mut String,
        truncated: &mut bool,
        value: &[u8],
    ) {
        if *truncated || value.is_empty() {
            return;
        }
        let value = String::from_utf8_lossy(value);
        let allowed = MAX_BODY_BYTES
            .saturating_sub(body.len())
            .min(*remaining_body_bytes);
        let mut boundary = allowed.min(value.len());
        while boundary > 0 && !value.is_char_boundary(boundary) {
            boundary -= 1;
        }
        body.push_str(&value[..boundary]);
        *remaining_body_bytes = remaining_body_bytes.saturating_sub(boundary);
        if boundary < value.len() {
            *truncated = true;
        }
    }
}

impl RequestContext {
    /// Build a request-scoped context under a server's drain token.
    #[must_use]
    pub fn new(drain: &CancelToken, total: Duration, per_attempt: Duration) -> Self {
        Self {
            cancel: drain.child(),
            deadline: Instant::now() + total,
            per_attempt_timeout: per_attempt,
            upstream_response_limit: None,
            http_trace: Mutex::new(None),
        }
    }

    /// A standalone context with no drain parent — for tests and callers that do
    /// not (yet) run under a supervised server.
    #[must_use]
    pub fn detached(total: Duration, per_attempt: Duration) -> Self {
        Self::new(&CancelToken::root(), total, per_attempt)
    }

    /// Applies a caller-specific raw upstream response limit before Provider parsing.
    ///
    /// # Panics
    ///
    /// Panics when `bytes` is zero.
    #[must_use]
    pub fn with_upstream_response_limit(mut self, bytes: u64) -> Self {
        assert!(bytes > 0, "upstream response limit must be positive");
        self.upstream_response_limit = Some(bytes);
        self
    }

    /// Returns the caller-specific raw upstream response limit, when present.
    #[must_use]
    pub const fn upstream_response_limit(&self) -> Option<u64> {
        self.upstream_response_limit
    }

    /// Trip this request's cancel — the client disconnected.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// True once the client hung up, the drain fired, or the deadline passed.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel_reason().is_some()
    }

    /// The exact lifecycle reason, including a deadline that is derived from
    /// the monotonic clock rather than written into the cancellation token.
    #[must_use]
    pub fn cancel_reason(&self) -> Option<CancelReason> {
        self.cancel
            .cancel_reason()
            .or_else(|| self.remaining().is_zero().then_some(CancelReason::Deadline))
    }

    /// Time left before the overall deadline (zero once it has passed).
    #[must_use]
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// The cap on a single upstream attempt.
    #[must_use]
    pub fn per_attempt_timeout(&self) -> Duration {
        self.per_attempt_timeout
    }

    /// The immutable overall request deadline.
    #[must_use]
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Freezes the current attempt's absolute deadline from one clock sample.
    #[must_use]
    pub fn attempt_deadline(&self) -> Instant {
        self.attempt_deadline_for(self.per_attempt_timeout)
    }

    /// Freezes an absolute deadline for a caller-supplied attempt cap.
    #[must_use]
    pub fn attempt_deadline_for(&self, attempt_timeout: Duration) -> Instant {
        let started_at = Instant::now();
        let attempt_cap = started_at
            .checked_add(attempt_timeout)
            .unwrap_or(self.deadline);
        self.deadline.min(attempt_cap)
    }

    /// A handle to hand an upstream client for abort/read-timeout slicing.
    #[must_use]
    pub fn token(&self) -> CancelToken {
        self.cancel.clone()
    }

    pub(crate) fn enable_http_trace(&self) {
        *self.http_trace.lock().unwrap() = Some(HttpTraceCapture::new());
    }

    fn captured_headers<'a>(
        headers: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Vec<HttpHeaderSnapshot> {
        const SAFE_VALUE_HEADERS: [&str; 8] = [
            "accept",
            "accept-encoding",
            "anthropic-beta",
            "anthropic-version",
            "cache-control",
            "content-length",
            "content-type",
            "user-agent",
        ];
        headers
            .into_iter()
            .map(|(name, value)| {
                let redacted = !SAFE_VALUE_HEADERS
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(name));
                HttpHeaderSnapshot {
                    name: name.to_ascii_lowercase(),
                    value: if redacted {
                        "<redacted>".to_owned()
                    } else {
                        value.to_owned()
                    },
                    redacted,
                }
            })
            .collect()
    }

    pub(crate) fn capture_agent_request(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) {
        let mut trace = self.http_trace.lock().unwrap();
        let Some(trace) = trace.as_mut() else {
            return;
        };
        let (body, body_truncated) = trace.take_body(body);
        let request = HttpRequestSnapshot {
            method: method.to_owned(),
            url: url.to_owned(),
            headers: Self::captured_headers(
                headers
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.as_str())),
            ),
            body,
            body_truncated,
        };
        trace.snapshot.agent_request = Some(request);
    }

    pub(crate) fn capture_upstream_request(
        &self,
        upstream: &str,
        model: &str,
        descriptor: &HttpRequestDescriptor,
    ) {
        let mut headers = descriptor
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        let auth_name = descriptor.auth.as_ref().map(|auth| match auth {
            Auth::Bearer { .. } | Auth::OAuth { .. } => "authorization",
            Auth::Header { name, .. } => name.as_str(),
        });
        if let Some(name) = auth_name {
            headers.push((name, "<redacted>"));
        }
        let mut trace = self.http_trace.lock().unwrap();
        let Some(trace) = trace.as_mut() else {
            return;
        };
        let mut writer = CappedJsonWriter::new(MAX_BODY_BYTES.min(trace.remaining_body_bytes));
        if let Some(value) = descriptor.body.as_ref() {
            let _ = serde_json::to_writer(&mut writer, value);
        }
        let serialization_truncated = writer.truncated;
        let (body, budget_truncated) = trace.take_body(&writer.bytes);
        let body_truncated = serialization_truncated || budget_truncated;
        let ordinal =
            u32::try_from(trace.snapshot.upstream_exchanges.len() + 1).unwrap_or(u32::MAX);
        trace
            .snapshot
            .upstream_exchanges
            .push(UpstreamHttpExchange {
                ordinal,
                upstream: upstream.to_owned(),
                model: model.to_owned(),
                request: HttpRequestSnapshot {
                    method: format!("{:?}", descriptor.method).to_ascii_uppercase(),
                    url: descriptor.url.clone(),
                    headers: Self::captured_headers(headers),
                    body,
                    body_truncated,
                },
                response: None,
            });
    }

    pub(crate) fn capture_upstream_response_head(
        &self,
        status: u16,
        headers: &BTreeMap<String, String>,
    ) {
        let mut trace = self.http_trace.lock().unwrap();
        let Some(trace) = trace.as_mut() else {
            return;
        };
        if let Some(exchange) = trace.snapshot.upstream_exchanges.last_mut() {
            exchange.response = Some(HttpResponseSnapshot {
                status,
                headers: Self::captured_headers(
                    headers
                        .iter()
                        .map(|(name, value)| (name.as_str(), value.as_str())),
                ),
                ..HttpResponseSnapshot::default()
            });
        }
    }

    pub(crate) fn append_upstream_response_body(&self, value: &[u8]) {
        let mut trace = self.http_trace.lock().unwrap();
        let Some(trace) = trace.as_mut() else {
            return;
        };
        let HttpTraceCapture {
            snapshot,
            remaining_body_bytes,
        } = trace;
        let Some(response) = snapshot
            .upstream_exchanges
            .last_mut()
            .and_then(|exchange| exchange.response.as_mut())
        else {
            return;
        };
        HttpTraceCapture::append_body(
            remaining_body_bytes,
            &mut response.body,
            &mut response.body_truncated,
            value,
        );
    }

    pub(crate) fn capture_agent_response_head(&self, status: u16, streaming: bool) {
        let headers = if streaming {
            vec![
                HttpHeaderSnapshot {
                    name: "content-type".to_owned(),
                    value: "text/event-stream".to_owned(),
                    redacted: false,
                },
                HttpHeaderSnapshot {
                    name: "cache-control".to_owned(),
                    value: "no-cache".to_owned(),
                    redacted: false,
                },
            ]
        } else {
            vec![HttpHeaderSnapshot {
                name: "content-type".to_owned(),
                value: "application/json".to_owned(),
                redacted: false,
            }]
        };
        let mut trace = self.http_trace.lock().unwrap();
        let Some(trace) = trace.as_mut() else {
            return;
        };
        trace.snapshot.agent_response = Some(HttpResponseSnapshot {
            status,
            headers,
            ..HttpResponseSnapshot::default()
        });
    }

    pub(crate) fn append_agent_response_body(&self, value: &str) {
        let mut trace = self.http_trace.lock().unwrap();
        let Some(trace) = trace.as_mut() else {
            return;
        };
        let HttpTraceCapture {
            snapshot,
            remaining_body_bytes,
        } = trace;
        let Some(response) = snapshot.agent_response.as_mut() else {
            return;
        };
        HttpTraceCapture::append_body(
            remaining_body_bytes,
            &mut response.body,
            &mut response.body_truncated,
            value.as_bytes(),
        );
    }

    #[must_use]
    pub(crate) fn http_trace_snapshot(&self) -> HttpTraceSnapshot {
        self.http_trace
            .lock()
            .unwrap()
            .as_ref()
            .map_or_else(HttpTraceSnapshot::default, |trace| trace.snapshot.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{CappedJsonWriter, MAX_HTTP_TRACE_BODY_BYTES, RequestContext};
    use crate::bodylog::{HttpTraceSnapshot, MAX_BODY_BYTES};
    use crate::cancel::{CancelReason, CancelToken};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::time::Duration;
    use token_station_protocol::{Auth, HttpMethod, HttpRequestDescriptor, SafeHeaders, SecretRef};

    #[test]
    fn a_cancelled_client_cancels_the_context() {
        let ctx = RequestContext::detached(Duration::from_mins(1), Duration::from_secs(30));
        assert!(!ctx.is_cancelled());
        ctx.cancel();
        assert!(ctx.is_cancelled());
        assert_eq!(ctx.cancel_reason(), Some(CancelReason::ClientDisconnect));
    }

    #[test]
    fn a_zero_deadline_reads_as_cancelled() {
        let ctx = RequestContext::detached(Duration::ZERO, Duration::from_secs(30));
        assert!(ctx.is_cancelled());
        assert_eq!(ctx.cancel_reason(), Some(CancelReason::Deadline));
    }

    #[test]
    fn a_drain_cascades_into_live_contexts() {
        let drain = CancelToken::root();
        let ctx = RequestContext::new(&drain, Duration::from_mins(1), Duration::from_secs(30));
        assert!(!ctx.is_cancelled());
        drain.cancel_with(CancelReason::ServerDrain);
        assert!(ctx.is_cancelled());
        assert_eq!(ctx.cancel_reason(), Some(CancelReason::ServerDrain));
    }

    #[test]
    fn attempt_deadline_is_capped_by_the_request_and_attempt_budgets() {
        let short_attempt =
            RequestContext::detached(Duration::from_mins(1), Duration::from_secs(10));
        let before = std::time::Instant::now() + Duration::from_secs(9);
        let deadline = short_attempt.attempt_deadline();
        let after = std::time::Instant::now() + Duration::from_secs(11);
        assert!(deadline >= before && deadline <= after);

        let short_request =
            RequestContext::detached(Duration::from_secs(1), Duration::from_mins(1));
        assert!(short_request.attempt_deadline() <= short_request.deadline());
    }

    #[test]
    fn a_scoped_upstream_response_limit_is_explicit_and_immutable() {
        let ctx = RequestContext::detached(Duration::from_secs(1), Duration::from_secs(1))
            .with_upstream_response_limit(4 * 1024 * 1024);

        assert_eq!(ctx.upstream_response_limit(), Some(4 * 1024 * 1024));
    }

    #[test]
    fn http_trace_keeps_packets_but_never_credentials() {
        let ctx = RequestContext::detached(Duration::from_secs(1), Duration::from_secs(1));
        ctx.enable_http_trace();
        ctx.capture_agent_request(
            "POST",
            "/agents/codex/v1/responses",
            &[
                ("authorization".to_owned(), "Bearer local-secret".to_owned()),
                ("user-agent".to_owned(), "codex-cli".to_owned()),
                ("x-auth-token".to_owned(), "custom-secret".to_owned()),
            ],
            br#"{"model":"auto"}"#,
        );
        let mut descriptor =
            HttpRequestDescriptor::new(HttpMethod::Post, "https://api.example/v1/chat/completions");
        descriptor.headers =
            SafeHeaders::try_new([("content-type", "application/json")]).expect("safe header");
        descriptor.body = Some(json!({"model": "model-a"}));
        descriptor.auth = Some(Auth::bearer(SecretRef::new("provider_api_key")));
        ctx.capture_upstream_request("example", "model-a", &descriptor);

        let trace = ctx.http_trace_snapshot();
        let inbound = trace.agent_request.as_ref().expect("agent request");
        assert_eq!(inbound.headers[0].value, "<redacted>");
        assert_eq!(inbound.headers[1].value, "codex-cli");
        assert_eq!(inbound.headers[2].value, "<redacted>");
        let outbound = &trace.upstream_exchanges[0].request;
        assert!(
            outbound
                .headers
                .iter()
                .any(|header| header.name == "authorization" && header.value == "<redacted>")
        );
        assert!(
            !serde_json::to_string(&trace)
                .unwrap()
                .contains("local-secret")
        );
        assert!(
            !serde_json::to_string(&trace)
                .unwrap()
                .contains("custom-secret")
        );
    }

    #[test]
    fn http_trace_is_opt_in_and_stops_copying_after_the_shared_budget() {
        let disabled = RequestContext::detached(Duration::from_secs(1), Duration::from_secs(1));
        disabled.capture_agent_request("POST", "/v1/chat/completions", &[], b"secret");
        assert_eq!(disabled.http_trace_snapshot(), HttpTraceSnapshot::default());

        let ctx = RequestContext::detached(Duration::from_secs(1), Duration::from_secs(1));
        ctx.enable_http_trace();
        ctx.capture_agent_request("POST", "/v1/chat/completions", &[], b"request");
        let descriptor =
            HttpRequestDescriptor::new(HttpMethod::Post, "https://api.example/v1/chat");
        ctx.capture_upstream_request("example", "model-a", &descriptor);
        ctx.capture_upstream_response_head(200, &BTreeMap::new());
        let oversized = vec![b'a'; MAX_HTTP_TRACE_BODY_BYTES + 1];
        ctx.append_upstream_response_body(&oversized);
        let first = ctx.http_trace_snapshot();
        let response = first.upstream_exchanges[0].response.as_ref().unwrap();
        assert!(response.body_truncated);
        assert!(response.body.len() <= MAX_BODY_BYTES);

        ctx.append_upstream_response_body(&vec![b'b'; MAX_BODY_BYTES]);
        assert_eq!(ctx.http_trace_snapshot(), first);

        for attempt in 0..4 {
            let descriptor = HttpRequestDescriptor::new(
                HttpMethod::Post,
                format!("https://api.example/v1/attempt-{attempt}"),
            );
            ctx.capture_upstream_request("example", "model-a", &descriptor);
            ctx.capture_upstream_response_head(200, &BTreeMap::new());
            ctx.append_upstream_response_body(&vec![b'c'; MAX_BODY_BYTES]);
        }
        let complete = ctx.http_trace_snapshot();
        let captured = complete
            .upstream_exchanges
            .iter()
            .filter_map(|exchange| exchange.response.as_ref())
            .map(|response| response.body.len())
            .sum::<usize>();
        assert!(captured <= MAX_HTTP_TRACE_BODY_BYTES);
        assert!(
            complete
                .upstream_exchanges
                .last()
                .and_then(|exchange| exchange.response.as_ref())
                .is_some_and(|response| response.body_truncated)
        );
    }

    #[test]
    fn outbound_json_serialization_stops_at_the_capture_limit() {
        let value = json!({"payload": "x".repeat(MAX_BODY_BYTES * 4)});
        let mut writer = CappedJsonWriter::new(4 * 1024);

        let error = serde_json::to_writer(&mut writer, &value).expect_err("large JSON is capped");

        assert!(error.is_io());
        assert!(writer.truncated);
        assert_eq!(writer.bytes.len(), 4 * 1024);
    }
}

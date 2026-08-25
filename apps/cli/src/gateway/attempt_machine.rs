// Split from gateway.rs — code moved verbatim; see gateway.rs for the module map.
#[allow(clippy::wildcard_imports)]
use super::*;

/// One logical request's fallback limits. Count, wall-clock and per-attempt
/// timeout are always active. Cost is optional until the router has a trusted
/// preflight estimator; when configured, an unknown estimate fails closed.
struct AttemptBudget {
    max_attempts: u32,
    max_elapsed: Duration,
    per_attempt_timeout: Duration,
    max_cost: Option<u64>,
    started: Instant,
    attempts: u32,
    reserved_cost: u64,
}

impl AttemptBudget {
    fn for_request(ctx: &RequestContext) -> Self {
        Self {
            max_attempts: MAX_ATTEMPTS,
            max_elapsed: ctx.remaining(),
            per_attempt_timeout: ctx.per_attempt_timeout(),
            max_cost: None,
            started: Instant::now(),
            attempts: 0,
            reserved_cost: 0,
        }
    }

    fn try_begin(&mut self, estimated_cost: Option<u64>) -> bool {
        if self.attempts >= self.max_attempts || self.remaining().is_zero() {
            return false;
        }
        let reservation = match (self.max_cost, estimated_cost) {
            (Some(_), None) => return false,
            (_, None) => 0,
            (_, Some(cost)) => cost,
        };
        if self
            .max_cost
            .is_some_and(|maximum| self.reserved_cost.saturating_add(reservation) > maximum)
        {
            return false;
        }
        self.attempts += 1;
        self.reserved_cost = self.reserved_cost.saturating_add(reservation);
        true
    }

    fn remaining(&self) -> Duration {
        self.max_elapsed.saturating_sub(self.started.elapsed())
    }

    fn retry_delay(&self, requested: Duration, ctx: &RequestContext) -> Duration {
        requested.min(self.remaining()).min(ctx.remaining())
    }

    const fn has_attempt_remaining(&self) -> bool {
        self.attempts < self.max_attempts
    }
}

fn wait_retry_delay(ctx: &RequestContext, wait: Duration) {
    let deadline = Instant::now() + wait;
    while !ctx.is_cancelled() && Instant::now() < deadline {
        std::thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(20)),
        );
    }
}

fn attempt_receipt(
    target: &UpstreamModel,
    ordinal: u32,
    latency_ms: u64,
    upstream_http_status: Option<u16>,
    provider_call: ProviderCallOutcome,
    result: Result<StreamOutcome, &ErrorEnvelope>,
    record: &RequestRecord,
) -> AttemptRecord {
    match result {
        Ok(outcome) => {
            let error_code = match outcome {
                StreamOutcome::Complete | StreamOutcome::ClientCancelled => None,
                StreamOutcome::FailedAfterPartial | StreamOutcome::FailedBeforeOutput => {
                    record.error_code
                }
            };
            AttemptRecord {
                ordinal,
                upstream: target.upstream.as_str().to_owned(),
                model: target.model.clone(),
                latency_ms,
                http_status: upstream_http_status,
                error_code,
                stream_outcome: Some(outcome),
                provider_call_engine: provider_call.engine,
                south_fallback_reason: provider_call.south_fallback_reason,
                fallback_allowed: matches!(outcome, StreamOutcome::FailedBeforeOutput)
                    && error_code.is_some_and(ErrorCode::is_retriable_elsewhere),
            }
        }
        Err(error) => AttemptRecord {
            ordinal,
            upstream: target.upstream.as_str().to_owned(),
            model: target.model.clone(),
            latency_ms,
            http_status: upstream_http_status,
            error_code: Some(error.code),
            stream_outcome: Some(StreamOutcome::FailedBeforeOutput),
            provider_call_engine: provider_call.engine,
            south_fallback_reason: provider_call.south_fallback_reason,
            fallback_allowed: attempt_fallback_allowed(error),
        },
    }
}

/// Marks a host-owned terminal attempt without changing the public error
/// catalog. Dispatch consumes this private marker before the error can be
/// rendered, persisted or returned to a caller.
fn forbid_attempt_fallback(mut error: ErrorEnvelope) -> ErrorEnvelope {
    error.extensions.insert(
        NO_ATTEMPT_FALLBACK_EXTENSION.to_owned(),
        serde_json::Value::Bool(true),
    );
    error
}

fn attempt_fallback_allowed(error: &ErrorEnvelope) -> bool {
    error.code.is_retriable_elsewhere()
        && !matches!(
            error.extensions.get(NO_ATTEMPT_FALLBACK_EXTENSION),
            Some(serde_json::Value::Bool(true))
        )
}

pub(super) fn sanitize_attempt_error_for_render(error: &mut ErrorEnvelope) {
    error.extensions.remove(NO_ATTEMPT_FALLBACK_EXTENSION);
}

/// Returns the routing decision and erases its host-private carrier.
fn take_attempt_fallback_policy(error: &mut ErrorEnvelope) -> bool {
    let allowed = attempt_fallback_allowed(error);
    sanitize_attempt_error_for_render(error);
    allowed
}

pub(super) fn map_south_stream_failure_for_attempt(
    failure: south_contracts::StreamReadErrorV1,
    cancellation: CancellationDispositionV1,
) -> ErrorEnvelope {
    let error = map_stream_read_failure_v1(failure, cancellation);
    if failure == south_contracts::StreamReadErrorV1::StreamDeadlineExceeded {
        forbid_attempt_fallback(error)
    } else {
        error
    }
}

pub(super) fn buffered_transport_timeout(
    attempt_deadline: Instant,
    now: Instant,
) -> Result<Duration, ErrorEnvelope> {
    let remaining = attempt_deadline.saturating_duration_since(now);
    if remaining.is_zero() {
        return Err(ErrorEnvelope::new(
            ErrorCode::Timeout,
            504,
            "request deadline exceeded",
        ));
    }
    let timeout = remaining
        .checked_sub(Duration::from_millis(1))
        .filter(|timeout| !timeout.is_zero())
        .unwrap_or(remaining);
    Ok(timeout)
}

fn attempt_receipt_for_result(
    target: &UpstreamModel,
    ordinal: u32,
    latency_ms: u64,
    upstream_http_status: Option<u16>,
    provider_call: ProviderCallOutcome,
    result: &Result<StreamOutcome, ErrorEnvelope>,
    record: &RequestRecord,
) -> AttemptRecord {
    match result {
        Ok(outcome) => attempt_receipt(
            target,
            ordinal,
            latency_ms,
            upstream_http_status,
            provider_call,
            Ok(*outcome),
            record,
        ),
        Err(error) => attempt_receipt(
            target,
            ordinal,
            latency_ms,
            upstream_http_status,
            provider_call,
            Err(error),
            record,
        ),
    }
}

fn record_route_decision(record: &mut RequestRecord, decision: &Decision) {
    record.decision = Some(DecisionRecord::from(decision));
}

fn record_actual_attempt_target(
    record: &mut RequestRecord,
    decision: &Decision,
    target: &UpstreamModel,
) {
    let mut routing = RoutingRecord::from(decision);
    target.upstream.as_str().clone_into(&mut routing.upstream);
    routing.model.clone_from(&target.model);
    record.routing = Some(routing);
}

/// What one attempt puts on the wire.
///
/// Both shapes go through `dispatch`, which is the point: the attempt budget,
/// provider admission, fallback, deadline, health, quota and receipts are the
/// host's, not the payload's. A verbatim Anthropic body used to reach the
/// upstream through its own path — no budget, no lease, no attempt record, and
/// Quota-first routing simply refused to serve it — because the payload's shape
/// had been allowed to decide the control plane.
pub(super) enum AttemptPayload<'a> {
    /// The Canonical IR. Each attempt's target renders it with its own
    /// component, so the wire bytes differ per upstream.
    Canonical(&'a ChatRequest),
    /// The caller's Anthropic Messages body, forwarded verbatim. Only the
    /// routed model is rewritten, so the bytes are the same whichever
    /// `anthropic-native` upstream serves them.
    ///
    /// This shape exists because the Canonical IR cannot carry Anthropic's
    /// server-tool history — the one gap left after stage A′ — so the caller's
    /// own bytes are the only faithful payload for those turns.
    AnthropicNative {
        body: &'a Value,
        /// Curated once by the caller: `SafeHeaders` already refuses any
        /// credential or host-owned name, so the client's own auth can never
        /// ride upstream.
        headers: &'a SafeHeaders,
        stream: bool,
        /// Where an attempt parks the upstream's own error response so the
        /// caller can relay it byte for byte once the pool is exhausted.
        ///
        /// It is deliberately *not* on the `ErrorEnvelope`. `ErrorCode`'s own
        /// docs forbid putting an upstream's raw body there, because a body
        /// may echo the request and the envelope reaches `requests.log`, which
        /// promises to hold no request content. This channel goes only to the
        /// client that already sent the request.
        last_upstream_error: &'a RefCell<Option<RawUpstreamError>>,
    },
}

/// An upstream's own error response, kept out of the envelope on purpose.
#[derive(Debug)]
pub(super) struct RawUpstreamError {
    pub(super) target: UpstreamModel,
    pub(super) status: u16,
    pub(super) body: String,
}

impl Gateway {
    /// Quota-first preamble: whether quota mode is on (as `Some(now_ms)` with a
    /// single wall-clock read) and the conversation's affinity key. The key
    /// derivation differs per payload shape (IR vs. Anthropic wire), so the
    /// caller passes it; it runs lazily, and `(None, "")` outside quota-first
    /// mode costs the caller nothing.
    pub(super) fn quota_preamble(router: &Router, session_key: impl FnOnce() -> String) -> (Option<u64>, String) {
        if router.routing_mode() != RoutingMode::QuotaFirst {
            return (None, String::new());
        }
        (Some(unix_millis()), session_key())
    }

    /// The shared routing step: Tiered consults the router alone; Quota-first
    /// additionally seeds it with the conversation's last-serving account.
    pub(super) fn route_with_mode(
        &self,
        router: &Router,
        request: &ChatRequest,
        hints: &[token_station_protocol::AgentHint],
        candidates: &[Candidate],
        session: &str,
    ) -> Result<Decision, NoRoute> {
        match router.routing_mode() {
            RoutingMode::Tiered => router.route(request, hints, candidates),
            RoutingMode::QuotaFirst => {
                let last = self
                    .quota
                    .lock()
                    .expect("quota lock")
                    .last_account(session)
                    .cloned();
                router.route_quota_first(request, candidates, last.as_ref())
            }
        }
    }

    /// The shared back half of one routed exchange, identical for every
    /// payload shape: the receipt's routing record and quota snapshot, the
    /// in-flight lease, the dispatch, and the settlement. What varies by
    /// payload — how the request was parsed, routed, and prepared — stays with
    /// the caller; what the host owns lives here, so the two entry paths
    /// cannot drift.
    #[allow(clippy::too_many_arguments)] // one routed attempt's full control-plane context
    pub(super) fn execute_routed_attempt(
        &self,
        ctx: &RequestContext,
        agent: &LoadedAgent,
        payload: &AttemptPayload<'_>,
        inbound_tools: &Value,
        decision: &Decision,
        candidates: &[Candidate],
        quota_now_ms: Option<u64>,
        session: &str,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<(UpstreamModel, StreamOutcome), ErrorEnvelope> {
        record_route_decision(record, decision);
        // In quota mode, record why this account was chosen — its window/rate
        // picture at decision time — for the receipt ("why this account").
        if quota_now_ms.is_some()
            && let Some(candidate) = candidates.iter().find(|c| c.target == decision.chosen)
            && let Some(recorded) = record.decision.as_mut()
        {
            recorded.quota = Some(token_station_metrics::QuotaDecisionSnapshot {
                reset_ms: candidate.quota.reset.as_ref().map(|r| r.ms_until_reset),
                remaining_permille: candidate.quota.reset.as_ref().map(|r| r.remaining_permille),
                headroom_permille: candidate.quota.rate_headroom_permille,
                pressured: candidate.quota.rate_pressured,
                exhausted: candidate.quota.exhausted,
            });
        }
        // In quota mode, take an in-flight lease on the chosen account before
        // dispatch so concurrent requests see the load and spread; settle the
        // account below once the exchange finishes.
        let lease = quota_now_ms.map(|now_ms| {
            self.quota
                .lock()
                .expect("quota lock")
                .grant(decision.chosen.upstream.as_str(), now_ms)
        });
        let result = self.dispatch(ctx, agent, payload, inbound_tools, decision, emit, record);
        if let Some(now_ms) = quota_now_ms {
            self.settle_quota(session, lease.as_ref(), now_ms, record, &result);
        }
        result
    }

    /// After a quota-first exchange: release its in-flight lease, charge the
    /// account that actually served, and remember it for this conversation's
    /// next turn (prompt-cache affinity).
    fn settle_quota(
        &self,
        session: &str,
        lease: Option<&crate::quota_lease::LeaseId>,
        now_ms: u64,
        record: &RequestRecord,
        result: &Result<(UpstreamModel, StreamOutcome), ErrorEnvelope>,
    ) {
        let mut quota = self.quota.lock().expect("quota lock");
        if let Some(lease) = lease {
            quota.release(lease);
        }
        if let Ok((served, _)) = result {
            if let Some(usage) = &record.usage {
                quota.record(served.upstream.as_str(), now_ms, usage);
            }
            quota.remember(session, served.clone());
        }
    }

    /// Retries one upstream after replacing visual blocks when that upstream
    /// explicitly rejects media. `None` means no retry was applicable or the
    /// request had no remaining attempt budget.
    #[allow(clippy::too_many_arguments)] // retry needs the same render context as `try_upstream`
    fn retry_without_media(
        &self,
        ctx: &RequestContext,
        agent: &LoadedAgent,
        payload: &AttemptPayload<'_>,
        inbound_tools: &Value,
        decision: &Decision,
        target: &UpstreamModel,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
        budget: &mut AttemptBudget,
        media_retried: &mut bool,
        error: &ErrorEnvelope,
    ) -> Option<Result<StreamOutcome, ErrorEnvelope>> {
        if *media_retried || !is_unsupported_media_error(error) {
            return None;
        }
        // Media fallback rewrites Canonical content parts. A verbatim payload
        // has already had its own fallback applied before it reached the wire,
        // and rewriting it here would mean editing the caller's bytes.
        let AttemptPayload::Canonical(request) = payload else {
            return None;
        };
        let mut fallback_request = (*request).clone();
        let replaced = replace_canonical_images(&mut fallback_request);
        if replaced == 0 || !budget.try_begin(None) {
            return None;
        }

        *media_retried = true;
        record.attempts = budget.attempts;
        record_actual_attempt_target(record, decision, target);
        eprintln!(
            "media fallback -> upstream rejected visual input; retrying {target} with {replaced} image block(s) replaced"
        );
        let retry_clock = Instant::now();
        let mut retry_status = None;
        let mut retry_engine = ProviderCallOutcome::default();
        let retry = self.try_upstream(
            ctx,
            budget.per_attempt_timeout,
            agent,
            &AttemptPayload::Canonical(&fallback_request),
            inbound_tools,
            target,
            emit,
            record,
            &mut retry_status,
            &mut retry_engine,
        );
        let retry_latency = u64::try_from(retry_clock.elapsed().as_millis()).unwrap_or(u64::MAX);
        let attempt = attempt_receipt_for_result(
            target,
            budget.attempts,
            retry_latency,
            retry_status,
            retry_engine,
            &retry,
            record,
        );
        record.attempt_records.push(attempt);
        Some(retry)
    }

    /// Tries the decision's targets in order; moves on only while the error
    /// says another upstream is worth trying, and only before first byte out.
    #[allow(clippy::too_many_arguments)] // one dispatch keeps request + render context explicit
    #[allow(clippy::too_many_lines)] // routing, fallback and receipt state are one attempt machine
    pub(super) fn dispatch(
        &self,
        ctx: &RequestContext,
        agent: &LoadedAgent,
        payload: &AttemptPayload<'_>,
        inbound_tools: &Value,
        decision: &Decision,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<(UpstreamModel, StreamOutcome), ErrorEnvelope> {
        let mut last_error = None;
        let mut budget = AttemptBudget::for_request(ctx);
        let mut media_retried = false;

        let mut targets = std::iter::once(&decision.chosen)
            .chain(&decision.fallbacks)
            .peekable();
        while let Some(target) = targets.next() {
            // A client that already hung up (or a fired drain) gets no further
            // upstreams tried on its behalf.
            if ctx.is_cancelled() {
                if let Some(error) = Self::lifecycle_cancellation(ctx) {
                    return Err(error);
                }
                Self::emit_cancelled(emit);
                return Ok((target.clone(), StreamOutcome::ClientCancelled));
            }
            // Per-Provider admission, held across this attempt. A provider at
            // its ceiling is skipped like a retriable failure — the next
            // candidate gets a turn rather than the request queueing on a hot
            // upstream.
            let Some(_provider) = self.admission.enter_provider(target.upstream.as_str()) else {
                last_error = Some(ErrorEnvelope::new(
                    ErrorCode::Capacity,
                    429,
                    "provider concurrency limit reached",
                ));
                continue;
            };
            // Only a request that obtained its Provider permit consumes an
            // attempt. Local admission skips do not pretend an upstream call
            // happened.
            if !budget.try_begin(None) {
                break;
            }
            record.attempts = budget.attempts;
            record_actual_attempt_target(record, decision, target);
            let attempt_clock = Instant::now();
            let mut upstream_http_status = None;
            let mut provider_call_engine = ProviderCallOutcome::default();
            let result = self.try_upstream(
                ctx,
                budget.per_attempt_timeout,
                agent,
                payload,
                inbound_tools,
                target,
                emit,
                record,
                &mut upstream_http_status,
                &mut provider_call_engine,
            );
            let latency_ms = u64::try_from(attempt_clock.elapsed().as_millis()).unwrap_or(u64::MAX);
            let attempt = attempt_receipt_for_result(
                target,
                budget.attempts,
                latency_ms,
                upstream_http_status,
                provider_call_engine,
                &result,
                record,
            );
            record.attempt_records.push(attempt);
            match result {
                // The terminal health verdict and status are decided exactly
                // once, in `settle`; here we only report who served and how the
                // exchange ended. Per-attempt failures below still trip health so
                // the fallback sweep can eject a bad upstream mid-flight.
                Ok(outcome) => return Ok((target.clone(), outcome)),
                Err(mut error) => {
                    if let Some(lifecycle) = Self::lifecycle_cancellation(ctx) {
                        return Err(lifecycle);
                    }
                    if let Some(retry) = self.retry_without_media(
                        ctx,
                        agent,
                        payload,
                        inbound_tools,
                        decision,
                        target,
                        emit,
                        record,
                        &mut budget,
                        &mut media_retried,
                        &error,
                    ) {
                        match retry {
                            Ok(outcome) => return Ok((target.clone(), outcome)),
                            Err(retry_error) => error = retry_error,
                        }
                    }
                    if let Some(lifecycle) = Self::lifecycle_cancellation(ctx) {
                        return Err(lifecycle);
                    }
                    let fallback_allowed = take_attempt_fallback_policy(&mut error);
                    self.observe(&target.upstream, &target.model, Err(&error));
                    let retriable = fallback_allowed && error.code.is_retriable_elsewhere();
                    eprintln!("upstream {target} failed ({:?})", error.code);
                    if !retriable {
                        last_error = Some(error);
                        break;
                    }
                    // Honor a `Retry-After` only when another real attempt can
                    // follow, bounded by both elapsed and request deadlines.
                    if let Some(retry_after_ms) = error
                        .retry_after_ms
                        .filter(|_| targets.peek().is_some() && budget.has_attempt_remaining())
                    {
                        let wait = budget.retry_delay(Duration::from_millis(retry_after_ms), ctx);
                        if !wait.is_zero() && !ctx.is_cancelled() {
                            wait_retry_delay(ctx, wait);
                        }
                    }
                    last_error = Some(error);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            ErrorEnvelope::new(ErrorCode::Internal, 500, "no upstream was tried")
        }))
    }

    /// The request one attempt renders: the routed model, and `document`
    /// blocks resolved for the upstream's dialect. An Anthropic upstream takes
    /// them as they are; every other dialect's renderer refuses them by name,
    /// so the text is extracted locally (or an honest marker left) first.
    fn request_for_attempt(
        upstream: &Upstream,
        target: &UpstreamModel,
        request: &ChatRequest,
    ) -> ChatRequest {
        let mut request = request.clone();
        request.model.clone_from(&target.model);
        if upstream.config.provider != "anthropic" {
            let documents = replace_canonical_documents(&mut request);
            if documents != DocumentFallbackStats::default() {
                eprintln!(
                    "document fallback -> extracted {} PDF document(s), omitted {} unsupported document(s) for {target}",
                    documents.extracted, documents.omitted
                );
            }
        }
        request
    }

    /// One upstream attempt: build, authorize, inject, send, translate back.
    #[allow(clippy::too_many_arguments)] // one attempt's explicit protocol boundary
    #[allow(clippy::too_many_lines)] // payload choice, eligibility and dispatch stay one path
    fn try_upstream(
        &self,
        ctx: &RequestContext,
        attempt_timeout: Duration,
        agent: &LoadedAgent,
        payload: &AttemptPayload<'_>,
        inbound_tools: &Value,
        target: &UpstreamModel,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
        upstream_http_status: &mut Option<u16>,
        provider_call_engine: &mut ProviderCallOutcome,
    ) -> Result<StreamOutcome, ErrorEnvelope> {
        // Freeze the attempt budget before provider rendering or eligibility
        // work so those stages cannot extend the caller-owned deadline.
        let attempt_deadline = ctx.attempt_deadline_for(attempt_timeout);
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

        let request = match payload {
            AttemptPayload::AnthropicNative {
                body,
                headers,
                stream,
                last_upstream_error,
            } => {
                return self.native_attempt(
                    ctx,
                    target,
                    headers,
                    body,
                    *stream,
                    attempt_timeout,
                    attempt_deadline,
                    emit,
                    record,
                    upstream_http_status,
                    provider_call_engine,
                    last_upstream_error,
                );
            }
            // Routing may have picked a different model than the caller named.
            AttemptPayload::Canonical(request) => {
                Self::request_for_attempt(upstream, target, request)
            }
        };
        let descriptor = Self::build_provider_request(upstream, &request, record)?;

        let response = match self.send_provider_call(
            ctx,
            attempt_timeout,
            upstream,
            &descriptor,
            target.upstream.as_str(),
            request.stream,
            attempt_deadline,
            provider_call_engine,
        ) {
            Err(_) if ctx.is_cancelled() => {
                record_conversion_cancelled(
                    record,
                    ConversionStage::ProviderResponse,
                    upstream.config.provider.as_str(),
                    CANONICAL_CHAT_PROTOCOL,
                );
                if let Some(error) = Self::lifecycle_cancellation(ctx) {
                    return Err(error);
                }
                Self::emit_cancelled(emit);
                return Ok(StreamOutcome::ClientCancelled);
            }
            Err(error) => {
                record_conversion(
                    record,
                    ConversionStage::ProviderResponse,
                    upstream.config.provider.as_str(),
                    CANONICAL_CHAT_PROTOCOL,
                    false,
                    Some(error.code),
                );
                return Err(error);
            }
            Ok(response) => response,
        };
        *upstream_http_status = Some(response.status);

        if let Err(error) = EgressPolicy::reject_redirect(response.status) {
            record_conversion(
                record,
                ConversionStage::ProviderResponse,
                upstream.config.provider.as_str(),
                CANONICAL_CHAT_PROTOCOL,
                false,
                Some(error.code),
            );
            return Err(error);
        }

        if response.status >= 400 {
            return match Self::classify_provider_error(ctx, upstream, response, emit, record) {
                Ok(error) => Err(error),
                Err(outcome) => Ok(outcome),
            };
        }

        // L2 authoritative quota: harvest the provider's own remaining/reset
        // headers off this successful response and record them for the account.
        // Mode-agnostic (cheap; only read in quota-first mode) so the data is
        // already warm whenever the user is in quota mode. Never touches the body.
        let windows = crate::quota_headers::parse_quota_windows(&response.headers, unix_millis());
        if !windows.is_empty() {
            self.quota.lock().expect("quota lock").note_authoritative(
                target.upstream.as_str(),
                unix_millis(),
                windows,
            );
        }

        if request.stream {
            Self::translate_stream_response(
                ctx,
                agent,
                upstream,
                &request,
                response,
                inbound_tools,
                target,
                emit,
                record,
            )
        } else {
            Self::translate_nonstream_response(
                ctx,
                agent,
                upstream,
                &request,
                response,
                inbound_tools,
                emit,
                record,
            )
        }
    }
}

#[cfg(test)]
mod attempt_budget_tests {
    use super::{AttemptBudget, map_transport_error};
    use std::time::{Duration, Instant};

    fn budget(max_attempts: u32, max_cost: Option<u64>) -> AttemptBudget {
        AttemptBudget {
            max_attempts,
            max_elapsed: Duration::from_mins(1),
            per_attempt_timeout: Duration::from_secs(10),
            max_cost,
            started: Instant::now(),
            attempts: 0,
            reserved_cost: 0,
        }
    }

    #[test]
    fn count_and_cost_are_consumed_before_each_attempt() {
        let mut value = budget(2, Some(10));
        assert!(value.try_begin(Some(4)));
        assert!(value.try_begin(Some(6)));
        assert!(!value.try_begin(Some(0)), "count ceiling wins");
    }

    #[test]
    fn configured_cost_budget_rejects_unknown_or_excess_cost() {
        let mut unknown = budget(3, Some(10));
        assert!(!unknown.try_begin(None));
        let mut excess = budget(3, Some(10));
        assert!(!excess.try_begin(Some(11)));
        assert_eq!(excess.attempts, 0);
    }

    #[test]
    fn a_transport_timeout_has_the_stable_timeout_classification() {
        let envelope = map_transport_error(ureq::Error::Timeout(ureq::Timeout::Global));
        assert_eq!(envelope.code, token_station_protocol::ErrorCode::Timeout);
        assert_eq!(envelope.http_status, 504);
    }
}

#[cfg(test)]
mod request_receipt_tests {
    use std::collections::BTreeMap;

    use super::{
        annotate_conversion_failure, begin_record, catalog_model_document, model_cost_document,
        record_actual_attempt_target, record_conversion, record_conversion_cancelled,
        record_route_decision, settle_estimated_cost, tag_transport,
    };
    use crate::pricing::{ModelPrice, PriceTable};
    use token_station_metrics::{
        ConversionOutcome, ConversionReasonCode, ConversionReasonDetail, ConversionStage, CostKind,
        RequestPathKind, RequestRecord,
    };
    use token_station_protocol::{ErrorCode, ErrorEnvelope, ModelCapability, Usage};
    use token_station_router_core::{
        DecidedBy, Decision, RequestFeatures, UpstreamModel, UpstreamRef,
    };

    #[test]
    fn settlement_prefers_scoped_prices_and_preserves_unscoped_fallback() {
        let price = |input_per_mtok| ModelPrice {
            input_per_mtok,
            ..ModelPrice::default()
        };
        let pricing = PriceTable {
            version: 9,
            models: BTreeMap::from([
                ("provider_a/shared".to_owned(), price(200_000)),
                ("provider_b/shared".to_owned(), price(700_000)),
                ("shared".to_owned(), price(50_000)),
            ]),
        };

        for (upstream, expected) in [
            ("provider_a", 200_000),
            ("provider_b", 700_000),
            ("legacy_provider", 50_000),
        ] {
            let served = UpstreamModel::new(UpstreamRef::new(upstream).unwrap(), "shared");
            let mut record = RequestRecord::begin(1, "openai-chat-completions");
            record.usage = Some(Usage {
                input_tokens: 1_000_000,
                ..Usage::default()
            });

            settle_estimated_cost(&pricing, &mut record, &served);

            assert_eq!(record.cost_kind, CostKind::Estimated);
            assert_eq!(record.cost_micros, Some(expected));
            assert_eq!(record.price_version, Some(9));
        }
    }

    #[test]
    fn models_document_preserves_discovered_limits_and_cost() {
        let capability: ModelCapability = serde_json::from_value(serde_json::json!({
            "model": "glm-5.2",
            "context_window": 257_550,
            "max_output_tokens": 32_768,
            "catalog_cost": {"input": 0.2, "output": 0.6, "cache_read": 0.04}
        }))
        .unwrap();

        let document = catalog_model_document(
            "wecoding",
            &capability,
            &crate::pricing::PriceTable::default(),
        );
        assert_eq!(document["context_window"], serde_json::json!(257_550));
        assert_eq!(document["max_output_tokens"], serde_json::json!(32_768));
        assert_eq!(
            document["limit"],
            serde_json::json!({"context": 257_550, "output": 32_768})
        );
        assert_eq!(
            document["cost"],
            serde_json::json!({"input": 0.2, "output": 0.6, "cache_read": 0.04})
        );
    }

    #[test]
    fn partial_catalog_cost_falls_back_to_complete_configured_pricing() {
        let capability: ModelCapability = serde_json::from_value(serde_json::json!({
            "model": "priced-model",
            "context_window": 32_000,
            "catalog_cost": {"input": 99.0}
        }))
        .unwrap();
        let pricing = PriceTable {
            models: BTreeMap::from([(
                "priced-model".to_owned(),
                ModelPrice {
                    input_per_mtok: 1_000_000,
                    output_per_mtok: 2_000_000,
                    cache_read_per_mtok: 300_000,
                    cache_write_per_mtok: 400_000,
                    reasoning_per_mtok: None,
                },
            )]),
            ..PriceTable::default()
        };

        assert_eq!(
            model_cost_document(&capability, &pricing),
            Some(serde_json::json!({
                "input": 1.0,
                "output": 2.0,
                "cache_read": 0.3,
                "cache_write": 0.4
            }))
        );
    }

    #[test]
    fn request_ids_are_random_fixed_width_and_scope_is_bound_at_arrival() {
        let first = begin_record(
            1_752_000_000_000,
            "openai-chat-completions",
            Some("codex"),
            Some(42),
        );
        let second = begin_record(
            1_752_000_000_000,
            "openai-chat-completions",
            Some("codex"),
            Some(42),
        );

        assert_eq!(first.request_id.len(), 36);
        assert!(first.request_id.starts_with("req_"));
        assert!(
            first.request_id[4..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
        assert_ne!(first.request_id, second.request_id);
        assert_eq!(first.agent_id.as_deref(), Some("codex"));
        assert_eq!(first.running_revision, Some(42));
    }

    #[test]
    fn transport_and_conversion_diagnostics_are_closed_and_content_free() {
        let mut record = begin_record(1, "openai-responses", Some("codex"), None);
        tag_transport(
            &mut record,
            "POST",
            "/not-a-real-endpoint/secret-caller-text",
            false,
        );
        assert_eq!(record.request_method.as_deref(), Some("POST"));
        assert_eq!(record.path_kind, RequestPathKind::UnknownAgentEndpoint);
        let serialized = serde_json::to_string(&record).unwrap();
        assert!(!serialized.contains("secret-caller-text"));

        let error = ErrorEnvelope::new(
            ErrorCode::Capability,
            400,
            "unsupported Responses tool type local_shell",
        );
        record_conversion(
            &mut record,
            ConversionStage::InboundNormalize,
            "openai-responses",
            "token-station-chat",
            false,
            Some(error.code),
        );
        annotate_conversion_failure(&mut record, &error);
        let conversion = record.conversion_reports.last().unwrap();
        assert_eq!(
            conversion.reason_code,
            Some(ConversionReasonCode::UnsupportedToolType)
        );
        assert_eq!(
            conversion.reason_detail,
            Some(ConversionReasonDetail::LocalShell)
        );

        record_conversion_cancelled(
            &mut record,
            ConversionStage::StreamTranslate,
            "token-station-chat",
            "openai-responses",
        );
        let cancelled = record.conversion_reports.last().unwrap();
        assert_eq!(cancelled.outcome, ConversionOutcome::Cancelled);
        assert_eq!(cancelled.error_code, None);
    }

    #[test]
    fn a_decision_does_not_claim_an_actual_provider_until_an_attempt_starts() {
        let chosen = UpstreamModel::new(UpstreamRef::new("primary").unwrap(), "model-a");
        let fallback = UpstreamModel::new(UpstreamRef::new("fallback").unwrap(), "model-b");
        let decision = Decision {
            chosen,
            decided_by: DecidedBy::Default,
            fallbacks: vec![fallback.clone()],
            features: RequestFeatures::default(),
            pool: "main".to_owned(),
        };
        let mut record = begin_record(1, "openai-chat-completions", None, None);

        record_route_decision(&mut record, &decision);
        assert!(record.decision.is_some());
        assert!(
            record.routing.is_none(),
            "a zero-attempt receipt has no actual server"
        );

        record_actual_attempt_target(&mut record, &decision, &fallback);
        assert_eq!(
            record.routing.as_ref().map(|route| route.upstream.as_str()),
            Some("fallback")
        );
    }
}

#[cfg(test)]
mod south_stream_fallback_policy_tests {
    use super::{
        CancellationDispositionV1, buffered_transport_timeout, forbid_attempt_fallback,
        map_south_stream_failure_for_attempt, sanitize_attempt_error_for_render,
        take_attempt_fallback_policy,
    };
    use south_contracts::StreamReadErrorV1;
    use std::time::{Duration, Instant};
    use token_station_protocol::{ErrorCode, ErrorEnvelope};

    #[test]
    fn south_stream_deadline_policy_is_consumed_before_the_error_leaves_dispatch() {
        let mut error = map_south_stream_failure_for_attempt(
            StreamReadErrorV1::StreamDeadlineExceeded,
            CancellationDispositionV1::Deadline,
        );

        assert!(!take_attempt_fallback_policy(&mut error));
        assert!(
            error.extensions.is_empty(),
            "the host-private routing marker must not reach clients or receipts"
        );
        let mut idle = map_south_stream_failure_for_attempt(
            StreamReadErrorV1::StreamIdleTimeout,
            CancellationDispositionV1::Deadline,
        );
        assert!(take_attempt_fallback_policy(&mut idle));
    }

    #[test]
    fn host_private_fallback_policy_never_reaches_the_postcommit_renderer() {
        let mut error = forbid_attempt_fallback(ErrorEnvelope::new(
            ErrorCode::Timeout,
            504,
            "request deadline exceeded",
        ));

        sanitize_attempt_error_for_render(&mut error);

        assert!(
            error.extensions.is_empty(),
            "the plugin renderer must never observe host-private routing state"
        );
    }

    #[test]
    fn expired_buffered_transport_budget_is_a_deadline_error() {
        let now = Instant::now();
        let error = buffered_transport_timeout(now, now)
            .expect_err("an expired attempt cannot build a zero-timeout client");

        assert_eq!((error.code, error.http_status), (ErrorCode::Timeout, 504));
        assert_eq!(error.message, "request deadline exceeded");
        assert_eq!(
            buffered_transport_timeout(now + Duration::from_millis(2), now)
                .expect("a live attempt retains one millisecond"),
            Duration::from_millis(1)
        );
    }
}

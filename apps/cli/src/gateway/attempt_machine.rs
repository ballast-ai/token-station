// Split from gateway.rs — code moved verbatim; see gateway.rs for the module map.
#[allow(clippy::wildcard_imports)]
use super::*;

// Include schema input in receipts without changing frozen routing policy.
fn estimated_input_with_schemas(
    request: &ChatRequest,
    hints: &[token_station_protocol::AgentHint],
) -> u32 {
    use token_station_router_core::{RequestFeatures, estimate_tokens};
    let mut tokens = RequestFeatures::extract(request, hints).estimated_input_tokens;
    for tool in &request.tools {
        tokens = tokens
            .saturating_add(estimate_tokens(&tool.name))
            .saturating_add(estimate_tokens(
                tool.description.as_deref().unwrap_or_default(),
            ))
            .saturating_add(estimate_tokens(&tool.parameters.to_string()));
    }
    if let Some(token_station_protocol::ResponseFormat::JsonSchema { json_schema }) =
        &request.response_format
    {
        tokens = tokens.saturating_add(estimate_tokens(&json_schema.to_string()));
    }
    tokens
}

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

/// One actual provider attempt owns its load reservation.
struct AttemptQuotaLease<'a> {
    quota: &'a std::sync::Mutex<crate::quota_tracker::QuotaTracker>,
    lease: crate::quota_lease::LeaseId,
}

impl Drop for AttemptQuotaLease<'_> {
    fn drop(&mut self) {
        // Recover the guard during unwind so the original failure remains visible.
        let mut quota = self
            .quota
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        quota.release(&self.lease);
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
    /// The caller's OpenAI Responses body, forwarded verbatim except for the
    /// routed model name.
    ResponsesNative {
        body: &'a Value,
        headers: &'a SafeHeaders,
        stream: bool,
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
    pub(super) fn quota_preamble(
        router: &Router,
        session_key: impl FnOnce() -> String,
    ) -> (Option<u64>, String) {
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
        let mut decision = match router.routing_mode() {
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
        }?;
        if request.tools.is_empty()
            && !matches!(
                request.response_format,
                Some(token_station_protocol::ResponseFormat::JsonSchema { .. })
            )
        {
            return Ok(decision);
        }
        let estimated_input_tokens = estimated_input_with_schemas(request, hints);
        // The frozen router accepts requests, not independent context estimates.
        // Keep its tier, quota, exact-model, and soft-overflow decisions unchanged.
        // Schema-aware route selection remains deferred until that API can evolve.
        decision.features.estimated_input_tokens = estimated_input_tokens;
        Ok(decision)
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
        let result = self.dispatch(ctx, agent, payload, inbound_tools, decision, emit, record);
        self.settle_quota(session, record, &result);
        result
    }

    /// Remember conversation affinity after the exchange. Each actual attempt
    /// has already settled its own consumption and released its lease.
    fn settle_quota(
        &self,
        session: &str,
        record: &RequestRecord,
        result: &Result<(UpstreamModel, StreamOutcome), ErrorEnvelope>,
    ) {
        let mut quota = self.quota.lock().expect("quota lock");
        if let Ok((served, outcome)) = result
            && !session.is_empty()
            && (*outcome == StreamOutcome::Complete || record.usage.is_some())
        {
            quota.remember(session, served.clone());
        }
    }

    fn settle_attempt_quota(
        &self,
        target: &UpstreamModel,
        now_ms: u64,
        record: &RequestRecord,
        result: &Result<StreamOutcome, ErrorEnvelope>,
    ) {
        // A retry can consume tokens before failing. Preserve that consumption
        // before the next attempt resets the accounting tap. The ordinal keeps
        // two attempts on the same provider distinct while settlement stays idempotent.
        let receipt_id = format!("{}:attempt:{}", record.request_id, record.attempts);
        self.quota.lock().expect("quota lock").record_settled(
            target.upstream.as_str(),
            &receipt_id,
            now_ms,
            record.usage.as_ref(),
            matches!(result, Ok(StreamOutcome::Complete)),
        );
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

        let mut targets = std::iter::once(&decision.chosen)
            .chain(&decision.fallbacks)
            .peekable();
        while let Some(target) = targets.next() {
            // A client that already hung up (or a fired drain) gets no further
            // upstreams tried on its behalf.
            if ctx.is_cancelled() {
                return Self::cancel_before_attempt(ctx, target, emit, record);
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
            let quota_lease = AttemptQuotaLease {
                quota: &self.quota,
                lease: self.quota.lock().expect("quota lock").grant_for(
                    target.upstream.as_str(),
                    unix_millis(),
                    u64::try_from(ctx.remaining().as_millis()).unwrap_or(u64::MAX),
                ),
            };
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
            let result = result.map_err(|error| {
                let has_images = match payload {
                    AttemptPayload::Canonical(request) => request_contains_images(request),
                    AttemptPayload::AnthropicNative { body, .. }
                    | AttemptPayload::ResponsesNative { body, .. } => raw_contains_images(body),
                };
                if has_images && is_unsupported_media_error(&error) {
                    upstream_image_error()
                } else {
                    error
                }
            });
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
                    let fallback_allowed = take_attempt_fallback_policy(&mut error);
                    self.observe(&target.upstream, &target.model, Err(&error));
                    let retriable = fallback_allowed && error.code.is_retriable_elsewhere();
                    eprintln!("upstream {target} failed ({:?})", error.code);
                    if !retriable {
                        last_error = Some(error);
                        break;
                    }
                    drop(quota_lease);
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

    fn cancel_before_attempt(
        ctx: &RequestContext,
        pending: &UpstreamModel,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &RequestRecord,
    ) -> Result<(UpstreamModel, StreamOutcome), ErrorEnvelope> {
        if let Some(error) = Self::lifecycle_cancellation(ctx) {
            return Err(error);
        }
        Self::emit_cancelled(emit);
        // Usage belongs to the last started attempt. The next candidate has
        // not entered its wrapper and must not receive its predecessor's bill.
        let served = record.attempt_records.last().map_or_else(
            || pending.clone(),
            |attempt| {
                UpstreamModel::new(
                    UpstreamRef::new(&attempt.upstream).expect("attempt upstream was validated"),
                    &attempt.model,
                )
            },
        );
        Ok((served, StreamOutcome::ClientCancelled))
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
    #[allow(clippy::too_many_arguments)]
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
        let endpoint = self
            .upstreams
            .get(target.upstream.as_str())
            .map_or_else(String::new, |upstream| upstream.config.base_url.as_str());
        ctx.begin_accounting(&endpoint);
        record.usage = None;
        record.usage_observation = None;
        record.cost_micros = None;
        record.cost_kind = CostKind::Unknown;
        record.price_version = None;
        let result = self.try_upstream_inner(
            ctx,
            attempt_timeout,
            agent,
            payload,
            inbound_tools,
            target,
            emit,
            record,
            upstream_http_status,
            provider_call_engine,
        );
        Self::finish_attempt_accounting(ctx, record, &result);
        self.settle_attempt_quota(target, unix_millis(), record, &result);
        result
    }

    fn finish_attempt_accounting(
        ctx: &RequestContext,
        record: &mut RequestRecord,
        result: &Result<StreamOutcome, ErrorEnvelope>,
    ) {
        ctx.finish_accounting(record);
        if !matches!(result, Ok(StreamOutcome::Complete)) {
            if record.usage_observation.is_none() && record.usage.is_some() {
                record.usage_observation = Some(token_station_metrics::UsageObservation::default());
            }
            if let Some(observation) = record.usage_observation.as_mut() {
                observation.incomplete = true;
            }
        }
    }

    #[allow(clippy::too_many_arguments)] // one attempt's explicit protocol boundary
    #[allow(clippy::too_many_lines)] // payload choice, eligibility and dispatch stay one path
    fn try_upstream_inner(
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
            AttemptPayload::ResponsesNative {
                body,
                headers,
                stream,
                last_upstream_error,
            } => {
                return self.responses_native_attempt(
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
        ctx.capture_upstream_request(target.upstream.as_str(), &target.model, &descriptor);

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
        // Preserve an observed response even when policy rejects it before the
        // provider parser takes ownership (for example, a blocked redirect).
        ctx.capture_upstream_response_head(response.status, &response.headers);

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
        self.quota.lock().expect("quota lock").note_authoritative(
            target.upstream.as_str(),
            unix_millis(),
            windows,
        );

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
mod cancelled_settlement_tests {
    use super::*;
    use crate::pricing::{ModelPrice, PriceTable};
    use crate::quota_tracker::{QuotaPlan, QuotaTracker, QuotaWindowSpec};
    use token_station_router_core::{DecidedBy, RequestFeatures};

    fn gateway() -> Gateway {
        let config: ClientConfig = serde_json::from_str(crate::EXAMPLE_CONFIG).unwrap();
        let plan = QuotaPlan {
            windows: vec![QuotaWindowSpec {
                len_ms: 60_000,
                limit: 10_000_000,
            }],
            ..QuotaPlan::default()
        };
        Gateway {
            agents: Vec::new(),
            skipped_agents: Vec::new(),
            home_router: None,
            home_dynamic_router: None,
            agent_routers: std::sync::RwLock::new(BTreeMap::new()),
            supported_agent_ids: BTreeSet::new(),
            upstreams: BTreeMap::new(),
            local_upstreams: BTreeSet::new(),
            free_upstreams: BTreeSet::new(),
            catalog: Vec::new(),
            health: std::sync::Mutex::new(HealthTracker::new(HealthPolicy {
                eject_after: 3,
                cooldown: Duration::from_secs(1),
            })),
            quota: std::sync::Mutex::new(QuotaTracker::new(
                [("a".to_owned(), plan.clone()), ("b".to_owned(), plan)].into(),
            )),
            admission: Admission::new(config.concurrency),
            pricing: PriceTable {
                version: 9,
                models: [
                    (
                        "a/shared".to_owned(),
                        ModelPrice {
                            input_per_mtok: 200_000,
                            ..ModelPrice::default()
                        },
                    ),
                    (
                        "b/shared".to_owned(),
                        ModelPrice {
                            input_per_mtok: 700_000,
                            ..ModelPrice::default()
                        },
                    ),
                ]
                .into(),
            },
            secrets: SecretStore::from_config(&config, &config.data.dir),
            egress: EgressPolicy::new(config.egress),
            south_runtime: None,
            recorder: Arc::new(token_station_metrics::NoopRecorder),
            body_log: None,
        }
    }

    #[test]
    fn failed_attempt_usage_and_same_account_retries_settle_once_each() {
        let gateway = gateway();
        let a = UpstreamModel::new(UpstreamRef::new("a").unwrap(), "shared");
        let b = UpstreamModel::new(UpstreamRef::new("b").unwrap(), "shared");
        let mut record = RequestRecord::begin(1, "openai");
        let failure = Err(ErrorEnvelope::new(
            ErrorCode::ProviderProtocolError,
            502,
            "invalid response",
        ));
        record.attempts = 1;
        record.usage = Some(Usage {
            input_tokens: 100,
            ..Usage::default()
        });
        gateway.settle_attempt_quota(&a, 1_000, &record, &failure);
        gateway.settle_attempt_quota(&a, 1_000, &record, &failure);
        assert_eq!(
            gateway.quota.lock().unwrap().snapshot(&["a".into()], 1_000)[0].windows[0].used,
            100
        );
        record.attempts = 2;
        record.usage.as_mut().unwrap().input_tokens = 200;
        gateway.settle_attempt_quota(&a, 1_000, &record, &failure);
        record.attempts = 3;
        record.usage.as_mut().unwrap().input_tokens = 300;
        gateway.settle_attempt_quota(&b, 1_000, &record, &Ok(StreamOutcome::Complete));
        gateway.settle_quota(
            "conversation",
            &record,
            &Ok((b.clone(), StreamOutcome::Complete)),
        );
        record.attempts = 4;
        record.usage = None;
        gateway.settle_attempt_quota(&b, 1_000, &record, &failure);
        let quota = gateway.quota.lock().unwrap();
        let snapshots = quota.snapshot(&["a".into(), "b".into()], 1_000);
        assert_eq!(snapshots[0].windows[0].used, 300);
        assert_eq!(snapshots[1].windows[0].used, 300);
        assert_eq!(quota.last_account("conversation"), Some(&b));
    }

    #[test]
    fn cancellation_between_attempts_keeps_cost_quota_and_affinity_on_actual_account() {
        let gateway = gateway();
        let ctx = RequestContext::detached(Duration::from_secs(10), Duration::from_secs(1));
        let a = UpstreamModel::new(UpstreamRef::new("a").unwrap(), "shared");
        let b = UpstreamModel::new(UpstreamRef::new("b").unwrap(), "shared");
        let decision = Decision {
            chosen: a.clone(),
            fallbacks: vec![b.clone()],
            decided_by: DecidedBy::Default,
            features: RequestFeatures::default(),
            pool: "main".to_owned(),
        };
        let mut record = RequestRecord::begin(1, "openai");
        record_actual_attempt_target(&mut record, &decision, &a);
        record.attempts = 1;
        ctx.begin_accounting("https://example.test/v1");
        ctx.capture_upstream_response_head(200, &BTreeMap::new());
        ctx.append_upstream_response_body(
            br#"{"usage":{"prompt_tokens":1000000,"completion_tokens":0}}"#,
        );
        ctx.finish_accounting(&mut record);
        let error = ErrorEnvelope::new(
            ErrorCode::ProviderProtocolError,
            502,
            "invalid response envelope",
        );
        record.attempt_records.push(attempt_receipt(
            &a,
            1,
            1,
            Some(200),
            ProviderCallOutcome::default(),
            Err(&error),
            &record,
        ));

        gateway.settle_attempt_quota(&a, 1_000, &record, &Err(error));

        // Freeze the real loop boundary: A failed with observed usage, B is
        // pending, and the client disconnects before B can enter its wrapper.
        ctx.cancel();
        let mut replies = Vec::new();
        let result = Gateway::cancel_before_attempt(
            &ctx,
            &b,
            &mut |reply| {
                replies.push(reply);
                true
            },
            &record,
        );
        gateway.settle_quota("conversation", &record, &result);
        let (served, outcome) = result.unwrap();
        gateway.settle(&mut record, &served, outcome);

        let quota = gateway.quota.lock().unwrap();
        let snapshots = quota.snapshot(&["a".to_owned(), "b".to_owned()], 1_000);
        assert_eq!(snapshots[0].windows[0].used, 1_000_000);
        assert_eq!(snapshots[1].windows[0].used, 0);
        assert_eq!(quota.last_account("conversation"), Some(&a));
        assert_eq!(served, a);
        assert_eq!(record.cost_micros, Some(200_000));
        assert_eq!(record.status, 499);
        assert_eq!(record.attempt_records.len(), 1);
        assert_eq!(record.routing.as_ref().unwrap().upstream, "a");
        assert_eq!(replies.len(), 1);
    }

    #[test]
    fn cancellation_before_any_attempt_does_not_charge_or_remember_pending_account() {
        let gateway = gateway();
        let ctx = RequestContext::detached(Duration::from_secs(10), Duration::from_secs(1));
        let pending = UpstreamModel::new(UpstreamRef::new("b").unwrap(), "shared");
        let mut record = RequestRecord::begin(1, "openai");
        ctx.cancel();
        let result = Gateway::cancel_before_attempt(&ctx, &pending, &mut |_| true, &record);
        gateway.settle_quota("unstarted", &record, &result);
        let (served, outcome) = result.unwrap();
        gateway.settle(&mut record, &served, outcome);
        let quota = gateway.quota.lock().unwrap();
        assert_eq!(
            quota.snapshot(&["b".to_owned()], 1_000)[0].windows[0].used,
            0
        );
        assert!(quota.last_account("unstarted").is_none());
        assert!(record.cost_micros.is_none());
        assert!(record.routing.is_none());
        assert!(record.attempt_records.is_empty());
        assert_eq!(record.status, 499);
    }

    #[test]
    fn failed_attempt_observations_are_incomplete_without_inventing_missing_usage() {
        let gateway = gateway();
        let target = UpstreamModel::new(UpstreamRef::new("a").unwrap(), "shared");
        let error = ErrorEnvelope::new(
            ErrorCode::ProviderProtocolError,
            502,
            "invalid response envelope",
        );
        for (body, adapter_usage) in [
            (
                Some(r#"{"usage":{"prompt_tokens":1000000,"completion_tokens":0}}"#),
                None,
            ),
            (Some(r#"{"usage":{}}"#), None),
            (
                None,
                Some(Usage {
                    input_tokens: 1_000_000,
                    ..Usage::default()
                }),
            ),
            (None, None),
        ] {
            for result in [
                Ok(StreamOutcome::Complete),
                Err(error.clone()),
                Ok(StreamOutcome::ClientCancelled),
                Ok(StreamOutcome::FailedAfterPartial),
                Ok(StreamOutcome::FailedBeforeOutput),
            ] {
                let failed = !matches!(result, Ok(StreamOutcome::Complete));
                let ctx = RequestContext::detached(Duration::from_secs(10), Duration::from_secs(1));
                ctx.begin_accounting("https://example.test/v1");
                ctx.capture_upstream_response_head(200, &BTreeMap::new());
                if let Some(body) = body {
                    ctx.append_upstream_response_body(body.as_bytes());
                }
                let mut record = RequestRecord::begin(1, "openai");
                record.usage = adapter_usage;
                Gateway::finish_attempt_accounting(&ctx, &mut record, &result);
                if body.is_some() || (failed && adapter_usage.is_some()) {
                    assert_eq!(record.usage_observation.unwrap().incomplete, failed);
                } else {
                    assert!(record.usage_observation.is_none());
                }
                if failed {
                    settle_estimated_cost(&gateway.pricing, &mut record, &target);
                    assert_eq!(record.cost_micros, None);
                }
            }
        }
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
    fn actual_cost_wins_and_incomplete_or_inconsistent_usage_is_not_estimated() {
        let pricing = PriceTable::builtin();
        let target = UpstreamModel {
            upstream: UpstreamRef::new("openrouter").unwrap(),
            model: "gpt-5.5".to_owned(),
        };
        let mut record = RequestRecord::begin(0, "openai");
        record.usage = Some(Usage {
            input_tokens: 10,
            output_tokens: 2,
            ..Usage::default()
        });
        record.cost_kind = CostKind::Actual;
        record.cost_micros = Some(1);
        settle_estimated_cost(&pricing, &mut record, &target);
        assert_eq!(record.cost_micros, Some(1));
        assert_eq!(record.price_version, None);
        record.cost_kind = CostKind::Unknown;
        record.cost_micros = None;
        record.usage_observation = Some(token_station_metrics::UsageObservation {
            input_tokens: Some(10),
            ..token_station_metrics::UsageObservation::default()
        });
        settle_estimated_cost(&pricing, &mut record, &target);
        assert_eq!(record.cost_micros, None);
        record.usage_observation = None;
        record.usage.as_mut().unwrap().cache_read_tokens = 11;
        settle_estimated_cost(&pricing, &mut record, &target);
        assert_eq!(record.cost_micros, None);
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
                    cache_write_5m_per_mtok: None,
                    cache_write_1h_per_mtok: None,
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

#[cfg(test)]
mod schema_estimate_tests {
    use super::*;
    use token_station_protocol::{CapabilityState, Message, ResponseFormat, Role};
    use token_station_router_core::{Health, RouterConfig};

    fn gateway() -> Gateway {
        let config: ClientConfig = serde_json::from_str(crate::EXAMPLE_CONFIG).unwrap();
        Gateway {
            agents: Vec::new(),
            skipped_agents: Vec::new(),
            home_router: None,
            home_dynamic_router: None,
            agent_routers: std::sync::RwLock::new(BTreeMap::new()),
            supported_agent_ids: BTreeSet::new(),
            upstreams: BTreeMap::new(),
            local_upstreams: BTreeSet::new(),
            free_upstreams: BTreeSet::new(),
            catalog: Vec::new(),
            health: std::sync::Mutex::new(HealthTracker::new(HealthPolicy {
                eject_after: 3,
                cooldown: Duration::from_secs(1),
            })),
            quota: std::sync::Mutex::new(crate::quota_tracker::QuotaTracker::new(
                std::collections::HashMap::new(),
            )),
            admission: Admission::new(config.concurrency),
            pricing: config.pricing.clone(),
            secrets: SecretStore::from_config(&config, &config.data.dir),
            egress: EgressPolicy::new(config.egress),
            south_runtime: None,
            recorder: Arc::new(token_station_metrics::NoopRecorder),
            body_log: None,
        }
    }

    fn candidate(upstream: &str, context_window: u32) -> Candidate {
        Candidate::new(
            UpstreamModel::new(UpstreamRef::new(upstream).unwrap(), "shared"),
            ModelCapability {
                tool: true,
                tool_state: Some(CapabilityState::Declared),
                json_schema: true,
                json_schema_state: Some(CapabilityState::Declared),
                context_window,
                ..ModelCapability::default()
            },
            Health::Healthy,
        )
    }

    fn router(mode: RoutingMode, candidates: &[Candidate], assumed: u32, exact: bool) -> Router {
        let targets: Vec<_> = candidates
            .iter()
            .map(|candidate| candidate.target.clone())
            .collect();
        let mut config: RouterConfig = serde_json::from_value(json!({
            "version": 1,
            "pools": {"primary": targets},
            "default_pool": "primary",
            "assumed_context_window": assumed,
            "honor_exact_model": exact,
            "routing_mode": mode,
            "quota_accounts": targets,
        }))
        .unwrap();
        if candidates.len() == 4 {
            config
                .pools
                .insert("primary".to_owned(), targets[..2].to_vec());
            config
                .pools
                .insert("backup".to_owned(), targets[2..].to_vec());
            config.recovery = token_station_router_core::RecoveryPolicy::Ordered {
                pools: vec!["backup".to_owned()],
            };
        }
        let example: ClientConfig = serde_json::from_str(crate::EXAMPLE_CONFIG).unwrap();
        config.heuristic = example.router.heuristic.map(|mut heuristic| {
            heuristic.above = "primary".to_owned();
            heuristic.below = "primary".to_owned();
            heuristic
        });
        Router::new(config).unwrap()
    }

    fn request(output_schema: bool) -> ChatRequest {
        let mut request = ChatRequest::new("shared", vec![Message::text(Role::User, "hello")]);
        let schema = json!({"type": "object", "description": "schema detail ".repeat(2000)});
        if output_schema {
            request.response_format = Some(ResponseFormat::JsonSchema {
                json_schema: schema,
            });
        } else {
            request.tools.push(ToolDef {
                name: "lookup".to_owned(),
                description: Some("Read a record".to_owned()),
                parameters: schema,
            });
        }
        request
    }

    #[test]
    fn schema_estimates_preserve_primary_recovery_and_exact_decisions_in_both_modes() {
        let gateway = gateway();
        let candidates = vec![
            candidate("small", 100),
            candidate("large", 100_000),
            candidate("small_backup", 100),
            candidate("large_backup", 100_000),
        ];
        for mode in [RoutingMode::Tiered, RoutingMode::QuotaFirst] {
            for exact in [false, true] {
                for output_schema in [false, true] {
                    let router = router(mode, &candidates, 8192, exact);
                    let request = request(output_schema);
                    let baseline = match mode {
                        RoutingMode::Tiered => router.route(&request, &[], &candidates),
                        RoutingMode::QuotaFirst => {
                            router.route_quota_first(&request, &candidates, None)
                        }
                    }
                    .unwrap();
                    let mut actual = gateway
                        .route_with_mode(&router, &request, &[], &candidates, "test")
                        .unwrap();
                    assert!(
                        !actual.fallbacks.is_empty(),
                        "exercise recovery and fallback order"
                    );
                    assert!(actual.features.estimated_input_tokens > 100);
                    actual.features.estimated_input_tokens =
                        baseline.features.estimated_input_tokens;
                    assert_eq!(actual, baseline);
                }
            }
        }
    }

    #[test]
    fn schema_estimates_preserve_soft_overflow_and_unknown_window_assumptions() {
        let gateway = gateway();
        let candidates = vec![candidate("unknown", 0), candidate("known", 200)];
        for mode in [RoutingMode::Tiered, RoutingMode::QuotaFirst] {
            for assumed in [100, 100_000] {
                let router = router(mode, &candidates, assumed, false);
                let mut request = request(false);
                request
                    .messages
                    .push(Message::text(Role::User, "conversation ".repeat(1000)));
                let baseline = match mode {
                    RoutingMode::Tiered => router.route(&request, &[], &candidates),
                    RoutingMode::QuotaFirst => {
                        router.route_quota_first(&request, &candidates, None)
                    }
                }
                .unwrap();
                let mut actual = gateway
                    .route_with_mode(&router, &request, &[], &candidates, "test")
                    .unwrap();
                if mode == RoutingMode::Tiered {
                    assert_eq!(
                        actual.chosen,
                        candidates[usize::from(assumed == 100)].target
                    );
                }
                assert!(
                    actual.features.estimated_input_tokens
                        > baseline.features.estimated_input_tokens
                );
                actual.features.estimated_input_tokens = baseline.features.estimated_input_tokens;
                assert_eq!(
                    actual, baseline,
                    "overflow must forward with the original order"
                );
            }
        }
    }

    #[test]
    fn schema_estimates_preserve_conversation_features_and_routing_policy() {
        let gateway = gateway();
        let candidates = vec![candidate("large", 100_000)];
        for mode in [RoutingMode::Tiered, RoutingMode::QuotaFirst] {
            for output_schema in [false, true] {
                let router = router(mode, &candidates, 8192, false);
                let request = request(output_schema);
                let baseline = match mode {
                    RoutingMode::Tiered => router.route(&request, &[], &candidates),
                    RoutingMode::QuotaFirst => {
                        router.route_quota_first(&request, &candidates, None)
                    }
                }
                .unwrap();
                let mut actual = gateway
                    .route_with_mode(&router, &request, &[], &candidates, "test")
                    .unwrap();
                assert!(
                    actual.features.estimated_input_tokens
                        > baseline.features.estimated_input_tokens
                );
                assert_eq!(
                    actual.features.conversation_tokens,
                    baseline.features.conversation_tokens
                );
                actual.features.estimated_input_tokens = baseline.features.estimated_input_tokens;
                assert_eq!(
                    actual, baseline,
                    "only the reported input estimate may change"
                );
            }
        }
    }
}

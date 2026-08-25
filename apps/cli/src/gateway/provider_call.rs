// Split from gateway.rs — code moved verbatim; see gateway.rs for the module map.
#[allow(clippy::wildcard_imports)]
use super::*;

/// Which engine actually carried one attempt, and — when South was eligible
/// by configuration but legacy ran — the content-free reason why.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ProviderCallOutcome {
    pub(super) engine: RecordedProviderCallEngine,
    pub(super) south_fallback_reason: Option<SouthFallbackReason>,
}

impl ProviderCallOutcome {
    pub(super) const fn legacy(reason: SouthFallbackReason) -> Self {
        Self {
            engine: RecordedProviderCallEngine::Legacy,
            south_fallback_reason: Some(reason),
        }
    }

    pub(super) const fn south(engine: RecordedProviderCallEngine) -> Self {
        Self {
            engine,
            south_fallback_reason: None,
        }
    }
}

/// The content-free classification of a South eligibility refusal.
const fn fallback_reason_for(reason: IneligibleV1) -> SouthFallbackReason {
    match reason {
        IneligibleV1::Egress => SouthFallbackReason::Egress,
        IneligibleV1::Streaming => SouthFallbackReason::Streaming,
        IneligibleV1::Method => SouthFallbackReason::Method,
        IneligibleV1::Auth => SouthFallbackReason::Auth,
        IneligibleV1::Body => SouthFallbackReason::Body,
        IneligibleV1::SecretSource => SouthFallbackReason::SecretSource,
        IneligibleV1::Headers => SouthFallbackReason::Headers,
    }
}

pub(super) enum AttemptTerminal {
    Outcome(StreamOutcome),
    Error(ErrorEnvelope),
}

fn map_prepare_failure(
    failure: PrepareProviderCallErrorV1,
    cancellation: CancellationDispositionV1,
) -> ErrorEnvelope {
    match failure {
        PrepareProviderCallErrorV1::Ineligible(_) => ErrorEnvelope::new(
            ErrorCode::Internal,
            500,
            "ineligible provider call crossed the legacy fallback boundary",
        ),
        PrepareProviderCallErrorV1::Contract(error) => {
            map_failure_v1(StableProviderCallFailureV1::Contract(error), cancellation)
        }
        PrepareProviderCallErrorV1::Preparation(error) => map_failure_v1(
            StableProviderCallFailureV1::Provider(south_core::ProviderCallErrorV1::Preparation(
                error,
            )),
            cancellation,
        ),
    }
}

impl Gateway {
    pub(super) fn build_provider_request(
        upstream: &Upstream,
        request: &ChatRequest,
        record: &mut RequestRecord,
    ) -> Result<HttpRequestDescriptor, ErrorEnvelope> {
        let provider_protocol = upstream.config.provider.as_str();
        let descriptor = upstream
            .plugin
            .build_http_request(request, &upstream.config)
            .inspect_err(|error| {
                record_conversion(
                    record,
                    ConversionStage::ProviderRequest,
                    CANONICAL_CHAT_PROTOCOL,
                    provider_protocol,
                    false,
                    Some(error.code),
                );
            })?;
        if let Err(refusal) = upstream.config.authorize(&descriptor) {
            let error = ErrorEnvelope::new(ErrorCode::Internal, 500, refusal.to_string());
            record_conversion(
                record,
                ConversionStage::ProviderRequest,
                CANONICAL_CHAT_PROTOCOL,
                provider_protocol,
                false,
                Some(error.code),
            );
            return Err(error);
        }
        record_conversion(
            record,
            ConversionStage::ProviderRequest,
            CANONICAL_CHAT_PROTOCOL,
            provider_protocol,
            true,
            None,
        );
        Ok(descriptor)
    }

    fn response_parts(
        ctx: &RequestContext,
        upstream: &Upstream,
        response: UpstreamResponse,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<HttpResponseParts, AttemptTerminal> {
        let provider_protocol = upstream.config.provider.as_str();
        let parts = match response.into_parts() {
            Err(_) if ctx.is_cancelled() => {
                record_conversion_cancelled(
                    record,
                    ConversionStage::ProviderResponse,
                    provider_protocol,
                    CANONICAL_CHAT_PROTOCOL,
                );
                if let Some(error) = Self::lifecycle_cancellation(ctx) {
                    return Err(AttemptTerminal::Error(error));
                }
                Self::emit_cancelled(emit);
                return Err(AttemptTerminal::Outcome(StreamOutcome::ClientCancelled));
            }
            Err(error) => {
                record_conversion(
                    record,
                    ConversionStage::ProviderResponse,
                    provider_protocol,
                    CANONICAL_CHAT_PROTOCOL,
                    false,
                    Some(error.code),
                );
                return Err(AttemptTerminal::Error(error));
            }
            Ok(parts) => parts,
        };
        if ctx.is_cancelled() {
            record_conversion_cancelled(
                record,
                ConversionStage::ProviderResponse,
                provider_protocol,
                CANONICAL_CHAT_PROTOCOL,
            );
            if let Some(error) = Self::lifecycle_cancellation(ctx) {
                return Err(AttemptTerminal::Error(error));
            }
            Self::emit_cancelled(emit);
            return Err(AttemptTerminal::Outcome(StreamOutcome::ClientCancelled));
        }
        Ok(parts)
    }

    pub(super) fn classify_provider_error(
        ctx: &RequestContext,
        upstream: &Upstream,
        response: UpstreamResponse,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<ErrorEnvelope, StreamOutcome> {
        let parts = match Self::response_parts(ctx, upstream, response, emit, record) {
            Ok(parts) => parts,
            Err(AttemptTerminal::Outcome(outcome)) => return Err(outcome),
            Err(AttemptTerminal::Error(error)) => return Ok(error),
        };
        let error = match upstream.plugin.map_provider_error(&parts) {
            Ok(error) | Err(error) => error,
        };
        record_conversion(
            record,
            ConversionStage::ProviderResponse,
            upstream.config.provider.as_str(),
            CANONICAL_CHAT_PROTOCOL,
            false,
            Some(error.code),
        );
        Ok(error)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn translate_stream_response(
        ctx: &RequestContext,
        agent: &LoadedAgent,
        upstream: &Upstream,
        request: &ChatRequest,
        response: UpstreamResponse,
        inbound_tools: &Value,
        target: &UpstreamModel,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<StreamOutcome, ErrorEnvelope> {
        let sequence = NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed);
        let mut render_context = json!({
            "protocol": record.protocol,
            "stream_id": format!("stream-{sequence}"),
            "response_id": format!("msg_token_station_{sequence}"),
            "model": target.model,
            "inbound_tools": inbound_tools,
        });
        if let Some(key) = request.extensions.get(CONTINUATION_KEY_EXTENSION) {
            render_context[CONTINUATION_KEY_EXTENSION] = key.clone();
        }
        if let Some(namespaces) = request.extensions.get(TOOL_NAMESPACES_EXTENSION) {
            render_context[TOOL_NAMESPACES_EXTENSION] = namespaces.clone();
        }
        let result = Self::relay_stream(
            ctx,
            agent,
            upstream,
            response,
            &render_context,
            emit,
            record,
        );
        match &result {
            Ok(StreamOutcome::ClientCancelled) => {
                record_conversion_cancelled(
                    record,
                    ConversionStage::ProviderResponse,
                    upstream.config.provider.as_str(),
                    CANONICAL_CHAT_PROTOCOL,
                );
                record_conversion_cancelled(
                    record,
                    ConversionStage::StreamTranslate,
                    CANONICAL_CHAT_PROTOCOL,
                    agent.protocol.as_str(),
                );
            }
            Ok(StreamOutcome::Complete) => {
                record_conversion(
                    record,
                    ConversionStage::ProviderResponse,
                    upstream.config.provider.as_str(),
                    CANONICAL_CHAT_PROTOCOL,
                    true,
                    None,
                );
                record_conversion(
                    record,
                    ConversionStage::StreamTranslate,
                    CANONICAL_CHAT_PROTOCOL,
                    agent.protocol.as_str(),
                    true,
                    None,
                );
            }
            Ok(_) => {
                let error_code = record.error_code;
                record_conversion(
                    record,
                    ConversionStage::ProviderResponse,
                    upstream.config.provider.as_str(),
                    CANONICAL_CHAT_PROTOCOL,
                    false,
                    error_code,
                );
                record_conversion(
                    record,
                    ConversionStage::StreamTranslate,
                    CANONICAL_CHAT_PROTOCOL,
                    agent.protocol.as_str(),
                    false,
                    error_code,
                );
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
                annotate_conversion_failure(record, error);
                record_conversion(
                    record,
                    ConversionStage::StreamTranslate,
                    CANONICAL_CHAT_PROTOCOL,
                    agent.protocol.as_str(),
                    false,
                    Some(error.code),
                );
                annotate_conversion_failure(record, error);
            }
        }
        result
    }

    #[allow(clippy::too_many_arguments)] // one response translation boundary is explicit
    pub(super) fn translate_nonstream_response(
        ctx: &RequestContext,
        agent: &LoadedAgent,
        upstream: &Upstream,
        request: &ChatRequest,
        response: UpstreamResponse,
        inbound_tools: &Value,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<StreamOutcome, ErrorEnvelope> {
        let parts = match Self::response_parts(ctx, upstream, response, emit, record) {
            Ok(parts) => parts,
            Err(AttemptTerminal::Outcome(outcome)) => return Ok(outcome),
            Err(AttemptTerminal::Error(error)) => return Err(error),
        };
        let mut chat_response = upstream
            .plugin
            .parse_response(&parts)
            .inspect_err(|error| {
                record_conversion(
                    record,
                    ConversionStage::ProviderResponse,
                    upstream.config.provider.as_str(),
                    CANONICAL_CHAT_PROTOCOL,
                    false,
                    Some(error.code),
                );
            })?;
        chat_response.usage =
            canonical_provider_usage(upstream.config.provider.as_str(), chat_response.usage);
        record_conversion(
            record,
            ConversionStage::ProviderResponse,
            upstream.config.provider.as_str(),
            CANONICAL_CHAT_PROTOCOL,
            true,
            None,
        );
        record.usage = Some(chat_response.usage);
        let mut render_context = json!({
            "protocol": record.protocol,
            "response_id": chat_response.id,
            "model": chat_response.model,
            "inbound_tools": inbound_tools,
        });
        if let Some(key) = request.extensions.get(CONTINUATION_KEY_EXTENSION) {
            render_context[CONTINUATION_KEY_EXTENSION] = key.clone();
        }
        if let Some(namespaces) = request.extensions.get(TOOL_NAMESPACES_EXTENSION) {
            render_context[TOOL_NAMESPACES_EXTENSION] = namespaces.clone();
        }
        let rendered = agent
            .plugin
            .render_response(&chat_response, &render_context)
            .inspect_err(|error| {
                record_conversion(
                    record,
                    ConversionStage::OutboundRender,
                    CANONICAL_CHAT_PROTOCOL,
                    agent.protocol.as_str(),
                    false,
                    Some(error.code),
                );
            })?;
        record_conversion(
            record,
            ConversionStage::OutboundRender,
            CANONICAL_CHAT_PROTOCOL,
            agent.protocol.as_str(),
            true,
            None,
        );
        if !emit(Reply::BeginJson(JsonReply {
            status: 200,
            body: rendered.to_string(),
        })) {
            return Ok(StreamOutcome::ClientCancelled);
        }
        Ok(StreamOutcome::Complete)
    }

    pub(super) fn community_call_policy(
        &self,
        upstream: &Upstream,
        body_mode: RequestBodyModeV1,
    ) -> CommunityCallPolicyV1 {
        // Provenance is settled at admission, not here. A component that reached
        // this point passed source trust, the compatibility handshake, the Wasm
        // gates and the identity check; re-deciding whether its package is
        // "approved" would be the transport second-guessing a question already
        // answered, and answering it wrong meant silently dropping to Legacy.
        CommunityCallPolicyV1::new(
            self.egress.policy.mode,
            body_mode,
            upstream.auth_arms.clone(),
        )
    }

    /// Resolves the descriptor into a real HTTP exchange.
    ///
    /// The credential is read here, written into one header, and goes out of
    /// scope with the request. It never touches a log, an error, or the guest.
    #[allow(clippy::too_many_arguments)] // the engine boundary keeps every eligibility fact explicit
    pub(super) fn send_provider_call(
        &self,
        ctx: &RequestContext,
        attempt_timeout: Duration,
        upstream: &Upstream,
        descriptor: &HttpRequestDescriptor,
        upstream_name: &str,
        streaming: bool,
        attempt_deadline: std::time::Instant,
        actual_engine: &mut ProviderCallOutcome,
    ) -> Result<UpstreamResponse, ErrorEnvelope> {
        if upstream.provider_call == ConfiguredProviderCallEngine::Legacy {
            *actual_engine = ProviderCallOutcome::legacy(SouthFallbackReason::ConfiguredLegacy);
            return self.send(ctx, attempt_timeout, descriptor, upstream_name);
        }
        if streaming && upstream.provider_call == ConfiguredProviderCallEngine::SouthV1Buffered {
            *actual_engine =
                ProviderCallOutcome::legacy(SouthFallbackReason::BufferedModeCannotStream);
            return self.send(ctx, attempt_timeout, descriptor, upstream_name);
        }

        let Some(auth_config) = upstream.auth_config.as_ref() else {
            *actual_engine =
                ProviderCallOutcome::legacy(SouthFallbackReason::UnauthenticatedUpstream);
            return self.send(ctx, attempt_timeout, descriptor, upstream_name);
        };
        let body_mode = if streaming {
            RequestBodyModeV1::Streaming
        } else {
            RequestBodyModeV1::Buffered
        };
        let policy = self.community_call_policy(upstream, body_mode);
        let prepared = match if streaming {
            prepare_provider_stream_v1(&policy, &upstream.config, auth_config, descriptor)
        } else {
            prepare_provider_call_v1(&policy, &upstream.config, auth_config, descriptor)
        } {
            Ok(prepared) => prepared,
            Err(PrepareProviderCallErrorV1::Ineligible(reason)) => {
                *actual_engine = ProviderCallOutcome::legacy(fallback_reason_for(reason));
                return self.send(ctx, attempt_timeout, descriptor, upstream_name);
            }
            Err(failure) => {
                return Err(map_prepare_failure(
                    failure,
                    Self::south_cancellation_disposition(ctx),
                ));
            }
        };
        let Ok(resolver) =
            CommunityCredentialResolverV1::try_new(&self.secrets, upstream_name, auth_config)
        else {
            *actual_engine = ProviderCallOutcome::legacy(SouthFallbackReason::CredentialResolver);
            return self.send(ctx, attempt_timeout, descriptor, upstream_name);
        };
        let Some(runtime) = self.south_runtime.as_ref() else {
            *actual_engine = ProviderCallOutcome::legacy(SouthFallbackReason::NoProviderRuntime);
            return self.send(ctx, attempt_timeout, descriptor, upstream_name);
        };
        // South reports a missing credential as an opaque resolution failure,
        // which is right at the transport seam and useless to an operator.
        // The host owns the store, so it asks the same question the legacy
        // path asks — is the slot populated? — and answers with the same
        // named, value-free 401 before anything is sent.
        if let Err(detail) = self.secrets.resolve(upstream_name, &auth_config.slot) {
            *actual_engine = ProviderCallOutcome::south(if streaming {
                RecordedProviderCallEngine::SouthV1Streaming
            } else {
                RecordedProviderCallEngine::SouthV1Buffered
            });
            return Err(ErrorEnvelope::new(ErrorCode::Auth, 401, detail));
        }
        let cancellation = ctx.token().async_token();
        let deadline = tokio::time::Instant::from_std(attempt_deadline);

        // Crossing this line may resolve a credential or perform network I/O.
        // From here onward the actual attempt is South and legacy replay is forbidden.
        if streaming {
            *actual_engine =
                ProviderCallOutcome::south(RecordedProviderCallEngine::SouthV1Streaming);
            return runtime.open_stream(
                &prepared,
                &resolver,
                deadline,
                &cancellation,
                Self::south_cancellation_disposition(ctx),
                upstream_body_limit(ctx),
            );
        }

        // Give reqwest a best-effort one-millisecond lead over South's outer
        // deadline. Per-attempt client ownership and drop provide the socket
        // cleanup guarantee when scheduler timing removes that lead.
        let transport_timeout = buffered_transport_timeout(attempt_deadline, Instant::now())?;
        let buffered_transport = build_direct_reqwest_transport_v1(
            transport_timeout,
            transport_timeout,
            transport_timeout,
        )
        .map_err(|failure| map_failure_v1(failure, Self::south_cancellation_disposition(ctx)))?;
        *actual_engine = ProviderCallOutcome::south(RecordedProviderCallEngine::SouthV1Buffered);
        let response = runtime.handle.block_on(execute_prepared_provider_call_v1(
            &prepared,
            &resolver,
            &buffered_transport,
            deadline,
            &cancellation,
        ));
        drop(buffered_transport);
        let response = response.map_err(|failure| {
            map_failure_v1(failure, Self::south_cancellation_disposition(ctx))
        })?;
        Ok(UpstreamResponse::from_parts_with_limit(
            response,
            upstream_body_limit(ctx),
        ))
    }

    fn south_cancellation_disposition(ctx: &RequestContext) -> CancellationDispositionV1 {
        match ctx.cancel_reason() {
            Some(CancelReason::ClientDisconnect) => CancellationDispositionV1::ClientDisconnected,
            Some(CancelReason::ServerDrain) => CancellationDispositionV1::ServerDrain,
            Some(CancelReason::Deadline) | None => CancellationDispositionV1::Deadline,
        }
    }

    fn send(
        &self,
        ctx: &RequestContext,
        attempt_timeout: Duration,
        descriptor: &HttpRequestDescriptor,
        upstream_name: &str,
    ) -> Result<UpstreamResponse, ErrorEnvelope> {
        let timeout = ctx.remaining().min(attempt_timeout);
        let http = cancel_aware_agent(&self.egress, &self.secrets, timeout, ctx.token()).map_err(
            |detail| ErrorEnvelope::new(ErrorCode::Auth, 401, format!("egress proxy: {detail}")),
        )?;
        self.send_raw_with(&http, descriptor, upstream_name, upstream_body_limit(ctx))
    }

    /// [`Gateway::send`] over a caller-chosen agent — the probe path brings
    /// its own, with a timeout sized for diagnostics rather than generation.
    pub(super) fn send_with(
        &self,
        http: &ureq::Agent,
        descriptor: &HttpRequestDescriptor,
        upstream_name: &str,
    ) -> Result<UpstreamResponse, ErrorEnvelope> {
        let response = self.send_raw_with(http, descriptor, upstream_name, MAX_UPSTREAM_BODY)?;
        EgressPolicy::reject_redirect(response.status)?;
        Ok(response)
    }

    /// Performs the authorized first hop without following redirects. The
    /// request path consumes the raw status before enforcing the redirect
    /// rejection so its receipt can preserve the upstream's real terminal
    /// response; probes use [`Gateway::send_with`] and retain the same
    /// fail-closed behavior.
    fn send_raw_with(
        &self,
        http: &ureq::Agent,
        descriptor: &HttpRequestDescriptor,
        upstream_name: &str,
        max_body_bytes: u64,
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
        sent.map(|response| UpstreamResponse::from(response, max_body_bytes))
            .map_err(map_transport_error)
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

    /// Streams complete byte-framed SSE events through the parse/render pair.
    /// Nothing is committed to the client before the first valid event.
    #[allow(clippy::too_many_lines)] // framing, checkpoint and terminal mapping stay one state machine
    fn relay_stream(
        ctx: &RequestContext,
        agent: &LoadedAgent,
        upstream: &Upstream,
        response: UpstreamResponse,
        render_context: &Value,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<StreamOutcome, ErrorEnvelope> {
        let mut parser = upstream.plugin.stream_parser();
        let mut reader = response.into_reader();
        let mut decoder = SseFrameDecoder::default();
        let mut committed = false;
        // Whether the stream has produced any event other than its terminal
        // one. See `terminal_without_delivery`.
        let mut delivered = false;
        let mut buffer = [0u8; STREAM_READ];

        loop {
            if ctx.is_cancelled() {
                if let Some(envelope) = Self::lifecycle_cancellation(ctx) {
                    return Self::terminate_stream(
                        agent,
                        render_context,
                        emit,
                        record,
                        envelope,
                        committed,
                    );
                }
                Self::clear_stream_state(agent, render_context);
                if !committed {
                    Self::emit_cancelled(emit);
                }
                return Ok(StreamOutcome::ClientCancelled);
            }
            let read = match reader.read(&mut buffer) {
                Ok(0) => {
                    if let Err(envelope) = decoder.finish() {
                        return Self::terminate_stream(
                            agent,
                            render_context,
                            emit,
                            record,
                            envelope,
                            committed,
                        );
                    }
                    let events = match parser.finish() {
                        Ok(events) => events,
                        Err(envelope) => {
                            return Self::terminate_stream(
                                agent,
                                render_context,
                                emit,
                                record,
                                envelope,
                                committed,
                            );
                        }
                    };
                    let events =
                        canonical_provider_events(upstream.config.provider.as_str(), events);
                    for event in &events {
                        if let StreamEvent::Usage { usage } = event {
                            absorb_record_usage(record, *usage);
                        }
                    }
                    if let Some(error) = events.iter().find_map(|event| match event {
                        StreamEvent::Error { error } => Some(error.clone()),
                        _ => None,
                    }) {
                        return Self::terminate_stream(
                            agent,
                            render_context,
                            emit,
                            record,
                            error,
                            committed,
                        );
                    }
                    if Self::terminal_without_delivery(&events, delivered) {
                        return Self::terminate_stream(
                            agent,
                            render_context,
                            emit,
                            record,
                            Self::empty_stream_envelope(),
                            committed,
                        );
                    }
                    let mut terminal = false;
                    for event in events {
                        let chunk = match agent.plugin.render_stream_event(&event, render_context) {
                            Ok(chunk) => chunk,
                            Err(envelope) => {
                                return Self::terminate_stream(
                                    agent,
                                    render_context,
                                    emit,
                                    record,
                                    envelope,
                                    committed,
                                );
                            }
                        };
                        if !committed && !emit(Reply::BeginStream) {
                            Self::clear_stream_state(agent, render_context);
                            return Ok(StreamOutcome::ClientCancelled);
                        }
                        let data = chunk
                            .get("data")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        if !emit(Reply::Chunk(data)) {
                            Self::clear_stream_state(agent, render_context);
                            return Ok(StreamOutcome::ClientCancelled);
                        }
                        committed = true;
                        // No `delivered` update: this is the last batch the
                        // stream can produce, and the guard above already ran.
                        if matches!(event, StreamEvent::Done { .. }) {
                            terminal = true;
                        }
                    }
                    if terminal {
                        Self::clear_stream_state(agent, render_context);
                        return Ok(StreamOutcome::Complete);
                    }
                    return Self::terminate_stream(
                        agent,
                        render_context,
                        emit,
                        record,
                        ErrorEnvelope::new(
                            ErrorCode::TransportTruncated,
                            502,
                            "upstream SSE connection closed before a terminal event",
                        ),
                        committed,
                    );
                }
                Ok(read) => read,
                Err(error) => {
                    if ctx.is_cancelled() {
                        if let Some(envelope) = Self::lifecycle_cancellation(ctx) {
                            return Self::terminate_stream(
                                agent,
                                render_context,
                                emit,
                                record,
                                envelope,
                                committed,
                            );
                        }
                        Self::clear_stream_state(agent, render_context);
                        if !committed {
                            Self::emit_cancelled(emit);
                        }
                        return Ok(StreamOutcome::ClientCancelled);
                    }
                    let envelope = error
                        .get_ref()
                        .and_then(|source| source.downcast_ref::<SouthStreamReadFailure>())
                        .map_or_else(
                            || {
                                ErrorEnvelope::new(
                                    ErrorCode::TransportTruncated,
                                    502,
                                    "upstream connection broke while streaming",
                                )
                            },
                            |failure| {
                                map_south_stream_failure_for_attempt(
                                    failure.0,
                                    Self::south_cancellation_disposition(ctx),
                                )
                            },
                        );
                    return Self::terminate_stream(
                        agent,
                        render_context,
                        emit,
                        record,
                        envelope,
                        committed,
                    );
                }
            };

            let frames = match decoder.push(&buffer[..read]) {
                Ok(frames) => frames,
                Err(envelope) => {
                    return Self::terminate_stream(
                        agent,
                        render_context,
                        emit,
                        record,
                        envelope,
                        committed,
                    );
                }
            };

            for data in frames {
                let events = match parser.parse_chunk(&StreamChunk { data }) {
                    Ok(events) => events,
                    Err(envelope) => {
                        return Self::terminate_stream(
                            agent,
                            render_context,
                            emit,
                            record,
                            envelope,
                            committed,
                        );
                    }
                };
                let events = canonical_provider_events(upstream.config.provider.as_str(), events);
                for event in &events {
                    if let StreamEvent::Usage { usage } = event {
                        absorb_record_usage(record, *usage);
                    }
                }
                if let Some(error) = events.iter().find_map(|event| match event {
                    StreamEvent::Error { error } => Some(error.clone()),
                    _ => None,
                }) {
                    return Self::terminate_stream(
                        agent,
                        render_context,
                        emit,
                        record,
                        error,
                        committed,
                    );
                }

                if Self::terminal_without_delivery(&events, delivered) {
                    return Self::terminate_stream(
                        agent,
                        render_context,
                        emit,
                        record,
                        Self::empty_stream_envelope(),
                        committed,
                    );
                }

                let mut rendered = Vec::with_capacity(events.len());
                for event in &events {
                    let chunk = match agent.plugin.render_stream_event(event, render_context) {
                        Ok(chunk) => chunk,
                        Err(envelope) => {
                            return Self::terminate_stream(
                                agent,
                                render_context,
                                emit,
                                record,
                                envelope,
                                committed,
                            );
                        }
                    };
                    rendered.push(
                        chunk
                            .get("data")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    );
                }
                if rendered.is_empty() {
                    continue;
                }
                if !committed && !emit(Reply::BeginStream) {
                    Self::clear_stream_state(agent, render_context);
                    return Ok(StreamOutcome::ClientCancelled);
                }

                let mut terminal = false;
                for (event, data) in events.into_iter().zip(rendered) {
                    if !emit(Reply::Chunk(data)) {
                        Self::clear_stream_state(agent, render_context);
                        return Ok(StreamOutcome::ClientCancelled);
                    }
                    committed = true;
                    if matches!(event, StreamEvent::Done { .. }) {
                        terminal = true;
                    } else {
                        delivered = true;
                    }
                }
                if terminal {
                    Self::clear_stream_state(agent, render_context);
                    return Ok(StreamOutcome::Complete);
                }
            }
        }
    }

    /// Whether this batch closes the stream without anything having been
    /// delivered — the terminal event, and nothing else, ever.
    ///
    /// Adapters no longer refuse the frames that produce this shape: an empty
    /// `choices` array is a keepalive to some upstreams and the opening frame
    /// to others, so refusing it frame by frame kills streams that work
    /// (south 0.11.0's S5 ruling; the v1 package matches it). Refusing only
    /// the *stream* that delivered nothing keeps both halves: the keepalive
    /// passes through, an empty completion still does not become a success.
    ///
    /// It lives here rather than in an adapter because it is the host's
    /// guarantee, not one dialect's — which also means it covers the v2 south
    /// component, whose translation carries no such guard of its own.
    fn terminal_without_delivery(events: &[StreamEvent], delivered: bool) -> bool {
        !delivered
            && !events.is_empty()
            && events
                .iter()
                .all(|event| matches!(event, StreamEvent::Done { .. }))
    }

    fn empty_stream_envelope() -> ErrorEnvelope {
        ErrorEnvelope::new(
            ErrorCode::ProviderProtocolError,
            502,
            "the upstream stream ended without delivering any event",
        )
    }

    fn terminate_stream(
        agent: &LoadedAgent,
        render_context: &Value,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
        mut envelope: ErrorEnvelope,
        committed: bool,
    ) -> Result<StreamOutcome, ErrorEnvelope> {
        if !committed {
            Self::clear_stream_state(agent, render_context);
            return Err(envelope);
        }
        sanitize_attempt_error_for_render(&mut envelope);
        record.error_code = Some(envelope.code);
        let rendered = Self::render_stream_error(agent, &envelope, render_context);
        if !emit(Reply::Chunk(rendered)) {
            Self::clear_stream_state(agent, render_context);
            return Ok(StreamOutcome::ClientCancelled);
        }
        Self::clear_stream_state(agent, render_context);
        Ok(StreamOutcome::FailedAfterPartial)
    }
}

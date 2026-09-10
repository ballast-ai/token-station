#[allow(clippy::wildcard_imports)]
use super::*;

/// Release a normalized request reservation on every exit, including admission
/// failures before the first upstream attempt. Completed histories survive it.
struct NativeContinuationGuard<'a> {
    agent: &'a LoadedAgent,
    context: Value,
}

impl Drop for NativeContinuationGuard<'_> {
    fn drop(&mut self) {
        Gateway::clear_stream_state(self.agent, &self.context);
    }
}

fn responses_error_code(status: u16) -> ErrorCode {
    match status {
        401 | 403 => ErrorCode::Auth,
        429 => ErrorCode::RateLimit,
        server if server >= 500 => ErrorCode::UpstreamUnavailable,
        _ => ErrorCode::InvalidRequest,
    }
}

fn declares_active_web_search(body: &Value) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|tool| {
            tool.get("type").and_then(Value::as_str) == Some("web_search")
                && tool.get("external_web_access").and_then(Value::as_bool) != Some(false)
        })
}

fn responses_wire_usage(usage: &Value) -> Option<Usage> {
    let field = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0);
    let detail = |group: &str, name: &str| {
        usage
            .get(group)
            .and_then(|details| details.get(name))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let parsed = Usage {
        input_tokens: field("input_tokens"),
        output_tokens: field("output_tokens"),
        cache_read_tokens: detail("input_tokens_details", "cached_tokens"),
        cache_write_tokens: 0,
        cache_write_5m_tokens: 0,
        cache_write_1h_tokens: 0,
        reasoning_tokens: detail("output_tokens_details", "reasoning_tokens"),
    };
    (parsed != Usage::default()).then_some(parsed)
}

#[derive(Default)]
struct ResponsesSseUsageTap {
    partial: String,
    usage: Option<Usage>,
    saw_terminal: bool,
    abandoned: bool,
    terminal_response: Option<Value>,
}

impl ResponsesSseUsageTap {
    const MAX_LINE: usize = 256 * 1024;

    fn observe(&mut self, chunk: &str) {
        if self.abandoned {
            return;
        }
        self.partial.push_str(chunk);
        while let Some(end) = self.partial.find('\n') {
            let line = self.partial[..end].trim_end_matches('\r').to_owned();
            self.partial.drain(..=end);
            self.observe_line(&line);
        }
        if self.partial.len() > Self::MAX_LINE {
            self.abandoned = true;
            self.partial.clear();
        }
    }

    fn observe_line(&mut self, line: &str) {
        let Some(payload) = line.strip_prefix("data:") else {
            return;
        };
        let Ok(event) = serde_json::from_str::<Value>(payload.trim()) else {
            return;
        };
        if matches!(
            event.get("type").and_then(Value::as_str),
            Some("response.completed" | "response.failed" | "response.incomplete")
        ) {
            self.saw_terminal = true;
        }
        if line.len() <= Self::MAX_LINE
            && event.get("type").and_then(Value::as_str) == Some("response.completed")
        {
            self.terminal_response = event.get("response").cloned();
        }
        let found = event
            .get("response")
            .and_then(|response| response.get("usage"))
            .or_else(|| event.get("usage"))
            .and_then(responses_wire_usage);
        if let Some(found) = found {
            self.usage.get_or_insert_with(Usage::default).absorb(found);
        }
    }

    fn finish(&mut self) {
        if self.abandoned || self.partial.is_empty() {
            return;
        }
        let line = std::mem::take(&mut self.partial);
        self.observe_line(line.trim_end_matches('\r'));
    }
}

impl Gateway {
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)] // native admission, routing, and relay form one pipeline
    pub(super) fn try_responses_passthrough(
        &self,
        ctx: &RequestContext,
        agent: &LoadedAgent,
        router: &Router,
        raw_headers: &[(String, String)],
        body: &[u8],
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<Option<(UpstreamModel, StreamOutcome)>, ErrorEnvelope> {
        if body.len() > MAX_INBOUND_BODY {
            return Ok(None);
        }
        let Ok(body_value) = serde_json::from_slice::<Value>(body) else {
            return Ok(None);
        };
        if !declares_active_web_search(&body_value) {
            return Ok(None);
        }
        let Some(model) = body_value
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return Ok(None);
        };
        let stream = body_value
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let mut mini = ChatRequest::new(&model, Vec::new());
        mini.stream = stream;
        add_native_image_requirement(&mut mini, &body_value);
        mini.tools.push(ToolDef {
            name: "responses_native_routing_probe".to_owned(),
            description: None,
            parameters: json!({}),
        });
        let (quota_now_ms, session) =
            Self::quota_preamble(router, || format!("responses-native:{model}"));
        let candidates: Vec<Candidate> = self
            .candidates(std::time::Instant::now(), quota_now_ms)
            .into_iter()
            .filter(|candidate| {
                self.upstreams
                    .get(candidate.target.upstream.as_str())
                    .is_some_and(|upstream| upstream.dialect == ApiDialect::ResponsesNative)
            })
            .collect();
        let mut decision = match self.route_with_mode(router, &mini, &[], &candidates, &session) {
            Ok(decision) => decision,
            Err(error) if raw_contains_images(&body_value) => return Err(route_error(&error)),
            Err(_) => return Ok(None),
        };
        let Some(upstream) = self.upstreams.get(decision.chosen.upstream.as_str()) else {
            return Ok(None);
        };
        if upstream.dialect != ApiDialect::ResponsesNative {
            return Ok(None);
        }
        decision.fallbacks.retain(|target| {
            self.upstreams
                .get(target.upstream.as_str())
                .is_some_and(|upstream| upstream.dialect == ApiDialect::ResponsesNative)
        });

        let headers = Self::curate_responses_headers(raw_headers)?;
        let configured = self.catalog.iter().any(|(target, _)| target.model == model);
        record.requested_model = canonical_requested_model(&model, configured);
        record.stream = stream;

        let last_upstream_error = RefCell::new(None);
        let result = self.execute_routed_attempt(
            ctx,
            agent,
            &AttemptPayload::ResponsesNative {
                body: &body_value,
                headers: &headers,
                stream,
                last_upstream_error: &last_upstream_error,
            },
            &json!({}),
            &decision,
            &candidates,
            quota_now_ms,
            &session,
            emit,
            record,
        );
        if let Some(raw) = last_upstream_error.borrow_mut().take().filter(|_| {
            result
                .as_ref()
                .is_err_and(|error| error.code != ErrorCode::Capability)
        }) {
            record.status = raw.status;
            record.error_code = Some(responses_error_code(raw.status));
            emit(Reply::BeginJson(JsonReply {
                status: raw.status,
                body: raw.body,
            }));
            return Ok(Some((raw.target, StreamOutcome::FailedBeforeOutput)));
        }
        result.map(Some)
    }

    /// Execute an ordinary Responses request after its single normalization and
    /// the shared policy gates. Native transport must not choose another route.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(super) fn execute_normalized_responses(
        &self,
        ctx: &RequestContext,
        agent: &LoadedAgent,
        request: &ChatRequest,
        raw_headers: &[(String, String)],
        body: &[u8],
        decision: &Decision,
        candidates: &[Candidate],
        quota_now_ms: Option<u64>,
        session: &str,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<(UpstreamModel, StreamOutcome), ErrorEnvelope> {
        let mut render_context = json!({
            "stream_id": record.request_id,
            "response_id": record.request_id,
            "model": request.model,
        });
        if let Some(key) = request.extensions.get(CONTINUATION_KEY_EXTENSION) {
            render_context[CONTINUATION_KEY_EXTENSION] = key.clone();
        }
        let continuation = NativeContinuationGuard {
            agent,
            context: render_context,
        };
        let render_context = &continuation.context;
        let mut forwarded: Value = serde_json::from_slice(body)
            .map_err(|_| ErrorEnvelope::new(ErrorCode::InvalidRequest, 400, "body is not JSON"))?;
        if let Some(input) = request.extensions.get("token_station_private_native_input") {
            forwarded["input"] = input.clone();
            forwarded
                .as_object_mut()
                .expect("normalized Responses object")
                .remove("previous_response_id");
        } else if forwarded
            .get("previous_response_id")
            .is_some_and(|id| !id.is_null())
        {
            return Err(ErrorEnvelope::new(
                ErrorCode::Capability,
                400,
                "Native continuation history is unavailable. Send the complete conversation again.",
            ));
        }
        forwarded
            .as_object_mut()
            .expect("normalized Responses object")
            .retain(|key, _| !key.starts_with("token_station_private_"));
        let headers = Self::curate_responses_headers(raw_headers)?;
        let mut decision = decision.clone();
        decision.fallbacks.retain(|target| {
            self.upstreams
                .get(target.upstream.as_str())
                .is_some_and(|upstream| upstream.dialect == ApiDialect::ResponsesNative)
        });
        let mut tap = ResponsesSseUsageTap::default();
        let last_upstream_error = RefCell::new(None);
        let result = self.execute_routed_attempt(
            ctx,
            agent,
            &AttemptPayload::ResponsesNative {
                body: &forwarded,
                headers: &headers,
                stream: request.stream,
                last_upstream_error: &last_upstream_error,
            },
            &Value::Null,
            &decision,
            candidates,
            quota_now_ms,
            session,
            &mut |reply| {
                let completed = match &reply {
                    Reply::BeginJson(reply) if reply.status == 200 => {
                        serde_json::from_str::<Value>(&reply.body).ok()
                    }
                    Reply::Chunk(chunk) => {
                        tap.observe(chunk);
                        tap.terminal_response.take()
                    }
                    _ => None,
                };
                if let Some(response) = completed {
                    Self::remember_native_response(agent, render_context, response);
                }
                emit(reply)
            },
            record,
        );
        tap.finish();
        if let Some(response) = tap.terminal_response.take() {
            Self::remember_native_response(agent, render_context, response);
        }
        if let Some(raw) = last_upstream_error.borrow_mut().take().filter(|_| {
            result
                .as_ref()
                .is_err_and(|error| error.code != ErrorCode::Capability)
        }) {
            record.status = raw.status;
            record.error_code = Some(responses_error_code(raw.status));
            emit(Reply::BeginJson(JsonReply {
                status: raw.status,
                body: raw.body,
            }));
            return Ok((raw.target, StreamOutcome::FailedBeforeOutput));
        }
        result
    }

    fn remember_native_response(agent: &LoadedAgent, context: &Value, native: Value) {
        if context.get(CONTINUATION_KEY_EXTENSION).is_none() {
            return;
        }
        let mut context = context.clone();
        context["token_station_private_native_response"] = native;
        let placeholder = ChatResponse {
            id: String::new(),
            model: String::new(),
            choices: Vec::new(),
            usage: Usage::default(),
            extensions: token_station_protocol::Extensions::new(),
        };
        // Protocol validation and bounded continuation retention belong to the
        // Responses adapter. An unsupported history must not alter a raw reply.
        let _ = agent.plugin.render_response(&placeholder, &context);
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(super) fn responses_native_attempt(
        &self,
        ctx: &RequestContext,
        target: &UpstreamModel,
        headers: &SafeHeaders,
        body: &Value,
        stream: bool,
        attempt_timeout: Duration,
        attempt_deadline: std::time::Instant,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
        upstream_http_status: &mut Option<u16>,
        provider_call_engine: &mut ProviderCallOutcome,
        last_upstream_error: &RefCell<Option<RawUpstreamError>>,
    ) -> Result<StreamOutcome, ErrorEnvelope> {
        const RESPONSES: &str = "openai-responses";
        last_upstream_error.borrow_mut().take();
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
        if upstream.dialect != ApiDialect::ResponsesNative {
            return Err(ErrorEnvelope::new(
                ErrorCode::Capability,
                400,
                "native Responses payload requires a responses-native upstream",
            ));
        }

        let mut forwarded = body.clone();
        if let Some(object) = forwarded.as_object_mut() {
            object.insert("model".to_owned(), json!(target.model));
        }
        let mut descriptor = HttpRequestDescriptor::new(
            HttpMethod::Post,
            upstream.config.base_url.resolve(ProviderApi::Responses),
        );
        descriptor.headers = headers.clone();
        descriptor.body = Some(forwarded);
        descriptor.auth = upstream.config.auth.clone().map(Auth::bearer);

        if let Err(refusal) = upstream.config.authorize(&descriptor) {
            let error = ErrorEnvelope::new(ErrorCode::Internal, 500, refusal.to_string());
            record_conversion(
                record,
                ConversionStage::ProviderRequest,
                RESPONSES,
                RESPONSES,
                false,
                Some(error.code),
            );
            return Err(error);
        }
        record_conversion(
            record,
            ConversionStage::ProviderRequest,
            RESPONSES,
            RESPONSES,
            true,
            None,
        );
        ctx.capture_upstream_request(target.upstream.as_str(), &target.model, &descriptor);

        let response = match self.send_provider_call(
            ctx,
            attempt_timeout,
            upstream,
            &descriptor,
            target.upstream.as_str(),
            stream,
            attempt_deadline,
            provider_call_engine,
        ) {
            Err(error) if ctx.is_cancelled() => {
                if matches!(ctx.cancel_reason(), Some(CancelReason::ClientDisconnect)) {
                    Self::emit_cancelled(emit);
                    return Ok(StreamOutcome::ClientCancelled);
                }
                return Err(error);
            }
            Err(error) => {
                record_conversion(
                    record,
                    ConversionStage::ProviderResponse,
                    RESPONSES,
                    RESPONSES,
                    false,
                    Some(error.code),
                );
                return Err(error);
            }
            Ok(response) => response,
        };
        *upstream_http_status = Some(response.status);
        ctx.capture_upstream_response_head(response.status, &response.headers);
        if let Err(error) = EgressPolicy::reject_redirect(response.status) {
            record_conversion(
                record,
                ConversionStage::ProviderResponse,
                RESPONSES,
                RESPONSES,
                false,
                Some(error.code),
            );
            return Err(error);
        }
        let windows = crate::quota_headers::parse_quota_windows(&response.headers, unix_millis());
        self.quota.lock().expect("quota lock").note_authoritative(
            target.upstream.as_str(),
            unix_millis(),
            windows,
        );

        if response.status >= 400 {
            let code = responses_error_code(response.status);
            let parts = response.into_parts()?;
            ctx.append_upstream_response_body(parts.body.as_bytes());
            if raw_contains_images(body) && native_rejects_image(parts.status, &parts.body) {
                return Err(upstream_image_error());
            }

            record_conversion(
                record,
                ConversionStage::ProviderResponse,
                RESPONSES,
                RESPONSES,
                false,
                Some(code),
            );
            if code.is_retriable_elsewhere() {
                *last_upstream_error.borrow_mut() = Some(RawUpstreamError {
                    target: target.clone(),
                    status: parts.status,
                    body: parts.body,
                });
                return Err(ErrorEnvelope::new(
                    code,
                    parts.status,
                    "upstream refused the native Responses request",
                ));
            }
            record.status = parts.status;
            record.error_code = Some(code);
            emit(Reply::BeginJson(JsonReply {
                status: parts.status,
                body: parts.body,
            }));
            return Ok(StreamOutcome::FailedBeforeOutput);
        }

        record_conversion(
            record,
            ConversionStage::ProviderResponse,
            RESPONSES,
            RESPONSES,
            true,
            None,
        );
        if stream {
            return Self::relay_responses_sse(ctx, attempt_deadline, response, emit, record);
        }
        let parts = response.into_parts()?;
        ctx.append_upstream_response_body(parts.body.as_bytes());
        if let Some(usage) = serde_json::from_str::<Value>(&parts.body)
            .ok()
            .as_ref()
            .and_then(|body| body.get("usage"))
            .and_then(responses_wire_usage)
        {
            record.usage = Some(usage);
        }
        emit(Reply::BeginJson(JsonReply {
            status: parts.status,
            body: parts.body,
        }));
        Ok(StreamOutcome::Complete)
    }

    #[allow(clippy::too_many_lines)] // Raw relay owns one streaming lifecycle and cleanup path.
    fn relay_responses_sse(
        ctx: &RequestContext,
        attempt_deadline: std::time::Instant,
        response: UpstreamResponse,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<StreamOutcome, ErrorEnvelope> {
        let mut tap = ResponsesSseUsageTap::default();
        let mut reader = response.into_reader();
        let mut buffer = [0u8; STREAM_READ];
        let mut committed = false;
        let mut pending = Vec::new();

        loop {
            if ctx.is_cancelled() {
                if let Some(error) = Self::lifecycle_cancellation(ctx) {
                    if !committed {
                        return Err(error);
                    }
                    record.error_code = Some(error.code);
                    record.status = error.http_status;
                    return Ok(StreamOutcome::FailedAfterPartial);
                }
                if !committed {
                    Self::emit_cancelled(emit);
                }
                return Ok(StreamOutcome::ClientCancelled);
            }

            match reader.read(&mut buffer) {
                Ok(0) => {
                    if !pending.is_empty() {
                        let tail = String::from_utf8_lossy(&pending).into_owned();
                        tap.observe(&tail);
                        if !emit(Reply::Chunk(tail)) {
                            return Ok(StreamOutcome::ClientCancelled);
                        }
                    }
                    tap.finish();
                    if let Some(usage) = tap.usage {
                        record.usage = Some(usage);
                    }
                    if tap.saw_terminal {
                        return Ok(StreamOutcome::Complete);
                    }
                    let error = ErrorEnvelope::new(
                        ErrorCode::TransportTruncated,
                        502,
                        "upstream Responses stream ended before a terminal response event",
                    );
                    if committed {
                        record.error_code = Some(error.code);
                        record.status = error.http_status;
                        return Ok(StreamOutcome::FailedAfterPartial);
                    }
                    return Err(error);
                }
                Ok(read) => {
                    ctx.append_upstream_response_body(&buffer[..read]);
                    pending.extend_from_slice(&buffer[..read]);
                    let boundary = match std::str::from_utf8(&pending) {
                        Ok(_) => pending.len(),
                        Err(error) if error.error_len().is_none() => error.valid_up_to(),
                        Err(_) => {
                            let error = ErrorEnvelope::new(
                                ErrorCode::ProviderProtocolError,
                                502,
                                "upstream Responses stream contains invalid UTF-8",
                            );
                            if committed {
                                record.error_code = Some(error.code);
                                record.status = error.http_status;
                                return Ok(StreamOutcome::FailedAfterPartial);
                            }
                            return Err(error);
                        }
                    };
                    if boundary == 0 {
                        continue;
                    }
                    let chunk = String::from_utf8(pending.drain(..boundary).collect())
                        .expect("valid up to the boundary");
                    if !committed && !emit(Reply::BeginStream) {
                        return Ok(StreamOutcome::ClientCancelled);
                    }
                    committed = true;
                    tap.observe(&chunk);
                    if !emit(Reply::Chunk(chunk)) {
                        return Ok(StreamOutcome::ClientCancelled);
                    }
                }
                Err(_) => {
                    if ctx.is_cancelled() {
                        if let Some(error) = Self::lifecycle_cancellation(ctx) {
                            if !committed {
                                return Err(error);
                            }
                            record.error_code = Some(error.code);
                            record.status = error.http_status;
                            return Ok(StreamOutcome::FailedAfterPartial);
                        }
                        if !committed {
                            Self::emit_cancelled(emit);
                        }
                        return Ok(StreamOutcome::ClientCancelled);
                    }
                    if std::time::Instant::now() >= attempt_deadline {
                        let error = ErrorEnvelope::new(
                            ErrorCode::Timeout,
                            504,
                            "upstream attempt deadline exceeded",
                        );
                        if committed {
                            record.error_code = Some(error.code);
                            record.status = error.http_status;
                            return Ok(StreamOutcome::FailedAfterPartial);
                        }
                        return Err(error);
                    }
                    if committed {
                        record.error_code = Some(ErrorCode::TransportTruncated);
                        if record.status < 400 {
                            record.status = 502;
                        }
                        return Ok(StreamOutcome::FailedAfterPartial);
                    }
                    return Err(ErrorEnvelope::new(
                        ErrorCode::TransportTruncated,
                        502,
                        "upstream connection broke while streaming Responses events",
                    ));
                }
            }
        }
    }

    fn curate_responses_headers(
        raw_headers: &[(String, String)],
    ) -> Result<SafeHeaders, ErrorEnvelope> {
        const FORWARD: [&str; 4] = [
            "content-type",
            "accept",
            "openai-organization",
            "openai-project",
        ];
        let mut kept: Vec<(String, String)> = raw_headers
            .iter()
            .filter(|(name, _)| FORWARD.contains(&name.to_ascii_lowercase().as_str()))
            .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
            .collect();
        if !kept.iter().any(|(name, _)| name == "content-type") {
            kept.push(("content-type".to_owned(), "application/json".to_owned()));
        }
        SafeHeaders::try_new(kept).map_err(|error| {
            ErrorEnvelope::new(
                ErrorCode::Internal,
                500,
                format!("passthrough header rejected: {error}"),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ResponsesSseUsageTap, responses_wire_usage};

    #[test]
    fn responses_usage_reads_cache_and_reasoning_details() {
        let usage = responses_wire_usage(&serde_json::json!({
            "input_tokens": 100,
            "output_tokens": 30,
            "input_tokens_details": {"cached_tokens": 80},
            "output_tokens_details": {"reasoning_tokens": 20}
        }))
        .expect("usage");

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 30);
        assert_eq!(usage.cache_read_tokens, 80);
        assert_eq!(usage.reasoning_tokens, 20);
    }

    #[test]
    fn responses_stream_requires_a_terminal_event() {
        let mut tap = ResponsesSseUsageTap::default();
        tap.observe(
            "data: {\"type\":\"response.web_search_call.completed\",\"item_id\":\"ws_1\"}\n\n",
        );
        assert!(!tap.saw_terminal);

        tap.observe(concat!(
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{",
            "\"input_tokens\":13,\"output_tokens\":5}}}\n\n"
        ));
        assert!(tap.saw_terminal);
        let usage = tap.usage.expect("terminal usage");
        assert_eq!(usage.input_tokens, 13);
        assert_eq!(usage.output_tokens, 5);
    }

    #[test]
    #[cfg(feature = "builtin-plugins")]
    fn missing_native_history_releases_pending_continuation_on_early_return() {
        use super::{CONTINUATION_KEY_EXTENSION, Gateway, RequestContext, RequestRecord};
        use serde_json::json;
        use std::{sync::Arc, time::Duration};
        use token_station_protocol::ErrorCode;
        use token_station_router_core::{
            DecidedBy, Decision, RequestFeatures, UpstreamModel, UpstreamRef,
        };

        // A waiting gateway loads the real builtin Agent but has no upstream
        // or credential. This test cannot send a provider request.
        let directory =
            std::env::temp_dir().join(format!("ts-native-pending-cleanup-{}", std::process::id()));
        let config = serde_json::from_value(json!({
            "version": 1,
            "server": {"listen": "127.0.0.1:0"},
            "data": {"dir": directory, "metrics": false},
            "plugins": {"dir": directory.join("absent-plugins"),
                "agents": ["agent-openai-responses"], "providers": {}},
            "upstreams": {}, "routing": {"mode": "direct"},
            "router": {"version": 1, "pools": {}, "default_pool": ""}
        }))
        .expect("waiting gateway config parses");
        let gateway = Gateway::new(&config, Arc::new(token_station_metrics::NoopRecorder))
            .expect("builtin Responses Agent loads without an upstream");
        let agent = gateway
            .agents
            .iter()
            .find(|agent| agent.protocol == "openai-responses")
            .expect("real Responses Agent is loaded");
        let original = json!({"model": "auto", "input": [{"role": "user", "content": [
            {"type": "input_image", "image_url": "data:image/png;base64,cGl4ZWxz"}
        ]}]})
        .to_string()
        .into_bytes();
        let mut record = RequestRecord::begin(1, "openai-responses");
        record.request_id = "native-pending-cleanup".into();
        let normalize = |record: &mut RequestRecord| {
            Gateway::normalize_request(agent, "POST", "/v1/responses", &[], &original, record)
        };
        let (mut request, _, _) =
            normalize(&mut record).expect("first normalization reserves the key");
        let key = request.extensions[CONTINUATION_KEY_EXTENSION].clone();
        assert!(key.is_string(), "a real pending reservation is required");
        // Establish that reusing the key really fails while the reservation
        // exists. The success below therefore proves cleanup, not cache bypass.
        let duplicate = normalize(&mut record).expect_err("the key is already reserved");
        assert!(duplicate.message.contains("already in flight"));
        assert!(
            request
                .extensions
                .remove("token_station_private_native_input")
                .is_some()
        );
        let decision = Decision {
            chosen: UpstreamModel::new(UpstreamRef::new("unreachable").unwrap(), "model"),
            fallbacks: Vec::new(),
            decided_by: DecidedBy::Default,
            features: RequestFeatures::extract(&request, &[]),
            pool: "unreachable".into(),
        };
        let mut continued: serde_json::Value = serde_json::from_slice(&original).unwrap();
        continued["previous_response_id"] = json!("resp_prior_translated");
        let ctx = RequestContext::detached(Duration::from_secs(2), Duration::from_secs(1));
        let mut emitted = false;
        let error = gateway
            .execute_normalized_responses(
                &ctx,
                agent,
                &request,
                &[],
                continued.to_string().as_bytes(),
                &decision,
                &[],
                None,
                "",
                &mut |_| {
                    emitted = true;
                    true
                },
                &mut record,
            )
            .expect_err("missing native history refuses before dispatch");
        assert_eq!(error.code, ErrorCode::Capability);
        assert_eq!(error.http_status, 400);
        assert!(
            error
                .message
                .contains("Native continuation history is unavailable")
        );
        assert!(!emitted);
        assert_eq!(record.attempts, 0);

        let (again, _, _) = normalize(&mut record)
            .expect("early refusal must release the pending key before returning");
        assert_eq!(again.extensions[CONTINUATION_KEY_EXTENSION], key);
    }
}

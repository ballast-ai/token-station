// Split from gateway.rs — code moved verbatim; see gateway.rs for the module map.
#[allow(clippy::wildcard_imports)]
use super::*;

/// Reads Anthropic's wire `usage` object into the canonical [`Usage`].
///
/// The native path relays the provider's own bytes, so nothing upstream of it
/// ever builds a `ChatResponse` — and `record.usage` stayed `None`. That one
/// gap reached three places: pricing skips a request with no usage, budgets
/// therefore show a native turn as free, and `quota.record` is guarded on
/// `record.usage` too, so consumption-aware routing kept believing an account
/// it had just spent was untouched.
fn anthropic_wire_usage(usage: &serde_json::Value) -> Option<Usage> {
    let field = |name: &str| usage.get(name).and_then(serde_json::Value::as_u64);
    let tier = |name: &str| {
        usage
            .get("cache_creation")
            .and_then(|creation| creation.get(name))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let parsed = canonical_provider_usage(
        ANTHROPIC_PROVIDER,
        Usage {
            input_tokens: field("input_tokens").unwrap_or(0),
            output_tokens: field("output_tokens").unwrap_or(0),
            cache_read_tokens: field("cache_read_input_tokens").unwrap_or(0),
            cache_write_tokens: field("cache_creation_input_tokens").unwrap_or(0),
            cache_write_5m_tokens: tier("ephemeral_5m_input_tokens"),
            cache_write_1h_tokens: tier("ephemeral_1h_input_tokens"),
            reasoning_tokens: 0,
        },
    );
    // An object with none of the fields we understand is not a usage report;
    // recording zeros would claim a free turn rather than an unknown one.
    (parsed != Usage::default()).then_some(parsed)
}

#[cfg(test)]
mod usage_tests {
    use super::{AnthropicSseUsageTap, anthropic_wire_usage};

    #[test]
    fn wire_input_becomes_one_inclusive_canonical_total() {
        let usage = anthropic_wire_usage(&serde_json::json!({
            "input_tokens": 30,
            "output_tokens": 12,
            "cache_read_input_tokens": 100,
            "cache_creation_input_tokens": 20
        }))
        .expect("usage");

        assert_eq!(usage.input_tokens, 150);
        assert_eq!(usage.cache_read_tokens, 100);
        assert_eq!(usage.cache_write_tokens, 20);
        assert_eq!(usage.total(), 162);
    }

    #[test]
    fn streaming_usage_keeps_input_buckets_when_output_arrives_later() {
        let mut tap = AnthropicSseUsageTap::default();
        tap.observe(concat!(
            "event: message_start\n",
            "data: {\"message\":{\"usage\":{\"input_tokens\":30,",
            "\"cache_read_input_tokens\":100,\"cache_creation_input_tokens\":20}}}\n\n",
            "event: message_delta\n",
            "data: {\"usage\":{\"output_tokens\":12}}\n\n"
        ));

        let usage = tap.into_usage().expect("usage");
        assert_eq!(usage.input_tokens, 150);
        assert_eq!(usage.output_tokens, 12);
        assert_eq!(usage.cache_read_tokens, 100);
        assert_eq!(usage.cache_write_tokens, 20);
    }

    #[test]
    fn streaming_completion_requires_anthropic_message_stop() {
        let mut tap = AnthropicSseUsageTap::default();
        tap.observe("data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":12}}\n\n");
        assert!(!tap.is_complete());

        tap.observe("data: {\"type\":\"message_stop\"}");
        tap.finish();
        assert!(tap.is_complete());
    }
}

/// Pulls usage out of a relayed Anthropic SSE stream without altering a byte
/// of it.
///
/// Anthropic reports usage twice — `message_start` carries the input buckets,
/// `message_delta` the final output count — which is exactly the shape
/// [`Usage::absorb`] exists for. The tap is line-oriented so it never needs
/// the whole stream in memory, and it gives up rather than grow without bound
/// if a peer sends no newline.
#[derive(Default)]
struct AnthropicSseUsageTap {
    partial: String,
    usage: Option<Usage>,
    saw_message_stop: bool,
    abandoned: bool,
}

impl AnthropicSseUsageTap {
    /// A single SSE line far beyond any real Anthropic event. Past this the
    /// peer is not sending frames and the tap stops looking.
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
            self.partial = String::new();
        }
    }

    fn observe_line(&mut self, line: &str) {
        let Some(payload) = line.strip_prefix("data:") else {
            return;
        };
        let Ok(event) = serde_json::from_str::<serde_json::Value>(payload.trim()) else {
            return;
        };
        if event.get("type").and_then(serde_json::Value::as_str) == Some("message_stop") {
            self.saw_message_stop = true;
        }
        // `message_start` nests usage under `message`; `message_delta` puts it
        // at the top level beside `delta`.
        let found = event
            .get("usage")
            .or_else(|| {
                event
                    .get("message")
                    .and_then(|message| message.get("usage"))
            })
            .and_then(anthropic_wire_usage);
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

    const fn is_complete(&self) -> bool {
        self.saw_message_stop
    }

    fn into_usage(self) -> Option<Usage> {
        self.usage
    }
}

fn raw_user_content_prefers_chinese(content: &Value) -> Option<bool> {
    match content {
        Value::String(text) if !text.trim().is_empty() => Some(contains_han(text)),
        Value::Array(parts) => {
            let texts: Vec<&str> = parts
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .filter(|text| !text.trim().is_empty())
                .collect();
            (!texts.is_empty()).then(|| texts.into_iter().any(contains_han))
        }
        _ => None,
    }
}

fn raw_request_prefers_chinese(body: &Value) -> bool {
    body.get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .rev()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .find_map(|message| {
            message
                .get("content")
                .and_then(raw_user_content_prefers_chinese)
        })
        .unwrap_or(false)
}

fn anthropic_document_replacement(block: &Value, prefers_chinese: bool) -> (Value, bool) {
    let (text, extracted) = document_replacement_text(block, prefers_chinese);
    (json!({"type": "text", "text": text}), extracted)
}

fn replace_anthropic_documents_in_content(
    content: &mut Value,
    prefers_chinese: bool,
    stats: &mut DocumentFallbackStats,
) {
    let Value::Array(parts) = content else {
        return;
    };
    for part in parts {
        match part.get("type").and_then(Value::as_str) {
            Some("document") => {
                let (replacement, extracted) =
                    anthropic_document_replacement(part, prefers_chinese);
                *part = replacement;
                if extracted {
                    stats.extracted += 1;
                } else {
                    stats.omitted += 1;
                }
            }
            Some("tool_result") => {
                if let Some(nested) = part.get_mut("content") {
                    replace_anthropic_documents_in_content(nested, prefers_chinese, stats);
                }
            }
            _ => {}
        }
    }
}

fn replace_anthropic_documents(body: &mut Value) -> DocumentFallbackStats {
    let prefers_chinese = raw_request_prefers_chinese(body);
    let mut stats = DocumentFallbackStats::default();
    for message in body
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(content) = message.get_mut("content") {
            replace_anthropic_documents_in_content(content, prefers_chinese, &mut stats);
        }
    }
    stats
}

fn replace_anthropic_images(body: &mut Value) -> usize {
    let marker = if raw_request_prefers_chinese(body) {
        MEDIA_FALLBACK_ZH
    } else {
        MEDIA_FALLBACK_EN
    };
    body.get_mut("messages")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter_map(|message| message.get_mut("content").and_then(Value::as_array_mut))
        .map(|parts| {
            let mut replaced = 0;
            for part in parts {
                if part.get("type").and_then(Value::as_str) == Some("image") {
                    let cache_control = part.get("cache_control").cloned();
                    *part = json!({ "type": "text", "text": marker });
                    if let (Some(cache_control), Some(object)) =
                        (cache_control, part.as_object_mut())
                    {
                        object.insert("cache_control".to_owned(), cache_control);
                    }
                    replaced += 1;
                }
            }
            replaced
        })
        .sum()
}

/// A receipt-only [`ErrorCode`] for an upstream HTTP status on the passthrough
/// path. The client already received the verbatim body; this only shapes the
/// receipt and the health verdict — a 5xx counts toward ejection (real server
/// trouble), while 4xx / 429 / auth do not (a client or quota problem, not an
/// unwell upstream).
fn passthrough_error_code(status: u16) -> ErrorCode {
    match status {
        401 | 403 => ErrorCode::Auth,
        429 => ErrorCode::RateLimit,
        server if server >= 500 => ErrorCode::UpstreamUnavailable,
        _ => ErrorCode::InvalidRequest,
    }
}

/// Whether an Anthropic Messages body declares a tool the upstream executes
/// itself. Mirrors `agent-anthropic`'s `is_anthropic_server_tool`: the six
/// families Anthropic runs server-side, by `type` prefix.
fn anthropic_request_declares_server_tool(body: &Value) -> bool {
    const SERVER_TOOL_PREFIXES: [&str; 6] = [
        "web_search_",
        "web_fetch_",
        "code_execution_",
        "tool_search_",
        "mcp_",
        "advisor_",
    ];
    body.get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| SERVER_TOOL_PREFIXES.iter().any(|p| kind.starts_with(p)))
            })
        })
}

/// The same conversation key as [`quota_session_key`], read off the Anthropic
/// wire instead of the Canonical IR.
///
/// Prompt-cache affinity is the reason both exist: a follow-up turn should land
/// on the account that already holds the prefix. The native payload has no IR
/// to hash, but it has the same first two turns, so the key is derivable without
/// normalising the body — which is precisely what the native path exists to
/// avoid doing.
fn native_quota_session_key(body: &Value) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for message in body
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(2)
    {
        match message.get("content") {
            Some(Value::String(text)) => text.hash(&mut hasher),
            Some(Value::Array(blocks)) => {
                for text in blocks
                    .iter()
                    .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                {
                    text.hash(&mut hasher);
                }
            }
            _ => {}
        }
    }
    format!("{:016x}", hasher.finish())
}

impl Gateway {
    /// Native Anthropic passthrough decision + execution. Returns `Some(served)`
    /// when an anthropic-messages request routed to an `anthropic-native` upstream
    /// and declared a server tool, and was forwarded verbatim; `None` when it is
    /// not a passthrough and the caller must fall through to the Canonical-IR
    /// pipeline. Never runs `normalize_inbound`.
    #[allow(clippy::too_many_arguments)] // decision, fallbacks and dispatch stay one path
    #[allow(clippy::too_many_lines)] // routing probe, fallbacks, lease and dispatch are one decision
    pub(super) fn try_anthropic_passthrough(
        &self,
        ctx: &RequestContext,
        agent: &LoadedAgent,
        router: &Router,
        raw_headers: &[(String, String)],
        body: &[u8],
        routing_model: Option<&str>,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<Option<(UpstreamModel, StreamOutcome)>, ErrorEnvelope> {
        // A body that is oversized or not JSON falls through, so the normal path
        // reports the size / parse error in its usual shape.
        if body.len() > MAX_INBOUND_BODY {
            return Ok(None);
        }
        let Ok(mut body_value) = serde_json::from_slice::<Value>(body) else {
            return Ok(None);
        };
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

        // Route a minimal request (model only) to learn which upstream would
        // serve it. Quota-first needs the canonical path's lease/settlement
        // state, so it deliberately falls through. A routing failure also falls
        // through so the normal path surfaces the real error.
        let route_model = routing_model.unwrap_or(&model);
        let mut mini = ChatRequest::new(route_model, Vec::new());
        mini.stream = stream;
        // Preserve the one feature the router needs for its hard tool gate
        // without parsing or rewriting Anthropic's native tool vocabulary.
        // The fixed marker never leaves this routing probe; the original array
        // (including server tools) remains structurally untouched in `body_value`.
        if body_value
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty())
        {
            mini.tools.push(ToolDef {
                name: "anthropic_native_routing_probe".to_owned(),
                description: None,
                parameters: json!({}),
            });
        }
        // Quota-first used to bail out here, which meant a server-tool turn was
        // simply unservable in that mode: the request fell through to the
        // Canonical path, where the adapter refuses the blocks. The payload's
        // shape was deciding which routing modes existed. It gets the same
        // treatment as any other request now — consumption-aware candidates, a
        // conversation-affine account, an in-flight lease and a settlement.
        let (quota_now_ms, session) =
            Self::quota_preamble(router, || native_quota_session_key(&body_value));
        let candidates = self.candidates(std::time::Instant::now(), quota_now_ms);
        let Ok(decision) = self.route_with_mode(router, &mini, &[], &candidates, &session) else {
            return Ok(None);
        };
        let Some(upstream) = self.upstreams.get(decision.chosen.upstream.as_str()) else {
            return Ok(None);
        };
        if upstream.dialect != ApiDialect::AnthropicNative {
            return Ok(None);
        }
        // The escape hatch is for what the IR cannot carry: a server tool has
        // no `ToolDef` representation (there is no `type` to say "the upstream
        // runs this"). A request without one round-trips through the IR and the
        // Anthropic component instead, where thinking, forced tool choice and
        // server-tool history all survive.
        if !anthropic_request_declares_server_tool(&body_value) {
            return Ok(None);
        }
        let vision_state = candidates
            .iter()
            .find(|candidate| candidate.target == decision.chosen)
            .map_or(CapabilityState::Unknown, |candidate| {
                candidate.capability.vision_state()
            });
        if vision_state == CapabilityState::Unsupported {
            let replaced = replace_anthropic_images(&mut body_value);
            if replaced > 0 {
                eprintln!(
                    "media fallback -> replaced {replaced} Anthropic image block(s) before native passthrough"
                );
            }
            let documents = replace_anthropic_documents(&mut body_value);
            if documents != DocumentFallbackStats::default() {
                eprintln!(
                    "document fallback -> extracted {} PDF document(s), omitted {} unsupported document(s) before native passthrough",
                    documents.extracted, documents.omitted
                );
            }
        }

        // Committed to the passthrough path.
        let headers = Self::curate_passthrough_headers(raw_headers)?;
        // Store the bounded Harness key when the gateway translated a Claude
        // family. Otherwise retain the caller-name privacy boundary.
        let configured = self.catalog.iter().any(|(target, _)| target.model == model);
        record.requested_model = canonical_requested_model(route_model, configured);
        record.stream = stream;

        // The native body is now just another attempt payload. Everything below
        // this line — routing record, quota snapshot, lease, budget, provider
        // admission, fallback, deadline, health and receipts — is the same
        // machinery every Canonical request gets, which is the whole point of
        // folding it in here.
        let last_upstream_error = RefCell::new(None);
        let result = self.execute_routed_attempt(
            ctx,
            agent,
            &AttemptPayload::AnthropicNative {
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

        // The pool is exhausted and every attempt was a retriable upstream
        // failure. The passthrough contract still holds for the answer the
        // client finally gets: relay the last upstream's own status and body,
        // not a token-station envelope built from them.
        if let Some(raw) = last_upstream_error
            .borrow_mut()
            .take()
            .filter(|_| result.is_err())
        {
            record.status = raw.status;
            record.error_code = Some(passthrough_error_code(raw.status));
            emit(Reply::BeginJson(JsonReply {
                status: raw.status,
                body: raw.body,
            }));
            return Ok(Some((raw.target, StreamOutcome::FailedBeforeOutput)));
        }
        result.map(Some)
    }

    /// Forwards the caller's Anthropic Messages body verbatim (only the routed
    /// model name is rewritten) to `base_url` + `/messages`, then relays the
    /// upstream response. Reuses the host's `authorize` egress gate and `send`
    /// path, so egress-to-base_url-only, credential isolation and redirect refusal
    /// are preserved; the client's own auth headers can never reach the upstream.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)] // authorize, send and terminal mapping form one state machine
    pub(super) fn native_attempt(
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
        const ANTHROPIC: &str = "anthropic-messages";
        // A later dial/auth/protocol failure must never accidentally relay an
        // earlier upstream's parked body as though it produced the terminal
        // attempt. A fresh attempt owns this slot from its first operation.
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

        // Verbatim body, except the caller's model is remapped to the routed one.
        let mut forwarded = body.clone();
        if let Some(object) = forwarded.as_object_mut() {
            object.insert("model".to_owned(), json!(target.model));
        }

        let mut descriptor = HttpRequestDescriptor::new(
            HttpMethod::Post,
            upstream.config.base_url.resolve(ProviderApi::Messages),
        );
        descriptor.headers = headers.clone();
        descriptor.body = Some(forwarded);
        descriptor.auth = upstream.config.auth.clone().map(|secret| {
            Auth::header("x-api-key", secret).expect("x-api-key is a credential header")
        });

        // The same exfiltration gate as build_provider_request: the URL must sit
        // inside base_url and the credential slot must match, checked before the
        // key is even resolved.
        if let Err(refusal) = upstream.config.authorize(&descriptor) {
            let error = ErrorEnvelope::new(ErrorCode::Internal, 500, refusal.to_string());
            record_conversion(
                record,
                ConversionStage::ProviderRequest,
                ANTHROPIC,
                ANTHROPIC,
                false,
                Some(error.code),
            );
            return Err(error);
        }
        record_conversion(
            record,
            ConversionStage::ProviderRequest,
            ANTHROPIC,
            ANTHROPIC,
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
                // A caller disconnect is a 499 and nobody's fault. Server
                // drain and deadline are host lifecycle failures instead;
                // dispatch owns their sanitized 503/504 rendering. Folding
                // all three into ClientCancelled made native South drains lie
                // as 499 while the translated path correctly returned 503.
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
                    ANTHROPIC,
                    ANTHROPIC,
                    false,
                    Some(error.code),
                );
                return Err(error);
            }
            Ok(response) => response,
        };
        // The attempt record carries the upstream's real status too. Without
        // it a served native request showed up in Stats with no engine and no
        // upstream status — which is how a request that plainly went out could
        // be filed as `(unrouted)`.
        *upstream_http_status = Some(response.status);
        ctx.capture_upstream_response_head(response.status, &response.headers);

        if let Err(error) = EgressPolicy::reject_redirect(response.status) {
            record_conversion(
                record,
                ConversionStage::ProviderResponse,
                ANTHROPIC,
                ANTHROPIC,
                false,
                Some(error.code),
            );
            return Err(error);
        }

        // L2 authoritative quota: harvest remaining/reset headers, never the body.
        let windows = crate::quota_headers::parse_quota_windows(&response.headers, unix_millis());
        if !windows.is_empty() {
            self.quota.lock().expect("quota lock").note_authoritative(
                target.upstream.as_str(),
                unix_millis(),
                windows,
            );
        }

        // Upstream error: return its status + body VERBATIM (Claude Code depends
        // on the original error body to self-heal), never token-station's wrapped
        // shape. Record the real status so the receipt stays truthful.
        if response.status >= 400 {
            let code = passthrough_error_code(response.status);
            let parts = response.into_parts()?;
            ctx.append_upstream_response_body(parts.body.as_bytes());
            record_conversion(
                record,
                ConversionStage::ProviderResponse,
                ANTHROPIC,
                ANTHROPIC,
                false,
                Some(code),
            );

            // A 5xx or a rate limit is the upstream's fault, not the request's:
            // the host already classifies these as retriable elsewhere, and the
            // translated path acts on that by trying the rest of the pool. This
            // path used to relay the first one and stop, so the same outage was
            // survivable on one route and fatal on the other. Park the response
            // and hand `dispatch` an error it can fall over on; the caller
            // relays whatever the last upstream said if the pool runs out.
            //
            // A 4xx is not retried, and the reasoning is the opposite: the same
            // request fails the same way everywhere, and Claude Code reads the
            // original body to repair itself. Relaying it at once is right.
            if code.is_retriable_elsewhere() {
                *last_upstream_error.borrow_mut() = Some(RawUpstreamError {
                    target: target.clone(),
                    status: parts.status,
                    body: parts.body,
                });
                return Err(ErrorEnvelope::new(
                    code,
                    parts.status,
                    "upstream refused the native passthrough",
                ));
            }

            record.status = parts.status;
            record.error_code = Some(code);
            emit(Reply::BeginJson(JsonReply {
                status: parts.status,
                body: parts.body,
            }));
            // FailedBeforeOutput preserves the already-set >=400 status through
            // settle (which only overwrites a <400 status) and charges health only
            // for the retriable server-error codes.
            return Ok(StreamOutcome::FailedBeforeOutput);
        }

        record_conversion(
            record,
            ConversionStage::ProviderResponse,
            ANTHROPIC,
            ANTHROPIC,
            true,
            None,
        );

        if stream {
            Self::relay_raw_sse(ctx, attempt_deadline, response, emit, record)
        } else {
            let parts = response.into_parts()?;
            ctx.append_upstream_response_body(parts.body.as_bytes());
            // Read the provider's own usage report off the body we are about
            // to relay. Nothing is rewritten: the client still gets the exact
            // bytes, the host just stops pretending the turn was free.
            if let Some(usage) = serde_json::from_str::<serde_json::Value>(&parts.body)
                .ok()
                .as_ref()
                .and_then(|body| body.get("usage"))
                .and_then(anthropic_wire_usage)
            {
                record.usage = Some(usage);
            }
            emit(Reply::BeginJson(JsonReply {
                status: parts.status,
                body: parts.body,
            }));
            Ok(StreamOutcome::Complete)
        }
    }

    /// Relays an upstream SSE stream to the client byte-for-byte (whole frames,
    /// unmodified) — a passthrough must never re-frame the Anthropic event stream.
    /// A clean EOF is complete only after Anthropic's terminal `message_stop`;
    /// an earlier EOF or socket error after the first byte is a post-200
    /// truncation (`FailedAfterPartial` → transient, does not eject).
    fn relay_raw_sse(
        ctx: &RequestContext,
        attempt_deadline: std::time::Instant,
        response: UpstreamResponse,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<StreamOutcome, ErrorEnvelope> {
        // The tap reads the frames going past; it never changes them. Its
        // result is written on every exit, including a truncated stream, since
        // `message_start`'s input tokens are real whether or not the turn
        // finished.
        let mut tap = AnthropicSseUsageTap::default();
        let outcome =
            Self::relay_raw_sse_frames(ctx, attempt_deadline, response, emit, record, &mut tap);
        if let Some(usage) = tap.into_usage() {
            record.usage = Some(usage);
        }
        outcome
    }

    fn raw_stream_cancellation(
        ctx: &RequestContext,
        committed: bool,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Option<Result<StreamOutcome, ErrorEnvelope>> {
        if !ctx.is_cancelled() {
            return None;
        }
        if let Some(error) = Self::lifecycle_cancellation(ctx) {
            if !committed {
                return Some(Err(error));
            }
            record.error_code = Some(error.code);
            record.status = error.http_status;
            return Some(Ok(StreamOutcome::FailedAfterPartial));
        }
        if !committed {
            Self::emit_cancelled(emit);
        }
        Some(Ok(StreamOutcome::ClientCancelled))
    }

    #[allow(clippy::too_many_lines)] // Raw relay owns one streaming lifecycle and its cleanup paths.
    fn relay_raw_sse_frames(
        ctx: &RequestContext,
        attempt_deadline: std::time::Instant,
        response: UpstreamResponse,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
        tap: &mut AnthropicSseUsageTap,
    ) -> Result<StreamOutcome, ErrorEnvelope> {
        let mut reader = response.into_reader();
        let mut buffer = [0u8; STREAM_READ];
        let mut committed = false;
        // Bytes of a character the last read cut in half. A read returns
        // whatever has arrived, not a whole number of characters, so a
        // multi-byte character routinely straddles two of them — and decoding
        // each read on its own turned every such character into U+FFFD. The
        // repository has been here before: `3fec191` added byte-level SSE
        // decoding for exactly this, and the translated path keeps it by
        // feeding bytes to `SseFrameDecoder`. This relay was written later and
        // did not.
        let mut pending: Vec<u8> = Vec::new();
        loop {
            if let Some(outcome) = Self::raw_stream_cancellation(ctx, committed, emit, record) {
                return outcome;
            }
            match reader.read(&mut buffer) {
                Ok(0) => {
                    // A stream that ends mid-character really is truncated;
                    // show the damage rather than drop the bytes.
                    if !pending.is_empty() {
                        let tail = String::from_utf8_lossy(&pending).into_owned();
                        tap.observe(&tail);
                        if !emit(Reply::Chunk(tail)) {
                            return Ok(StreamOutcome::ClientCancelled);
                        }
                    }
                    tap.finish();
                    if tap.is_complete() {
                        return Ok(StreamOutcome::Complete);
                    }
                    let error = ErrorEnvelope::new(
                        ErrorCode::TransportTruncated,
                        502,
                        "upstream Anthropic stream ended before message_stop",
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
                                "upstream stream contains invalid UTF-8",
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
                    if let Some(outcome) =
                        Self::raw_stream_cancellation(ctx, committed, emit, record)
                    {
                        return outcome;
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
                        "upstream connection broke while streaming",
                    ));
                }
            }
        }
    }

    /// The allowlist of caller headers forwarded verbatim on the passthrough: the
    /// non-credential metadata the Anthropic wire needs. Everything else is
    /// dropped. `SafeHeaders::try_new` additionally rejects any credential or
    /// host-owned header fail-closed, so the client's own auth can never ride
    /// upstream even if this list regressed.
    fn curate_passthrough_headers(
        raw_headers: &[(String, String)],
    ) -> Result<SafeHeaders, ErrorEnvelope> {
        const FORWARD: [&str; 4] = [
            "content-type",
            "accept",
            "anthropic-version",
            "anthropic-beta",
        ];
        let mut kept: Vec<(String, String)> = raw_headers
            .iter()
            .filter(|(name, _)| FORWARD.contains(&name.to_ascii_lowercase().as_str()))
            .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
            .collect();
        if !kept.iter().any(|(name, _)| name == "content-type") {
            kept.push(("content-type".to_owned(), "application/json".to_owned()));
        }
        if !kept.iter().any(|(name, _)| name == "anthropic-version") {
            kept.push(("anthropic-version".to_owned(), "2023-06-01".to_owned()));
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

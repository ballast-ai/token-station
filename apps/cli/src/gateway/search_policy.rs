//! Native-first hosted search. Browser fallback is limited to explicit capability refusals.
#[allow(clippy::wildcard_imports)]
use super::*;
use crate::search::SearchMode;
use sha2::{Digest, Sha256};
use token_station_metrics::SearchExecution;

#[derive(Default)]
pub(super) struct Cache {
    revision: u64,
    rejected: BTreeMap<String, Instant>,
}

impl Cache {
    fn contains(&mut self, revision: u64, key: &str) -> bool {
        if self.revision != revision {
            self.rejected.clear();
            self.revision = revision;
        }
        self.rejected
            .retain(|_, at| at.elapsed() < Duration::from_mins(10));
        self.rejected.contains_key(key)
    }

    fn reject(&mut self, revision: u64, key: String) {
        self.contains(revision, &key);
        if self.rejected.len() >= 256 {
            self.rejected.clear();
        }
        self.rejected.insert(key, Instant::now());
    }
}

/// Only structured error messages about unsupported tool kinds permit a retry.
fn rejects_search(reply: &JsonReply) -> bool {
    if !matches!(reply.status, 400 | 422) {
        return false;
    }
    let Ok(body) = serde_json::from_str::<Value>(&reply.body) else {
        return false;
    };
    let Some(message) = body["error"]["message"].as_str() else {
        return false;
    };
    let message = message.to_ascii_lowercase();
    let tool = message.contains("web_search") || message.contains("web search");
    let refusal = message.contains("not supported")
        || message.contains("unsupported")
        || message.contains("does not support")
        || message.contains("not available");
    let functions_only = [
        "only flattened client functions",
        "only client functions with name and object input_schema",
        "only supports function tools",
        "only function tools are supported",
    ]
    .iter()
    .any(|phrase| message.contains(phrase));
    // Parameter validation, account limits, and model access are not capability evidence.
    let parameter = [
        "tool_choice",
        "allowed_domains",
        "blocked_domains",
        "user_location",
        "max_uses",
        "max_tool_calls",
        "allowed_callers",
        "response_inclusion",
        "external_web_access",
        "quota",
        "balance",
        "credit",
        "permission",
        "api key",
        "authentication",
        "access denied",
        "model not found",
        "model is not available",
        "invalid schema",
        "argument",
        "invalid value",
    ]
    .iter()
    .any(|field| message.contains(field));
    !parameter && (functions_only || tool && refusal)
}

impl Gateway {
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(super) fn try_search_policy(
        &self,
        ctx: &RequestContext,
        agent: &LoadedAgent,
        router: &Router,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
        routing_model: Option<&str>,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<Option<(UpstreamModel, StreamOutcome)>, ErrorEnvelope> {
        let settings = self.search.settings();
        if !settings.enabled
            || body.len() > MAX_INBOUND_BODY
            || !matches!(
                agent.protocol.as_str(),
                "anthropic-messages" | "openai-responses"
            )
        {
            return Ok(None);
        }
        let Ok(original) = serde_json::from_slice::<Value>(body) else {
            return Ok(None);
        };
        let anthropic = agent.protocol == "anthropic-messages";
        if !original["tools"].as_array().is_some_and(|tools| {
            tools
                .iter()
                .any(|tool| super::local_search::hosted(tool, anthropic))
        }) {
            return Ok(None);
        }
        if settings.mode == SearchMode::Local {
            record.search_execution = Some(SearchExecution::Local);
            return self.try_local_search(
                ctx,
                agent,
                router,
                method,
                path,
                headers,
                body,
                routing_model,
                emit,
                record,
                None,
            );
        }
        let model = routing_model
            .or_else(|| original["model"].as_str())
            .unwrap_or("auto");
        let mut probe = ChatRequest::new(model, Vec::new());
        probe.stream = original["stream"].as_bool().unwrap_or(false);
        add_native_image_requirement(&mut probe, &original);
        probe.tools.push(ToolDef {
            name: "native_search_route".into(),
            description: None,
            parameters: json!({}),
            cache_control: None,
            strict: None,
        });
        let (now, session) = Self::quota_preamble(router, || {
            if anthropic {
                super::anthropic_native::native_quota_session_key(&original)
            } else {
                format!("responses-native:{model}")
            }
        });
        let candidates = self.candidates(Instant::now(), now);
        let mut decision = self
            .route_with_mode(router, &probe, &[], &candidates, &session)
            .map_err(|error| route_error(&error))?;
        let dialect = self
            .upstreams
            .get(decision.chosen.upstream.as_str())
            .ok_or_else(|| {
                ErrorEnvelope::new(ErrorCode::Internal, 500, "Search route is unavailable.")
            })?
            .search_dialect();
        let native = matches!(
            dialect,
            ApiDialect::ResponsesNative | ApiDialect::AnthropicNative
        );
        if !native {
            if settings.mode == SearchMode::Native {
                return Err(ErrorEnvelope::new(
                    ErrorCode::Capability,
                    400,
                    "The selected route cannot execute native web search. Select Auto or Local mode.",
                ));
            }
            record.search_execution = Some(SearchExecution::LocalProtocolFallback);
            // The normal local loop retains full-content routing for translated requests.
            return self.try_local_search(
                ctx,
                agent,
                router,
                method,
                path,
                headers,
                body,
                routing_model,
                emit,
                record,
                None,
            );
        }
        let jev_request = self
            .jev
            .is_enabled()
            .then(|| {
                super::jev_routing::native_request(
                    &original,
                    if anthropic {
                        ApiDialect::AnthropicNative
                    } else {
                        ApiDialect::ResponsesNative
                    },
                    model,
                )
            })
            .flatten();
        decision = self.route_native_with_jev(
            ctx,
            router,
            jev_request.as_ref(),
            &[],
            &candidates,
            decision,
            dialect,
            true,
        );
        // Keep native recovery within its wire protocol. Local fallback is pinned separately.
        decision.fallbacks.retain(|target| {
            self.upstreams
                .get(target.upstream.as_str())
                .is_some_and(|upstream| upstream.search_dialect() == dialect)
        });
        let cache_key = |target: &UpstreamModel| {
            format!(
                "{:x}",
                Sha256::digest(
                    format!("{}|{}|{}", target, agent.protocol, original["tools"]).as_bytes()
                )
            )
        };
        let key = cache_key(&decision.chosen);
        let revision = self.search.revision();
        let cached = self
            .search_policy_cache
            .lock()
            .expect("search cache")
            .contains(revision, &key);
        if settings.mode == SearchMode::Auto && cached {
            decision.fallbacks.clear();
            record.search_execution = Some(SearchExecution::LocalCachedFallback);
            return self.try_local_search(
                ctx,
                agent,
                router,
                method,
                path,
                headers,
                body,
                routing_model,
                emit,
                record,
                Some(decision),
            );
        }
        let bridge = (anthropic && dialect == ApiDialect::ResponsesNative)
            || (!anthropic && dialect == ApiDialect::AnthropicNative);
        let (mut forwarded, name) = if bridge {
            let translated = if anthropic {
                super::web_search::to_responses(&original)
            } else {
                super::reverse_search::to_anthropic(&original).map(|body| (body, String::new()))
            };
            match translated {
                Ok(value) => value,
                Err(error)
                    if settings.mode == SearchMode::Auto && error.code == ErrorCode::Capability =>
                {
                    decision.fallbacks.clear();
                    record.search_execution = Some(SearchExecution::LocalProtocolFallback);
                    return self.try_local_search(
                        ctx,
                        agent,
                        router,
                        method,
                        path,
                        headers,
                        body,
                        routing_model,
                        emit,
                        record,
                        Some(decision),
                    );
                }
                Err(error) => return Err(error),
            }
        } else {
            (original.clone(), String::new())
        };
        forwarded["model"] = json!(model);
        let stream = forwarded["stream"].as_bool().unwrap_or(false);
        let safe_headers = if dialect == ApiDialect::ResponsesNative {
            Self::curate_responses_headers(headers)?
        } else {
            Self::curate_passthrough_headers(headers)?
        };
        record.requested_model = canonical_requested_model(
            model,
            self.catalog.iter().any(|(target, _)| target.model == model),
        );
        record.stream = probe.stream;
        record.search_execution = Some(SearchExecution::Native);
        let raw_error = RefCell::new(None);
        let payload = if dialect == ApiDialect::ResponsesNative {
            AttemptPayload::ResponsesNative {
                body: &forwarded,
                headers: &safe_headers,
                stream,
                last_upstream_error: &raw_error,
            }
        } else {
            AttemptPayload::AnthropicNative {
                body: &forwarded,
                headers: &safe_headers,
                stream,
                last_upstream_error: &raw_error,
            }
        };
        let mut bridge_stream = (bridge && probe.stream).then(|| {
            super::search_stream::SearchStream::new(
                anthropic,
                &original,
                &name,
                forwarded["max_tool_calls"].as_u64(),
            )
        });
        let mut bridge_error = None;
        let mut parked = None;
        let mut output_started = false;
        ctx.begin_host_loop_accounting();
        let result = self.execute_routed_attempt(
            ctx,
            agent,
            &payload,
            &json!({}),
            &decision,
            &candidates,
            now,
            &session,
            &mut |reply| match reply {
                Reply::BeginJson(json) if !output_started && (json.status >= 400 || bridge) => {
                    parked = Some(json);
                    true
                }
                reply => {
                    output_started = true;
                    if let Some(bridge) = &mut bridge_stream {
                        if let Reply::Chunk(chunk) = reply {
                            match bridge.push(&chunk, emit) {
                                Ok(accepted) => accepted,
                                Err(error) => {
                                    bridge_error = Some(error);
                                    false
                                }
                            }
                        } else {
                            true
                        }
                    } else {
                        emit(reply)
                    }
                }
            },
            record,
        );
        // No retry after successful output or a transport/cancellation failure.
        if !output_started
            && !ctx.is_cancelled()
            && settings.mode == SearchMode::Auto
            && result
                .as_ref()
                .is_ok_and(|(_, outcome)| *outcome == StreamOutcome::FailedBeforeOutput)
            && parked.as_ref().is_some_and(rejects_search)
        {
            if let Ok((target, _)) = &result {
                decision.chosen = target.clone();
            }
            decision.fallbacks.clear();
            self.search_policy_cache
                .lock()
                .expect("search cache")
                .reject(revision, cache_key(&decision.chosen));
            record.search_execution = Some(SearchExecution::LocalRejectionFallback);
            record.status = 0;
            record.error_code = None;
            return self.try_local_search(
                ctx,
                agent,
                router,
                method,
                path,
                headers,
                body,
                routing_model,
                emit,
                record,
                Some(decision),
            );
        }
        if let Some(raw) = raw_error
            .borrow_mut()
            .take()
            .filter(|_| result.is_err() && !output_started)
        {
            record.status = raw.status;
            record.error_code = Some(match raw.status {
                401 | 403 => ErrorCode::Auth,
                429 => ErrorCode::RateLimit,
                status if status >= 500 => ErrorCode::UpstreamUnavailable,
                _ => ErrorCode::InvalidRequest,
            });
            let accepted = emit(Reply::BeginJson(JsonReply {
                status: raw.status,
                body: raw.body,
            }));
            return Ok(Some((
                raw.target,
                if accepted {
                    StreamOutcome::FailedBeforeOutput
                } else {
                    StreamOutcome::ClientCancelled
                },
            )));
        }
        let (target, outcome) = result?;
        if let Some(bridge) = &mut bridge_stream
            && output_started
            && parked.is_none()
        {
            if bridge_error.is_none() && outcome != StreamOutcome::ClientCancelled {
                bridge_error = bridge.finish().err();
            }
            if let Some(error) = bridge_error {
                if !bridge.started() {
                    emit(Reply::BeginStream);
                }
                super::search_stream::emit_failure(anthropic, &error, emit);
                record.status = error.http_status;
                record.error_code = Some(error.code);
                return Ok(Some((target, StreamOutcome::FailedAfterPartial)));
            }
            return Ok(Some((target, outcome)));
        }

        if let Some(reply) = parked {
            let accepted = if bridge && reply.status < 400 {
                let document = serde_json::from_str(&reply.body).map_err(|_| {
                    ErrorEnvelope::new(
                        ErrorCode::ProviderProtocolError,
                        502,
                        "Native search returned invalid JSON.",
                    )
                })?;
                if anthropic {
                    let answer = super::web_search::from_responses(
                        &document,
                        &name,
                        original["model"].as_str().unwrap_or(model),
                        forwarded["max_tool_calls"].as_u64(),
                        &original,
                    )?;
                    super::web_search::emit_message(&answer, probe.stream, emit)
                } else {
                    let answer = super::reverse_search::from_anthropic(&document, &original)?;
                    super::local_search::emit_responses(&answer, probe.stream, emit)
                }
            } else {
                emit(Reply::BeginJson(reply))
            };
            if !accepted {
                return Ok(Some((target, StreamOutcome::ClientCancelled)));
            }
        }
        Ok(Some((target, outcome)))
    }
}

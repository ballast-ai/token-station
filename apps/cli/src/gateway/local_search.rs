//! Host-owned search for function-capable models. The native path remains the default.
#[allow(clippy::wildcard_imports)]
use super::*;

const INTERNAL: &str = "token_station_browser_search";
const MAX_SEARCHES: u64 = 3;

fn invalid(message: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::Capability, 400, message)
}
pub(super) fn hosted(tool: &Value, anthropic: bool) -> bool {
    tool["type"].as_str().is_some_and(|kind| {
        if anthropic {
            kind.starts_with("web_search_")
        } else {
            matches!(kind, "web_search" | "web_search_preview")
        }
    })
}

fn search_action(args: &Value) -> Result<Value, String> {
    let fields = args
        .as_object()
        .ok_or("Search arguments must be an object.")?;
    if fields
        .keys()
        .any(|key| !matches!(key.as_str(), "query" | "url" | "pattern"))
    {
        return Err("Search arguments contain unsupported fields.".into());
    }
    if let Some(query) = args.get("query") {
        if !query.is_string() || fields.len() != 1 {
            return Err("Search arguments require either a query or a URL.".into());
        }
        return Ok(json!({"type":"search","query":query}));
    }
    let url = args["url"]
        .as_str()
        .ok_or("Page arguments require a URL string.")?;
    if let Some(pattern) = args.get("pattern") {
        let pattern = pattern
            .as_str()
            .filter(|p| !p.is_empty() && p.chars().count() <= 200)
            .ok_or("Page pattern arguments require 1 to 200 characters.")?;
        Ok(json!({"type":"find_in_page","url":url,"pattern":pattern}))
    } else {
        Ok(json!({"type":"open_page","url":url}))
    }
}

fn domain_filter(
    search: &Value,
    anthropic: bool,
) -> Result<crate::search::DomainFilter, ErrorEnvelope> {
    let present = |field| search.get(field).is_some_and(|value| !value.is_null());
    if (anthropic && present("filters"))
        || (!anthropic && (present("allowed_domains") || present("blocked_domains")))
    {
        return Err(invalid(
            "Domain filter fields do not match the request protocol.",
        ));
    }
    let filters = &search["filters"];
    if !anthropic
        && !filters.is_null()
        && !filters
            .as_object()
            .is_some_and(|fields| fields.keys().all(|key| key == "allowed_domains"))
    {
        return Err(invalid(
            "Browser search supports only filters.allowed_domains for Responses requests.",
        ));
    }
    let list = |value: &Value| -> Result<Vec<String>, ErrorEnvelope> {
        if value.is_null() {
            return Ok(Vec::new());
        }
        serde_json::from_value(value.clone())
            .map_err(|_| invalid("Domain filters must be arrays of domain names."))
    };
    let allowed = list(if anthropic {
        &search["allowed_domains"]
    } else {
        &filters["allowed_domains"]
    })?;
    let blocked = list(&search["blocked_domains"])?;
    crate::search::DomainFilter::new(&allowed, &blocked).map_err(|error| invalid(&error))
}

/// Returns None for normal requests. Validate before invoking any model or browser.
#[allow(clippy::too_many_lines)]
fn prepare(
    original: &Value,
    anthropic: bool,
) -> Result<Option<(Value, String, u64)>, ErrorEnvelope> {
    let Some(tools) = original["tools"].as_array() else {
        return Ok(None);
    };
    let declarations: Vec<_> = tools
        .iter()
        .filter(|tool| hosted(tool, anthropic))
        .collect();
    if declarations.is_empty() {
        return Ok(None);
    }
    if declarations.len() != 1 {
        return Err(invalid(
            "Browser search requires one hosted search declaration.",
        ));
    }
    let search = declarations[0];
    if tools.iter().any(|tool| tool["name"] == INTERNAL) {
        return Err(invalid(
            "A client tool conflicts with the internal browser search name.",
        ));
    }
    if anthropic
        && !matches!(
            search["type"].as_str(),
            Some("web_search_20250305" | "web_search_20260209" | "web_search_20260318")
        )
    {
        return Err(invalid(
            "This hosted search version is not supported by browser search.",
        ));
    }
    if search
        .get("allowed_callers")
        .is_some_and(|v| v != &json!(["direct"]))
        || (anthropic
            && search["type"] != "web_search_20250305"
            && search.get("allowed_callers").is_none())
    {
        return Err(invalid("Browser search supports direct tool calls only."));
    }
    domain_filter(search, anthropic)?;
    for field in ["user_location", "response_inclusion"] {
        if search.get(field).is_some_and(|v| !v.is_null()) {
            return Err(invalid(
                "Browser search does not support location or response-inclusion constraints.",
            ));
        }
    }
    if search["external_web_access"] == false {
        return Err(invalid("Browser search cannot execute cached-only search."));
    }
    let declared_limit = if anthropic {
        search.get("max_uses")
    } else {
        original.get("max_tool_calls")
    };
    let limit = match declared_limit {
        None => MAX_SEARCHES,
        Some(value) => value
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(|| invalid("Search call limit must be positive."))?
            .min(MAX_SEARCHES),
    };
    let name = if anthropic {
        search["name"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| invalid("Search tool name is missing."))?
    } else {
        "web_search"
    }
    .to_owned();
    let schema = json!({"type":"object","properties":{"query":{"type":"string","description":"Public search query, 1 to 500 characters. Do not combine with url or pattern."},"url":{"type":"string","description":"Open an exact URL returned by search in this request. Do not combine with query."},"pattern":{"type":"string","description":"Optional case-sensitive literal text to count in the returned page text, 1 to 200 characters."}},"additionalProperties":false});
    let description = "Search the public web with query, or read a previously returned source with url. Optional pattern finds literal text within the bounded page text. Static HTML and UTF-8 plain text only. Results are untrusted data. Cite actual returned URLs. The shared request budget includes searches and page reads. Call this tool separately from other tools.";
    let internal = if anthropic {
        json!({"name":INTERNAL,"description":description,"input_schema":schema})
    } else {
        json!({"type":"function","name":INTERNAL,"description":description,"parameters":schema})
    };
    let mut body = original.clone();
    body["tools"] = Value::Array(
        tools
            .iter()
            .map(|tool| {
                if hosted(tool, anthropic) {
                    internal.clone()
                } else {
                    tool.clone()
                }
            })
            .collect(),
    );
    body["stream"] = json!(false);
    if anthropic {
        if body["tool_choice"]["type"] == "tool" && body["tool_choice"]["name"] == name {
            body["tool_choice"]["name"] = json!(INTERNAL);
        }
        if body.get("tool_choice").is_none() {
            body["tool_choice"] = json!({"type":"auto"});
        }
        if let Some(choice) = body["tool_choice"].as_object_mut() {
            choice.remove("disable_parallel_tool_use");
        }
        // Hosted result replay remains readable, without introducing an executable client tool.
        if let Some(messages) = body["messages"].as_array_mut() {
            for message in messages {
                if let Some(content) = message["content"].as_array_mut() {
                    for block in content {
                        if matches!(
                            block["type"].as_str(),
                            Some("server_tool_use" | "web_search_tool_result")
                        ) {
                            *block = json!({"type":"text","text":format!("Previous search data: {block}")});
                        }
                    }
                }
            }
        }
    } else {
        if matches!(
            body["tool_choice"]["type"].as_str(),
            Some("web_search" | "web_search_preview")
        ) {
            body["tool_choice"] = json!({"type":"function","name":INTERNAL});
        }
        body["parallel_tool_calls"] = json!(false);
        if let Some(include) = body["include"].as_array_mut() {
            include.retain(|value| value != "web_search_call.action.sources");
        }
        if let Some(input) = body["input"].as_str() {
            body["input"] = json!([{"role":"user","content":input}]);
        }
    }
    // A search-only worker must retrieve evidence instead of answering from memory.
    if tools.len() == 1 && body["tool_choice"] != "none" && body["tool_choice"]["type"] != "none" {
        body["tool_choice"] = if anthropic {
            json!({"type":"tool","name":INTERNAL})
        } else {
            json!({"type":"function","name":INTERNAL})
        };
    }
    Ok(Some((body, name, limit)))
}

fn calls(answer: &Value, anthropic: bool) -> Vec<Value> {
    answer[if anthropic { "content" } else { "output" }]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| {
            if anthropic {
                item["type"] == "tool_use"
            } else {
                matches!(
                    item["type"].as_str(),
                    Some(
                        "function_call"
                            | "custom_tool_call"
                            | "tool_search_call"
                            | "local_shell_call"
                    )
                )
            }
        })
        .cloned()
        .collect()
}

/// Preserve required protocol numbers as known subtotals. The extra marker
/// distinguishes partial accounting without inventing usage for missing rounds.
fn write_loop_usage(answer: &mut Value, anthropic: bool, record: &RequestRecord, searches: u64) {
    let usage = record.usage.unwrap_or_default();
    let observation = record.usage_observation.unwrap_or_default();
    let cache_read = observation
        .cache_read_tokens
        .or_else(|| (usage.cache_read_tokens > 0).then_some(usage.cache_read_tokens));
    let cache_write = observation
        .cache_write_tokens
        .or_else(|| (usage.cache_write_tokens > 0).then_some(usage.cache_write_tokens));
    let reasoning = observation
        .reasoning_tokens
        .or_else(|| (usage.reasoning_tokens > 0).then_some(usage.reasoning_tokens));
    let partial_details = (cache_read.is_some() && observation.cache_read_tokens.is_none())
        || (cache_write.is_some() && observation.cache_write_tokens.is_none())
        || (reasoning.is_some() && observation.reasoning_tokens.is_none());
    let input = if anthropic {
        usage
            .input_tokens
            .saturating_sub(usage.cache_read_tokens)
            .saturating_sub(usage.cache_write_tokens)
    } else {
        usage.input_tokens
    };
    let mut wire = json!({"input_tokens":input,"output_tokens":usage.output_tokens});
    if observation.incomplete
        || partial_details
        || observation.input_tokens.is_none()
        || observation.output_tokens.is_none()
    {
        wire["token_station_incomplete"] = json!(true);
    }
    if anthropic {
        if let Some(tokens) = cache_read {
            wire["cache_read_input_tokens"] = json!(tokens);
        }
        if let Some(tokens) = cache_write {
            wire["cache_creation_input_tokens"] = json!(tokens);
        }
        wire["server_tool_use"] = json!({"web_search_requests":searches});
    } else {
        wire["total_tokens"] = json!(usage.total());
        if let Some(tokens) = cache_read {
            wire["input_tokens_details"]["cached_tokens"] = json!(tokens);
        }
        if let Some(tokens) = cache_write {
            wire["input_tokens_details"]["cache_write_tokens"] = json!(tokens);
        }
        if let Some(tokens) = reasoning {
            wire["output_tokens_details"]["reasoning_tokens"] = json!(tokens);
        }
    }
    answer["usage"] = wire;
}

/// Alias after normalization, including names flattened from Responses namespaces.
fn alias_client_tools(request: &mut ChatRequest) -> std::collections::BTreeMap<String, String> {
    use sha2::{Digest, Sha256};
    let mut occupied: std::collections::BTreeSet<String> =
        request.tools.iter().map(|tool| tool.name.clone()).collect();
    let originals = occupied.clone();
    let mut forward = std::collections::BTreeMap::new();
    for name in originals {
        if name.len() <= 64
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            continue;
        }
        let digest = format!("{:x}", Sha256::digest(name.as_bytes()));
        let base = format!("ts_alias_{}", &digest[..40]);
        let mut alias = base.clone();
        let mut suffix = 0_u32;
        while occupied.contains(&alias) {
            suffix += 1;
            alias = format!("{base}_{suffix}");
        }
        occupied.insert(alias.clone());
        forward.insert(name, alias);
    }
    for tool in &mut request.tools {
        if let Some(alias) = forward.get(&tool.name) {
            tool.name.clone_from(alias);
        }
    }
    for message in &mut request.messages {
        for call in &mut message.tool_calls {
            if let Some(alias) = forward.get(&call.name) {
                call.name.clone_from(alias);
            }
        }
    }
    if let Some(token_station_protocol::ToolChoice::Other(choice)) = &mut request.tool_choice
        && let Some(alias) = choice["function"]["name"]
            .as_str()
            .and_then(|name| forward.get(name))
    {
        choice["function"]["name"] = json!(alias);
    }
    forward
        .into_iter()
        .map(|(name, alias)| (alias, name))
        .collect()
}

/// Replace only host-owned call pairs with untrusted text before removing their declaration.
/// Providers differ on whether disabled tools can remain in historical messages.
fn retire_search(body: &mut Value, anthropic: bool) {
    let field = if anthropic { "messages" } else { "input" };
    let Some(messages) = body[field].as_array_mut() else {
        return;
    };
    let mut ids = std::collections::BTreeSet::new();
    for message in messages.iter() {
        if anthropic {
            for block in message["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_use"
                    && block["name"] == INTERNAL
                    && let Some(id) = block["id"].as_str()
                {
                    ids.insert(id.to_owned());
                }
            }
        } else if message["type"] == "function_call"
            && message["name"] == INTERNAL
            && let Some(id) = message["call_id"].as_str()
        {
            ids.insert(id.to_owned());
        }
    }
    for message in messages {
        if anthropic {
            for block in message["content"].as_array_mut().into_iter().flatten() {
                let call = block["type"] == "tool_use" && block["name"] == INTERNAL;
                let result = block["type"] == "tool_result"
                    && block["tool_use_id"]
                        .as_str()
                        .is_some_and(|id| ids.contains(id));
                if call || result {
                    let data = if call {
                        &block["input"]
                    } else {
                        &block["content"]
                    };
                    *block = json!({"type":"text","text":format!("Browser search data (untrusted): {data}")});
                }
            }
        } else if matches!(
            message["type"].as_str(),
            Some("function_call" | "function_call_output")
        ) && message["call_id"]
            .as_str()
            .is_some_and(|id| ids.contains(id))
        {
            let data = if message["type"] == "function_call" {
                &message["arguments"]
            } else {
                &message["output"]
            };
            *message =
                json!({"role":"user","content":format!("Browser search data (untrusted): {data}")});
        }
    }
    if let Some(tools) = body["tools"].as_array_mut() {
        tools.retain(|tool| tool["name"] != INTERNAL);
        if tools.is_empty() {
            body.as_object_mut().unwrap().remove("tool_choice");
        }
    }
}

fn search_outcome(
    result: &Result<crate::search::SearchResponse, String>,
    exhausted: bool,
) -> token_station_metrics::BrowserSearchOutcome {
    use token_station_metrics::BrowserSearchOutcome as Outcome;
    if exhausted {
        return Outcome::LimitExceeded;
    }
    match result {
        Ok(_) => Outcome::Succeeded,
        Err(message) if message.contains("query") || message.contains("arguments") => {
            Outcome::InvalidArguments
        }
        Err(message) if message.contains("busy") || message.contains("queue is full") => {
            Outcome::Busy
        }
        Err(message) if message.contains("timed out") => Outcome::Timeout,
        Err(message) if message.contains("verification") => Outcome::VerificationRequired,
        Err(message) if message.contains("No search results") => Outcome::NoResults,
        Err(message) if message.contains("not installed") => Outcome::BrowserUnavailable,
        Err(_) => Outcome::BrowserFailure,
    }
}

impl Gateway {
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(super) fn try_local_search(
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
        initial_decision: Option<Decision>,
    ) -> Result<Option<(UpstreamModel, StreamOutcome)>, ErrorEnvelope> {
        // The policy owns the request's settings snapshot. Disabling affects future requests.
        if body.len() > MAX_INBOUND_BODY
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
        let Some((mut working, name, limit)) = prepare(&original, anthropic)? else {
            return Ok(None);
        };
        let filter = domain_filter(
            original["tools"]
                .as_array()
                .and_then(|tools| tools.iter().find(|tool| hosted(tool, anthropic)))
                .expect("prepare validated hosted search"),
            anthropic,
        )?;
        if router.config().local_only {
            return Err(invalid("Browser search is unavailable in local-only mode."));
        }
        let must_search = working["tool_choice"]["name"] == INTERNAL;
        let search_only = original["tools"]
            .as_array()
            .is_some_and(|tools| tools.len() == 1);
        let mut evidence = Vec::new();
        let mut known_sources = BTreeSet::new();
        let stream = original["stream"].as_bool().unwrap_or(false);
        let mut decision = initial_decision;
        let mut search_items = Vec::new();
        ctx.begin_host_loop_accounting();
        let mut count = 0_u64;
        let first_attempt = record.attempts;
        for round in 0..=MAX_SEARCHES {
            if ctx.is_cancelled() {
                return Err(ErrorEnvelope::new(
                    ErrorCode::Timeout,
                    504,
                    "Search request was cancelled.",
                ));
            }
            let (mut request, hints, inbound_tools) = Self::normalize_request(
                agent,
                method,
                path,
                headers,
                working.to_string().as_bytes(),
                record,
            )?;
            if let Some(model) = routing_model {
                model.clone_into(&mut request.model);
            }
            request.stream = false;
            // The host verifies mandatory search evidence below. Thinking providers can
            // reject both required and named choices, so let them select the sole tool.
            if search_only && must_search {
                request.tool_choice = Some(token_station_protocol::ToolChoice::Auto);
            }
            let mut named_choice = None;
            // Required is equivalent to a named choice when exactly one tool is available.
            if request.tools.len() == 1
                && matches!(&request.tool_choice, Some(token_station_protocol::ToolChoice::Other(choice))
                    if choice["type"] == "function" && choice["function"]["name"] == request.tools[0].name)
            {
                named_choice.clone_from(&request.tool_choice);
                request.tool_choice = Some(token_station_protocol::ToolChoice::Required);
            }
            if count >= limit && request.tools.is_empty() {
                request.tool_choice = None;
            }
            // A producer built against protocol 0.5.0 may still hold the key in
            // `extensions`. Remove it there so the typed field is the only copy.
            request.extensions.remove("parallel_tool_calls");
            request.parallel_tool_calls = (!request.tools.is_empty()).then_some(false);
            let (now, session) = Self::quota_preamble(router, || quota_session_key(&request));
            let candidates = self.candidates(Instant::now(), now);
            let selected = if let Some(selected) = &decision {
                selected.clone()
            } else {
                self.route_with_semantics(ctx, router, &request, &hints, &candidates, &session)
                    .map_err(|error| route_error(&error))?
            };
            let _tool_alias_scope = ctx.tool_alias_scope(alias_client_tools(&mut request));
            let mut reply = None;
            record.usage = None;
            let mut result = self.execute_routed_attempt(
                ctx,
                agent,
                &AttemptPayload::Canonical(&request),
                &inbound_tools,
                &selected,
                &candidates,
                now,
                &session,
                &mut |item| {
                    if let Reply::BeginJson(json) = item {
                        reply = Some(json);
                        true
                    } else {
                        false
                    }
                },
                record,
            );
            if round == 0
                && count == 0
                && record.attempts == first_attempt + 1
                && named_choice.is_some()
                && reply.is_none()
                && !ctx.is_cancelled()
                && result.as_ref().is_err_and(|error| {
                    error.code == ErrorCode::InvalidRequest && error.http_status == 400
                })
                && record.routing.as_ref().is_some_and(|route| {
                    route.upstream == selected.chosen.upstream.as_str()
                        && route.model == selected.chosen.model
                })
            {
                // Retry an equivalent wire choice once, before any browser work or client output.
                record.usage = None;
                request.tool_choice = named_choice;
                let mut retry_selected = selected.clone();
                retry_selected.fallbacks.clear();
                result = self.execute_routed_attempt(
                    ctx,
                    agent,
                    &AttemptPayload::Canonical(&request),
                    &inbound_tools,
                    &retry_selected,
                    &candidates,
                    now,
                    &session,
                    &mut |item| {
                        if let Reply::BeginJson(json) = item {
                            reply = Some(json);
                            true
                        } else {
                            false
                        }
                    },
                    record,
                );
            }
            ctx.apply_host_loop_accounting(record);
            let (target, outcome) = result?;
            if outcome != StreamOutcome::Complete {
                return Err(ErrorEnvelope::new(
                    ErrorCode::UpstreamUnavailable,
                    502,
                    "The model request failed during browser search.",
                ));
            }
            let reply = reply
                .ok_or_else(|| invalid("Browser search requires a complete model response."))?;
            if reply.status >= 400 {
                return Err(ErrorEnvelope::new(
                    ErrorCode::UpstreamUnavailable,
                    reply.status,
                    "The model rejected the browser search request.",
                ));
            }
            let mut answer: Value = serde_json::from_str(&reply.body)
                .map_err(|_| invalid("The model returned invalid JSON."))?;
            let tool_calls = calls(&answer, anthropic);
            let searches: Vec<_> = tool_calls
                .iter()
                .filter(|call| call["name"] == INTERNAL)
                .collect();

            if must_search && count == 0 && searches.is_empty() {
                return Err(ErrorEnvelope::new(
                    ErrorCode::UpstreamUnavailable,
                    502,
                    "The model did not execute the required browser search. No search evidence was returned.",
                ));
            }
            let mixed = searches.len() != tool_calls.len();
            if !searches.is_empty() && round == MAX_SEARCHES {
                return Err(invalid(
                    "The model ignored the disabled search tool. Retry the request.",
                ));
            }
            let mut results = Vec::new();
            for call in searches {
                let args = if anthropic {
                    Ok(call["input"].clone())
                } else {
                    serde_json::from_str(call["arguments"].as_str().unwrap_or(""))
                        .map_err(|_| "The model returned invalid search arguments.".to_owned())
                };
                let query = args
                    .as_ref()
                    .ok()
                    .and_then(|args| args["query"].as_str())
                    .unwrap_or("")
                    .to_owned();
                let action = args.and_then(|args| search_action(&args));
                let exhausted = count >= limit;
                let search_started = Instant::now();
                let found = if exhausted {
                    Err("The search limit was reached. Answer from available results.".to_owned())
                } else {
                    count += 1;
                    action.as_ref().map_err(Clone::clone).and_then(|action| {
                        if action["type"] == "search" {
                            self.search.search_filtered(
                                action["query"].as_str().unwrap(),
                                &filter,
                                &|| ctx.is_cancelled(),
                            )
                        } else {
                            self.search.read_page(
                                action["url"].as_str().unwrap(),
                                &known_sources,
                                &filter,
                                action["pattern"].as_str(),
                                &|| ctx.is_cancelled(),
                            )
                        }
                    })
                };
                if ctx.is_cancelled() {
                    return Err(ErrorEnvelope::new(
                        ErrorCode::Timeout,
                        504,
                        "Search request was cancelled.",
                    ));
                }
                record
                    .browser_searches
                    .push(token_station_metrics::BrowserSearchRecord {
                        ordinal: u32::try_from(record.browser_searches.len() + 1)
                            .unwrap_or(u32::MAX),
                        elapsed_ms: u64::try_from(search_started.elapsed().as_millis())
                            .unwrap_or(u64::MAX),
                        outcome: search_outcome(&found, exhausted),
                    });
                if let Ok(found) = &found {
                    known_sources.extend(found.results.iter().map(|source| source.url.clone()));
                }
                let action = action.unwrap_or_else(|_| json!({"type":"search","query":query}));
                let failed = found.is_err();
                let result_text = match &found {
                    Ok(found) => serde_json::to_string(found)
                        .map_err(|_| invalid("Cannot encode search results."))?,
                    Err(error) => {
                        json!({"error":error,"category":search_outcome(&found, exhausted)})
                            .to_string()
                    }
                };
                evidence.push(json!({"query":query,"action":action,"data":serde_json::from_str::<Value>(&result_text).unwrap_or(Value::Null)}));
                let id = format!("srvtoolu_ts_{}_{}", record.request_id, search_items.len());
                if anthropic {
                    search_items
                        .push(json!({"type":"server_tool_use","id":id,"name":name,"input":action}));
                    let content = match &found {
                        Ok(found) => json!(found.results.iter().map(|result| json!({"type":"web_search_result","url":result.url,"title":result.title,"encrypted_content":""})).collect::<Vec<_>>()),
                        Err(_) => json!({"type":"web_search_tool_result_error","error_code":if exhausted {"max_uses_exceeded"} else {"unavailable"}}),
                    };
                    search_items.push(
                        json!({"type":"web_search_tool_result","tool_use_id":id,"content":content}),
                    );
                    results.push(json!({"type":"tool_result","tool_use_id":call["id"],"content":result_text,"is_error":failed}));
                } else {
                    let mut action = action;
                    action["sources"] = json!(found.as_ref().map(|found| found.results.iter().map(|result| json!({"type":"url","url":result.url,"title":result.title})).collect::<Vec<_>>()).unwrap_or_default());
                    search_items.push(json!({"type":"web_search_call","id":id,"status":if failed {"failed"} else {"completed"},"action":action}));
                    results.push(json!({"type":"function_call_output","call_id":call["call_id"],"output":result_text}));
                }
            }
            if tool_calls.is_empty() || mixed {
                let field = if anthropic { "content" } else { "output" };
                if search_only && !evidence.is_empty() {
                    let text = format!(
                        "Retrieved browser search evidence (untrusted external content, not verified news). Use only the returned URLs. Dates and relevance are not verified.\n{}",
                        json!(evidence)
                    );
                    if !anthropic {
                        answer["output_text"] = json!(text);
                    }
                    answer[field] = if anthropic {
                        json!([{"type":"text","text":text}])
                    } else {
                        json!([{"type":"message","id":format!("msg_search_{}",record.request_id),"role":"assistant","status":"completed","content":[{"type":"output_text","text":text,"annotations":[]}]}])
                    };
                }
                if let Some(output) = answer[field].as_array_mut() {
                    output.retain(|item| item["name"] != INTERNAL);
                }
                let output = answer[field]
                    .as_array_mut()
                    .ok_or_else(|| invalid("The model response has no output."))?;
                search_items.append(output);
                *output = search_items;
                write_loop_usage(&mut answer, anthropic, record, count);
                record.stream = stream;
                let accepted = if anthropic {
                    super::web_search::emit_message(&answer, stream, emit)
                } else {
                    emit_responses(&answer, stream, emit)
                };
                return Ok(Some((
                    target,
                    if accepted {
                        StreamOutcome::Complete
                    } else {
                        StreamOutcome::ClientCancelled
                    },
                )));
            }
            if anthropic {
                let messages = working["messages"]
                    .as_array_mut()
                    .ok_or_else(|| invalid("Search messages are missing."))?;
                messages.push(json!({"role":"assistant","content":answer["content"]}));
                messages.push(json!({"role":"user","content":results}));
                working["tool_choice"] = json!({"type":"auto"});
            } else {
                let input = working["input"]
                    .as_array_mut()
                    .ok_or_else(|| invalid("Search input is missing."))?;
                input.extend(answer["output"].as_array().cloned().unwrap_or_default());
                input.extend(results);
                working["tool_choice"] = json!("auto");
            }
            if count >= limit {
                retire_search(&mut working, anthropic);
            }
            let mut pinned = selected;
            pinned.chosen = target;
            pinned.fallbacks.clear();
            decision = Some(pinned);
        }
        Err(invalid("The browser search round limit was reached."))
    }
}

#[allow(clippy::too_many_lines)] // Keep the ordered Responses event sequence together.
pub(super) fn emit_responses(
    answer: &Value,
    stream: bool,
    emit: &mut dyn FnMut(Reply) -> bool,
) -> bool {
    if !stream {
        return emit(Reply::BeginJson(JsonReply {
            status: 200,
            body: answer.to_string(),
        }));
    }
    if !emit(Reply::BeginStream) {
        return false;
    }
    let mut sequence = 0_u64;
    let mut event = |kind: &str, mut value: Value| {
        value["type"] = json!(kind);
        value["sequence_number"] = json!(sequence);
        sequence += 1;
        emit(Reply::Chunk(format!("event: {kind}\ndata: {value}\n\n")))
    };
    let mut started = answer.clone();
    started["output"] = json!([]);
    started["status"] = json!("in_progress");
    started["usage"] = Value::Null;
    if !event("response.created", json!({"response":started})) {
        return false;
    }
    for (index, item) in answer["output"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let mut pending = item.clone();
        if item["type"] == "message" {
            pending["content"] = json!([]);
        }
        if item["type"] == "function_call" {
            pending["arguments"] = json!("");
        }
        if item["type"] == "custom_tool_call" {
            pending["input"] = json!("");
        }
        pending["status"] = json!("in_progress");
        if !event(
            "response.output_item.added",
            json!({"output_index":index,"item":pending}),
        ) {
            return false;
        }
        for (content_index, content) in item["content"].as_array().into_iter().flatten().enumerate()
        {
            let mut empty = content.clone();
            if content["type"] == "output_text" {
                empty["text"] = json!("");
            }
            if !event(
                "response.content_part.added",
                json!({"item_id":item["id"],"output_index":index,"content_index":content_index,"part":empty}),
            ) {
                return false;
            }
            if content["type"] == "output_text" {
                if !event(
                    "response.output_text.delta",
                    json!({"item_id":item["id"],"output_index":index,"content_index":content_index,"delta":content["text"]}),
                ) {
                    return false;
                }
                if !event(
                    "response.output_text.done",
                    json!({"item_id":item["id"],"output_index":index,"content_index":content_index,"text":content["text"]}),
                ) {
                    return false;
                }
            }
            if !event(
                "response.content_part.done",
                json!({"item_id":item["id"],"output_index":index,"content_index":content_index,"part":content}),
            ) {
                return false;
            }
        }
        if item["type"] == "web_search_call"
            && item["status"] == "completed"
            && !event(
                "response.web_search_call.completed",
                json!({"item_id":item["id"],"output_index":index}),
            )
        {
            return false;
        }
        if item["type"] == "function_call"
            && !event(
                "response.function_call_arguments.delta",
                json!({"item_id":item["id"],"output_index":index,"delta":item["arguments"]}),
            )
        {
            return false;
        }
        if item["type"] == "function_call"
            && !event(
                "response.function_call_arguments.done",
                json!({"item_id":item["id"],"output_index":index,"arguments":item["arguments"]}),
            )
        {
            return false;
        }
        if item["type"] == "custom_tool_call" {
            if !event(
                "response.custom_tool_call_input.delta",
                json!({"item_id":item["id"],"output_index":index,"delta":item["input"]}),
            ) {
                return false;
            }
            if !event(
                "response.custom_tool_call_input.done",
                json!({"item_id":item["id"],"output_index":index,"input":item["input"]}),
            ) {
                return false;
            }
        }
        if !event(
            "response.output_item.done",
            json!({"output_index":index,"item":item}),
        ) {
            return false;
        }
    }
    let terminal = if answer["status"] == "incomplete" {
        "response.incomplete"
    } else {
        "response.completed"
    };
    event(terminal, json!({"response":answer}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_actions_require_one_operation_and_a_literal_pattern() {
        assert_eq!(
            search_action(&json!({"query":"Rust"})).unwrap()["type"],
            "search"
        );
        assert_eq!(
            search_action(&json!({"url":"https://docs.rs/"})).unwrap()["type"],
            "open_page"
        );
        assert_eq!(
            search_action(&json!({"url":"https://docs.rs/","pattern":"Result"})).unwrap()["type"],
            "find_in_page"
        );
        for args in [
            json!({}),
            json!({"query":"x","url":"https://docs.rs/"}),
            json!({"query":"x","pattern":"y"}),
            json!({"url":1}),
            json!({"url":"https://docs.rs/","pattern":false}),
            json!({"query":"x","headers":{}}),
        ] {
            assert!(search_action(&args).is_err(), "{args}");
        }
    }
    #[test]
    fn aggregate_usage_keeps_protocol_cache_semantics_in_json_and_streams() {
        let mut record = RequestRecord::begin(0, "anthropic-messages");
        record.usage = Some(token_station_protocol::Usage {
            input_tokens: 30,
            output_tokens: 8,
            cache_read_tokens: 6,
            cache_write_tokens: 4,
            ..token_station_protocol::Usage::default()
        });
        record.usage_observation = Some(token_station_metrics::UsageObservation {
            input_tokens: Some(30),
            output_tokens: Some(8),
            cache_read_tokens: Some(6),
            cache_write_tokens: Some(4),
            ..token_station_metrics::UsageObservation::default()
        });
        for anthropic in [true, false] {
            let mut answer = json!({"id":"answer","type":"message","role":"assistant", "model":"model", "status":"completed","stop_reason":"end_turn", "content":[],"output":[]});
            write_loop_usage(&mut answer, anthropic, &record, 1);
            assert_eq!(
                answer["usage"]["input_tokens"],
                if anthropic { 20 } else { 30 }
            );
            assert!(answer["usage"].get("token_station_incomplete").is_none());
            record.usage_observation.as_mut().unwrap().incomplete = true;
            write_loop_usage(&mut answer, anthropic, &record, 1);
            assert_eq!(answer["usage"]["token_station_incomplete"], true);
            for stream in [false, true] {
                let mut messages = Vec::new();
                let mut emit = |reply| {
                    match reply {
                        Reply::BeginJson(reply) => {
                            messages.push(serde_json::from_str::<Value>(&reply.body).unwrap());
                        }
                        Reply::Chunk(chunk) => {
                            for line in chunk.lines().filter_map(|line| line.strip_prefix("data: "))
                            {
                                messages.push(serde_json::from_str::<Value>(line).unwrap());
                            }
                        }
                        Reply::BeginStream => {}
                    }
                    true
                };
                assert!(if anthropic {
                    super::super::web_search::emit_message(&answer, stream, &mut emit)
                } else {
                    emit_responses(&answer, stream, &mut emit)
                });
                let terminal = messages
                    .iter()
                    .rev()
                    .find(|message| !anthropic || !stream || message["type"] == "message_delta")
                    .unwrap();
                let usage = if !anthropic && stream {
                    &terminal["response"]["usage"]
                } else {
                    &terminal["usage"]
                };
                assert_eq!(usage["output_tokens"], 8);
                assert_eq!(usage["token_station_incomplete"], true);
                assert_eq!(usage["input_tokens"], if anthropic { 20 } else { 30 });
            }
            record.usage_observation.as_mut().unwrap().incomplete = false;
        }
    }

    #[test]
    fn partial_cache_observations_keep_known_subtotals_on_the_wire() {
        let mut record = RequestRecord::begin(0, "anthropic-messages");
        record.usage = Some(token_station_protocol::Usage {
            input_tokens: 30,
            output_tokens: 8,
            cache_read_tokens: 10,
            ..token_station_protocol::Usage::default()
        });
        record.usage_observation = Some(token_station_metrics::UsageObservation {
            input_tokens: Some(30),
            output_tokens: Some(8),
            ..token_station_metrics::UsageObservation::default()
        });
        for anthropic in [true, false] {
            let mut answer = json!({});
            write_loop_usage(&mut answer, anthropic, &record, 1);
            assert_eq!(answer["usage"]["token_station_incomplete"], true);
            if anthropic {
                assert_eq!(answer["usage"]["input_tokens"], 20);
                assert_eq!(answer["usage"]["cache_read_input_tokens"], 10);
            } else {
                assert_eq!(answer["usage"]["input_tokens"], 30);
                assert_eq!(answer["usage"]["input_tokens_details"]["cached_tokens"], 10);
            }
        }
    }

    #[test]
    fn maps_hosted_search_without_changing_client_tools() {
        let body = json!({"input":"Search", "tools":[{"type":"web_search"},{"type":"function","name":"edit","parameters":{}}],"tool_choice":{"type":"web_search"}});
        let (working, _, _) = prepare(&body, false).unwrap().unwrap();
        assert_eq!(working["tools"][0]["type"], "function");
        assert_eq!(working["tools"][1], body["tools"][1]);
        assert_eq!(working["tool_choice"]["name"], INTERNAL);
        assert_eq!(working["input"][0]["content"], "Search");
    }
    #[test]
    fn search_preparation_preserves_historical_search_records() {
        let history = json!({"type":"web_search_call","id":"ws_previous","status":"completed",
            "action":{"type":"search","query":"Rust","sources":[{"url":"https://www.rust-lang.org/"}]}});
        let body = json!({"input":[history,{"role":"user","content":"Continue"}],
            "tools":[{"type":"web_search"}]});
        let (working, _, _) = prepare(&body, false).unwrap().unwrap();
        assert_eq!(working["input"], body["input"]);
        let mut compact = body;
        compact["tools"] = json!([]);
        assert!(prepare(&compact, false).unwrap().is_none());
    }

    #[test]
    fn search_only_requires_evidence_but_preserves_explicit_none() {
        for anthropic in [true, false] {
            let tool = if anthropic {
                json!({"type":"web_search_20250305","name":"web_search"})
            } else {
                json!({"type":"web_search"})
            };
            let mut body = json!({"tools":[tool]});
            assert_eq!(
                prepare(&body, anthropic).unwrap().unwrap().0["tool_choice"]["name"],
                INTERNAL
            );
            body["tool_choice"] = if anthropic {
                json!({"type":"none"})
            } else {
                json!("none")
            };
            assert_eq!(
                prepare(&body, anthropic).unwrap().unwrap().0["tool_choice"],
                body["tool_choice"]
            );
        }
    }
    #[test]
    fn respects_the_responses_request_search_limit() {
        let body = json!({"tools":[{"type":"web_search"}],"max_tool_calls":1});
        assert_eq!(prepare(&body, false).unwrap().unwrap().2, 1);
    }

    #[test]
    fn does_not_intercept_an_ordinary_function_named_web_search() {
        assert!(
            prepare(
                &json!({"tools":[{"type":"function","name":"web_search"}]}),
                false
            )
            .unwrap()
            .is_none()
        );
    }
    #[test]
    fn accepts_supported_filters_and_rejects_malformed_declarations() {
        for (anthropic, constraint) in [
            (true, json!({"allowed_domains":["docs.rs"]})),
            (true, json!({"blocked_domains":["example.com"]})),
            (false, json!({"filters":{"allowed_domains":["docs.rs"]}})),
        ] {
            let mut tool = constraint;
            tool["type"] = json!(if anthropic {
                "web_search_20250305"
            } else {
                "web_search"
            });
            tool["name"] = json!("web_search");
            assert!(prepare(&json!({"tools":[tool]}), anthropic).is_ok());
        }
        for constraint in [
            json!({"filters":[]}),
            json!({"filters":{"allowed_domains":"example.com"}}),
            json!({"filters":{"allowed_domains":["example.com/path"]}}),
            json!({"allowed_domains":["example.com"]}),
        ] {
            let mut tool = constraint;
            tool["type"] = json!("web_search");
            assert!(prepare(&json!({"tools":[tool]}), false).is_err());
        }
    }

    #[test]
    fn rejects_constraints_before_any_network_request() {
        for constraint in [
            json!({"external_web_access":false}),
            json!({"filters":{"unknown":["example.com"]}}),
            json!({"user_location":{}}),
        ] {
            let mut tool = constraint;
            tool["type"] = json!("web_search");
            assert!(prepare(&json!({"tools":[tool]}), false).is_err());
        }
    }
    #[test]
    fn aliases_preserve_history_choice_arguments_and_avoid_collisions() {
        let long = "mcp__".to_owned() + &"x".repeat(80);
        let mut request: ChatRequest = serde_json::from_value(json!({"model":"test","tools":[{"name":long,"parameters":{}}],"tool_choice":{"type":"function","function":{"name":long}},"messages":[{"role":"assistant","tool_calls":[{"id":"call_a","name":long,"arguments":"unchanged"}]}]})).unwrap();
        let original = request.clone();
        let aliases = alias_client_tools(&mut request);
        let alias = request.tools[0].name.clone();
        assert!(alias.len() <= 64);
        assert_eq!(
            serde_json::to_value(&request.tool_choice).unwrap()["function"]["name"],
            alias
        );
        assert_eq!(alias_client_tools(&mut original.clone()), aliases);
        assert_eq!(request.messages[0].tool_calls[0].name, alias);
        assert_eq!(request.messages[0].tool_calls[0].arguments, "unchanged");
        let mut collision = original;
        collision.tools.push(token_station_protocol::ToolDef {
            name: alias.clone(),
            description: None,
            parameters: json!({}),
            cache_control: None,
            strict: None,
        });
        let mapped = alias_client_tools(&mut collision);
        assert_ne!(collision.tools[0].name, alias);
        assert_eq!(collision.tools[1].name, alias);
        assert_eq!(mapped.len(), 1);
    }

    #[test]
    fn custom_tool_stream_preserves_the_input_event_family() {
        let answer = json!({"status":"completed","output":[{"id":"custom_1","call_id":"call_1","type":"custom_tool_call","name":"edit","input":"patch"}]});
        let mut chunks = String::new();
        assert!(emit_responses(&answer, true, &mut |reply| {
            if let Reply::Chunk(chunk) = reply {
                chunks.push_str(&chunk);
            }
            true
        }));
        assert!(chunks.contains("response.custom_tool_call_input.delta"));
        assert!(chunks.contains("response.custom_tool_call_input.done"));
        assert!(!chunks.contains("response.function_call_arguments"));
    }

    #[test]
    fn failed_search_does_not_emit_a_success_event() {
        let answer = json!({"status":"completed","output":[{"id":"ws_1","type":"web_search_call","status":"failed"}]});
        let mut chunks = String::new();
        assert!(emit_responses(&answer, true, &mut |reply| {
            if let Reply::Chunk(chunk) = reply {
                chunks.push_str(&chunk);
            }
            true
        }));
        assert!(!chunks.contains("response.web_search_call.completed"));
        assert!(chunks.contains("response.completed"));
        assert!(chunks.contains("\"status\":\"failed\""));
    }

    #[test]
    fn retiring_search_preserves_client_tool_history_and_declarations() {
        for anthropic in [true, false] {
            let mut body = if anthropic {
                json!({"tools":[{"name":INTERNAL},{"name":"edit"}],"messages":[
                    {"role":"assistant","content":[{"type":"tool_use","id":"search","name":INTERNAL,"input":{"query":"rust"}},{"type":"tool_use","id":"edit","name":"edit","input":{}}]},
                    {"role":"user","content":[{"type":"tool_result","tool_use_id":"search","content":"source"},{"type":"tool_result","tool_use_id":"edit","content":"saved"}]}]})
            } else {
                json!({"tools":[{"name":INTERNAL},{"name":"edit"}],"input":[
                    {"type":"function_call","name":INTERNAL,"call_id":"search","arguments":"{}"},
                    {"type":"function_call_output","call_id":"search","output":"source"},
                    {"type":"function_call","name":"edit","call_id":"edit","arguments":"{}"},
                    {"type":"function_call_output","call_id":"edit","output":"saved"}]})
            };
            retire_search(&mut body, anthropic);
            assert_eq!(body["tools"], json!([{"name":"edit"}]));
            assert!(body.to_string().contains("source"));
            assert!(body.to_string().contains("saved"));
            let items = &body[if anthropic { "messages" } else { "input" }];
            if anthropic {
                assert_eq!(items[0]["content"][1]["type"], "tool_use");
                assert_eq!(items[1]["content"][1]["type"], "tool_result");
            } else {
                assert_eq!(items[2]["type"], "function_call");
                assert_eq!(items[3]["type"], "function_call_output");
            }
        }
    }

    #[test]
    fn completed_stream_contains_search_sources_and_text() {
        let answer = json!({"id":"resp_a","status":"completed","output":[{"type":"web_search_call","id":"ws_1","status":"completed","action":{"query":"rust","sources":[]}},{"type":"message","id":"msg_1","content":[{"type":"output_text","text":"Result","annotations":[]}]}]});
        let mut chunks = String::new();
        assert!(emit_responses(&answer, true, &mut |reply| {
            if let Reply::Chunk(chunk) = reply {
                chunks.push_str(&chunk);
            }
            true
        }));
        assert!(chunks.contains("response.output_text.delta"));
        assert!(chunks.contains("response.completed"));
        assert!(chunks.contains("web_search_call"));
    }
}

#[cfg(test)]
mod loop_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Records(Mutex<Vec<RequestRecord>>);
    impl Recorder for Records {
        fn record(&self, record: &RequestRecord) {
            self.0.lock().unwrap().push(record.clone());
        }
    }

    #[test]
    fn hosted_search_runs_two_model_rounds_and_preserves_client_tools_and_usage() {
        run_scenario("ordinary");
    }

    #[test]
    fn partially_reported_cache_usage_survives_search_protocols_and_persistence() {
        run_scenario("partial_cache");
    }

    #[test]
    fn missing_round_usage_remains_unknown_after_search_and_persistence() {
        run_scenario("missing_usage");
    }

    #[test]
    fn exhausted_search_budget_keeps_valid_tool_history() {
        run_scenario("limit");
    }

    #[test]
    fn mixed_search_and_client_calls_return_to_the_client() {
        run_scenario("mixed");
    }

    #[test]
    fn invalid_search_arguments_are_recoverable_tool_results() {
        run_scenario("invalid");
    }

    #[test]
    fn parallel_searches_respect_the_budget_without_aborting() {
        run_scenario("parallel");
    }

    #[test]
    fn three_search_rounds_finish_without_dangling_tool_calls() {
        run_scenario("three");
    }

    #[test]
    fn browser_failure_is_reported_without_aborting_the_answer() {
        run_scenario("browser_failure");
    }

    #[test]
    fn long_client_tool_names_round_trip_through_strict_providers() {
        run_scenario("long_name");
    }

    #[test]
    fn namespaced_client_tools_are_aliased_after_flattening_and_restored() {
        for provider in ["openai-compatible", "anthropic"] {
            run_provider_scenario("namespace", provider);
        }
    }

    #[test]
    fn mixed_custom_client_tools_remain_client_owned() {
        for provider in ["openai-compatible", "anthropic"] {
            run_provider_scenario("mixed_custom", provider);
        }
    }

    #[test]
    fn forced_search_omission_is_not_a_successful_answer() {
        run_scenario("omitted");
    }

    #[test]
    fn thinking_provider_accepts_search_only_auto_with_mandatory_evidence() {
        run_scenario("thinking_auto");
    }

    fn run_scenario(scenario: &'static str) {
        for provider in ["openai-compatible", "anthropic"] {
            run_provider_scenario(scenario, provider);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn run_provider_scenario(scenario: &'static str, provider: &'static str) {
        for anthropic in [true, false] {
            if matches!(scenario, "namespace" | "mixed_custom") && anthropic {
                continue;
            }
            let root = std::env::temp_dir().join(format!(
                "ts-search-loop-{}-{anthropic}-{scenario}-{provider}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let key = root.join("key");
            std::fs::write(&key, "test-key").unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
            let seen = Arc::new(Mutex::new(Vec::new()));
            let capture = Arc::clone(&seen);
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(0);
            let worker = std::thread::spawn(move || {
                // Start the request deadline after gateway and plugin initialization.
                if ready_rx.recv().is_err() {
                    return;
                }
                let deadline = Instant::now() + Duration::from_secs(5);
                for round in 0..if matches!(scenario, "mixed" | "mixed_custom" | "omitted") {
                    1
                } else if scenario == "three" {
                    4
                } else {
                    2
                } {
                    let mut connection = loop {
                        match listener.accept() {
                            Ok((connection, _)) => break connection,
                            Err(_) if Instant::now() < deadline => {
                                std::thread::sleep(Duration::from_millis(10));
                            }
                            Err(_) => return,
                        }
                    };
                    connection.set_nonblocking(false).unwrap();
                    connection
                        .set_read_timeout(Some(Duration::from_secs(15)))
                        .unwrap();
                    let mut bytes = Vec::new();
                    let mut byte = [0_u8; 1];
                    while !bytes.ends_with(b"\r\n\r\n") {
                        connection.read_exact(&mut byte).unwrap();
                        bytes.push(byte[0]);
                    }
                    let headers = String::from_utf8(bytes).unwrap();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(str::trim)
                                .map(str::to_owned)
                        })
                        .unwrap()
                        .parse()
                        .unwrap();
                    let mut body = vec![0; length];
                    connection.read_exact(&mut body).unwrap();
                    let captured: Value = serde_json::from_slice(&body).unwrap();
                    let client_name = captured["tools"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find_map(|tool| {
                            let name = if provider == "anthropic" {
                                &tool["name"]
                            } else {
                                &tool["function"]["name"]
                            };
                            name.as_str().filter(|name| *name != INTERNAL)
                        })
                        .unwrap_or("edit")
                        .to_owned();
                    let automatic = if provider == "anthropic" {
                        captured["tool_choice"]["type"] == "auto"
                    } else {
                        captured["tool_choice"] == "auto"
                    };
                    capture.lock().unwrap().push(captured);
                    if scenario == "thinking_auto" && round == 0 && !automatic {
                        let body = json!({"error":{"message":"Thinking mode does not support this tool_choice"}}).to_string();
                        write!(connection,"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
                        continue;
                    }
                    let mut message = if round == 0 || scenario == "three" && round < 3 {
                        json!({"role":"assistant","content":null,"tool_calls":[{"id":"call_search","type":"function","function":{"name":INTERNAL,"arguments":"{\"query\":\"Rust documentation\"}"}}]})
                    } else {
                        json!({"role":"assistant","content":"Source: https://www.rust-lang.org/","tool_calls":[{"id":"call_edit","type":"function","function":{"name":client_name,"arguments":"{}"}}]})
                    };
                    if round == 0 {
                        if scenario == "invalid" {
                            message["tool_calls"][0]["function"]["arguments"] = json!("{}");
                        }
                        if matches!(scenario, "mixed" | "mixed_custom") {
                            message["tool_calls"].as_array_mut().unwrap().push(json!({"id":"call_edit","type":"function","function":{"name":"edit","arguments":"{}"}}));
                        }
                        if scenario == "parallel" {
                            let mut extra = message["tool_calls"][0].clone();
                            extra["id"] = json!("call_extra");
                            message["tool_calls"].as_array_mut().unwrap().push(extra);
                        }
                    } else if matches!(scenario, "limit" | "parallel" | "thinking_auto")
                        || scenario == "three" && round == 3
                    {
                        message = json!({"role":"assistant","content":"Source: https://www.rust-lang.org/"});
                    }
                    if scenario == "omitted" {
                        message =
                            json!({"role":"assistant","content":"Invented news without search"});
                    }
                    let mut body = if provider == "anthropic" {
                        let mut content = Vec::new();
                        if let Some(text) = message["content"].as_str() {
                            content.push(json!({"type":"text","text":text}));
                        }
                        for call in message["tool_calls"].as_array().into_iter().flatten() {
                            content.push(json!({"type":"tool_use","id":call["id"],"name":call["function"]["name"],"input":serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap()).unwrap()}));
                        }
                        json!({"id":format!("msg_{round}"),"type":"message","role":"assistant","model":"test-model","content":content,"stop_reason":if message["tool_calls"].is_array() {"tool_use"} else {"end_turn"},"usage":{"input_tokens":10,"output_tokens":5}})
                    } else {
                        json!({"id":format!("chat_{round}"),"model":"test-model","choices":[{"index":0,"message":message,"finish_reason":if message["tool_calls"].is_array() {"tool_calls"} else {"stop"}}],"usage":{"prompt_tokens":10,"completion_tokens":5}})
                    };
                    if scenario == "partial_cache" && round == 0 {
                        if provider == "anthropic" {
                            body["usage"]["cache_read_input_tokens"] = json!(3);
                        } else {
                            body["usage"]["prompt_tokens_details"]["cached_tokens"] = json!(3);
                        }
                    }
                    if scenario == "missing_usage" && round == 0 {
                        body.as_object_mut().unwrap().remove("usage");
                    }
                    let body = body.to_string();
                    write!(connection,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
                }
            });
            let config: ClientConfig = serde_json::from_value(json!({
                "version":1,"server":{"listen":"127.0.0.1:0"},"data":{"dir":root},
                "plugins":{"dir":Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins-dist"),"allow_unsigned":true,"agents":[if anthropic {"agent-anthropic"} else {"agent-openai-responses"}],"providers":{"openai-compatible":"provider-openai-compatible-v2","anthropic":"provider-anthropic-v2"}},
                "upstreams":{"mock":{"provider":provider,"base_url":endpoint,"auth":{"slot":"provider_api_key","file":key},"models":[{"model":"test-model","tool":true,"tool_state":"verified","context_window":100_000}]}},
                "pricing":{"version":77,"models":{"mock/test-model":{"input_per_mtok":1_000_000,"output_per_mtok":1_000_000}}},
                "router":{"version":1,"pools":{"main":[{"upstream":"mock","model":"test-model"}]},"default_pool":"main"}
            })).unwrap();
            let recorder = Arc::new(Records::default());
            let gateway = Gateway::new(&config, recorder.clone()).unwrap();
            gateway
                .search
                .save(crate::search::SearchSettings {
                    enabled: true,
                    mode: crate::search::SearchMode::Local,
                    engine: crate::search::Engine::Bing,
                })
                .unwrap();
            *gateway.search.fixture.lock().unwrap() = Some(Ok(vec![crate::search::SearchResult {
                title: "Rust".into(),
                url: "https://www.rust-lang.org/".into(),
                snippet: "Official site".into(),
            }]));
            if scenario == "browser_failure" {
                *gateway.search.fixture.lock().unwrap() =
                    Some(Err("Browser search timed out.".into()));
            }
            let mut body = if anthropic {
                json!({"model":"auto","max_tokens":100,"messages":[{"role":"user","content":"Find Rust"}],"tools":[{"type":"web_search_20250305","name":"web_search"},{"name":"edit","input_schema":{"type":"object","properties":{}}}],"tool_choice":{"type":"tool","name":"web_search"}})
            } else {
                json!({"model":"auto","input":"Find Rust","tools":[{"type":"web_search"},{"type":"function","name":"edit","parameters":{"type":"object","properties":{}}}],"tool_choice":{"type":"web_search"}})
            };
            if matches!(scenario, "limit" | "parallel" | "three" | "thinking_auto") {
                if anthropic {
                    body["tools"][0]["max_uses"] = json!(if scenario == "three" { 3 } else { 1 });
                } else {
                    body["max_tool_calls"] = json!(if scenario == "three" { 3 } else { 1 });
                }
                body["tools"].as_array_mut().unwrap().truncate(1);
            }
            if scenario == "long_name" {
                body["tools"][1]["name"] =
                    json!("mcp__service__".to_owned() + &"long_".repeat(15) + "edit");
            }
            if scenario == "namespace" {
                body["tools"][1] = json!({"type":"namespace","name":"mcp_namespace","tools":[{"type":"function","name":"long_".repeat(11) + "edit","parameters":{"type":"object","properties":{}}}]});
            }
            if scenario == "mixed_custom" {
                body["tools"][1] = json!({"type":"custom","name":"edit","format":{"type":"text"}});
            }
            let mut answer = None;
            ready_tx.send(()).unwrap();
            gateway.chat(
                "POST",
                if anthropic {
                    "/v1/messages"
                } else {
                    "/v1/responses"
                },
                &[],
                body.to_string().as_bytes(),
                &mut |reply| {
                    if let Reply::BeginJson(json) = reply {
                        answer = Some(json);
                    }
                    true
                },
            );
            worker.join().unwrap();
            let answer = answer.unwrap();
            if scenario == "omitted" {
                assert_eq!(
                    seen.lock().unwrap().len(),
                    1,
                    "upstream must receive the request"
                );
                assert_eq!(answer.status, 502, "{}", answer.body);
                assert!(!answer.body.contains("Invented news"));
                std::fs::remove_dir_all(&root).unwrap();
                continue;
            }
            assert_eq!(answer.status, 200, "{}", answer.body);
            assert!(
                !answer.body.contains(INTERNAL),
                "Internal tool must stay inside the host"
            );
            if !matches!(scenario, "limit" | "parallel" | "three" | "thinking_auto") {
                assert!(
                    answer.body.contains("call_edit"),
                    "Client tool must remain client-owned"
                );
            }
            let output: Value = serde_json::from_str(&answer.body).unwrap();
            if matches!(scenario, "limit" | "parallel" | "three" | "thinking_auto") {
                assert!(
                    answer.body.contains("Official site"),
                    "Search-only responses must carry actual snippets"
                );
                if !anthropic {
                    let message = output["output"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|item| item["type"] == "message")
                        .unwrap();
                    assert_eq!(output["output_text"], message["content"][0]["text"]);
                }
                assert!(
                    !answer.body.contains("Source: https://"),
                    "Model summaries must not masquerade as retrieved evidence: {}",
                    answer.body
                );
            }
            if scenario == "mixed_custom" {
                assert!(
                    output["output"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|item| item["type"] == "custom_tool_call"
                            && item["call_id"] == "call_edit")
                );
            }
            let rounds = if matches!(scenario, "mixed" | "mixed_custom") {
                1
            } else if scenario == "three" {
                4
            } else {
                2
            };
            let billed_rounds = if scenario == "missing_usage" {
                rounds - 1
            } else {
                rounds
            };
            let canonical_input = billed_rounds * 10
                + usize::from(scenario == "partial_cache" && provider == "anthropic") * 3;
            let wire_input =
                canonical_input - usize::from(scenario == "partial_cache" && anthropic) * 3;
            assert_eq!(output["usage"]["input_tokens"], wire_input);
            assert_eq!(output["usage"]["output_tokens"], billed_rounds * 5);
            let requests = seen.lock().unwrap();
            assert_eq!(requests.len(), rounds);
            if matches!(scenario, "limit" | "parallel" | "three" | "thinking_auto") {
                assert_eq!(
                    requests[0]["tool_choice"],
                    if provider == "anthropic" {
                        json!({"type":"auto"})
                    } else {
                        json!("auto")
                    },
                    "The host enforces search evidence without forcing provider tool choice"
                );
            }
            if matches!(scenario, "long_name" | "namespace") {
                for request in requests.iter() {
                    for tool in request["tools"].as_array().into_iter().flatten() {
                        let name = if provider == "anthropic" {
                            &tool["name"]
                        } else {
                            &tool["function"]["name"]
                        };
                        assert!(
                            name.as_str().unwrap().len() <= 64,
                            "Strict providers reject long tool names"
                        );
                    }
                }
                assert!(
                    answer
                        .body
                        .contains(body["tools"][1]["name"].as_str().unwrap()),
                    "Client tool names must be restored"
                );
                if scenario == "namespace" {
                    assert!(
                        answer
                            .body
                            .contains(body["tools"][1]["tools"][0]["name"].as_str().unwrap())
                    );
                    assert!(answer.body.contains("namespace"));
                }
            }
            if rounds > 1 {
                let history = requests.last().unwrap()["messages"].to_string();
                assert!(
                    history.contains(if matches!(scenario, "invalid" | "browser_failure") {
                        "error"
                    } else {
                        "Official site"
                    })
                );
                assert_eq!(requests[0]["model"], requests[1]["model"]);
                if matches!(scenario, "limit" | "parallel" | "three" | "thinking_auto") {
                    let final_request = requests.last().unwrap();
                    assert!(
                        !final_request["messages"]
                            .to_string()
                            .contains("call_search"),
                        "Retired searches must not leave dangling tool calls"
                    );
                    assert!(
                        final_request["tools"].is_null() || final_request["tools"] == json!([])
                    );
                    assert!(final_request["tool_choice"].is_null());
                }
            }
            let receipts = recorder.0.lock().unwrap();
            assert_eq!(receipts.last().unwrap().attempts as usize, rounds);
            assert_eq!(
                usize::try_from(receipts.last().unwrap().usage.unwrap().input_tokens).unwrap(),
                canonical_input
            );
            if matches!(scenario, "ordinary" | "missing_usage" | "partial_cache") {
                let database = root.join("accounting-regression.sqlite");
                let store = crate::store::SqliteStore::open(&database).unwrap();
                store.record(receipts.last().unwrap());
                let restored = crate::store::recent_receipts(&database, 1).unwrap();
                let statistics = crate::stats::collect(&database, None, None).unwrap();
                if scenario == "ordinary" {
                    assert_eq!(restored[0].usage.unwrap().input_tokens, 20);
                    assert_eq!(restored[0].usage.unwrap().output_tokens, 10);
                    assert_eq!(restored[0].cost_micros, Some(30));
                    assert_eq!(restored[0].cost_kind, CostKind::Estimated);
                    assert_eq!(statistics.total.input_tokens, 20);
                    assert_eq!(statistics.total.output_tokens, 10);
                    assert_eq!(statistics.total.cost_micros, Some(30));
                    assert!(output["usage"].get("token_station_incomplete").is_none());
                } else if scenario == "partial_cache" {
                    assert_eq!(
                        restored[0].usage.unwrap().input_tokens,
                        u64::try_from(canonical_input).unwrap()
                    );
                    assert_eq!(restored[0].usage.unwrap().cache_read_tokens, 3);
                    assert_eq!(
                        statistics.total.input_tokens,
                        u64::try_from(canonical_input).unwrap()
                    );
                    assert_eq!(statistics.total.cache_read_tokens, 3);
                    assert_eq!(output["usage"]["token_station_incomplete"], true);
                    let cache = if anthropic {
                        &output["usage"]["cache_read_input_tokens"]
                    } else {
                        &output["usage"]["input_tokens_details"]["cached_tokens"]
                    };
                    assert_eq!(cache, 3);
                } else {
                    let observation = restored[0].usage_observation.unwrap();
                    assert!(observation.incomplete);
                    assert_eq!(observation.input_tokens, None);
                    assert_eq!(observation.output_tokens, None);
                    assert_eq!(restored[0].cost_micros, None);
                    assert_eq!(restored[0].cost_kind, CostKind::Unknown);
                    assert_eq!(statistics.total.unpriced_requests, 1);
                    assert_eq!(statistics.total.incomplete_usage_requests, 1);
                    assert_eq!(output["usage"]["token_station_incomplete"], true);
                }
            }
            let attempt_ordinals: Vec<_> = receipts
                .last()
                .unwrap()
                .attempt_records
                .iter()
                .map(|attempt| attempt.ordinal)
                .collect();
            assert_eq!(
                attempt_ordinals,
                (1..=u32::try_from(rounds).unwrap()).collect::<Vec<_>>()
            );
            let searches = &receipts.last().unwrap().browser_searches;
            assert_eq!(
                searches.len(),
                if scenario == "parallel" {
                    2
                } else if scenario == "three" {
                    3
                } else {
                    1
                }
            );
            assert_eq!(
                searches[0].outcome,
                if scenario == "invalid" {
                    token_station_metrics::BrowserSearchOutcome::InvalidArguments
                } else if scenario == "browser_failure" {
                    token_station_metrics::BrowserSearchOutcome::Timeout
                } else {
                    token_station_metrics::BrowserSearchOutcome::Succeeded
                }
            );
            if scenario == "parallel" {
                assert_eq!(
                    searches[1].outcome,
                    token_station_metrics::BrowserSearchOutcome::LimitExceeded
                );
            }
            drop(gateway);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

//! Cloud tier selection leaves policy, protocol, and generation payloads local.

#[allow(clippy::wildcard_imports)]
use super::*;
use crate::jev::Outcome;
use token_station_protocol::Role;
use token_station_router_core::DecidedBy;

impl Gateway {
    #[allow(clippy::too_many_arguments)] // The baseline and all policy inputs belong to this decision.
    pub(super) fn route_with_jev(
        &self,
        ctx: &RequestContext,
        router: &Router,
        request: &ChatRequest,
        hints: &[token_station_protocol::AgentHint],
        candidates: &[Candidate],
        baseline: Result<Decision, NoRoute>,
    ) -> Result<Decision, NoRoute> {
        if router.config().local_only {
            self.jev.bypass(Outcome::LocalOnly);
            return baseline;
        }
        if !router.accepts_classifier(request, hints)
            || !["tier_low", "tier_mid", "tier_high"]
                .iter()
                .all(|pool| router.config().pools.contains_key(*pool))
        {
            self.jev.bypass(Outcome::Overridden);
            return baseline;
        }
        let Some(suggestion) = self.jev.classify(request, ctx, &self.egress.policy) else {
            return baseline;
        };
        if !self.jev.is_current(&suggestion) {
            return baseline;
        }
        if let Ok(mut decision) = router.route_with_classifier_pool(
            request,
            hints,
            candidates,
            Some(suggestion.tier.pool()),
        ) && decision.decided_by == DecidedBy::Classifier
            && decision.pool == suggestion.tier.pool()
        {
            decision.features.estimated_input_tokens =
                attempt_machine::estimated_input_with_schemas(request, hints);
            retain_free_fallbacks(&mut decision, &self.free_upstreams);
            self.jev.finish(suggestion, true);
            Ok(decision)
        } else {
            self.jev.finish(suggestion, false);
            baseline
        }
    }

    /// Run only after native admission commits to its protocol. A declined
    /// passthrough does not classify before the canonical path classifies.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn route_native_with_jev(
        &self,
        ctx: &RequestContext,
        router: &Router,
        request: Option<&ChatRequest>,
        hints: &[token_station_protocol::AgentHint],
        candidates: &[Candidate],
        baseline: Decision,
        dialect: ApiDialect,
        search: bool,
    ) -> Decision {
        if !self.jev.is_enabled() {
            return baseline;
        }
        if router.config().local_only {
            self.jev.bypass(Outcome::LocalOnly);
            return baseline;
        }
        // Native probes and full projections can match different rules. A
        // priority decision from the original probe must never be displaced.
        if !matches!(
            baseline.decided_by,
            DecidedBy::Default | DecidedBy::Heuristic { .. }
        ) {
            self.jev.bypass(Outcome::Overridden);
            return baseline;
        }
        let Some(request) = request else {
            self.jev.bypass(Outcome::Unsupported);
            return baseline;
        };
        let compatible = candidates
            .iter()
            .filter(|candidate| {
                self.upstreams
                    .get(candidate.target.upstream.as_str())
                    .is_some_and(|upstream| {
                        (if search {
                            upstream.search_dialect()
                        } else {
                            upstream.dialect
                        }) == dialect
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        // Baseline is already usable. Classification can change only its tier.
        self.route_with_jev(ctx, router, request, hints, &compatible, Ok(baseline))
            .expect("a successful native baseline remains the fallback")
    }
}

/// Project complete plain-text native messages for routing. Tool declarations
/// remain capability and context requirements. Tool history, opaque continuation,
/// reasoning blocks, and multimodal content explicitly retain baseline routing.
/// The original generation body is never modified by this projection.
pub(super) fn native_request(
    body: &Value,
    dialect: ApiDialect,
    model: &str,
) -> Option<ChatRequest> {
    let mut request = ChatRequest::new(model, Vec::new());
    request.stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let (system_key, output_key) = match dialect {
        ApiDialect::AnthropicNative => ("system", "max_tokens"),
        ApiDialect::ResponsesNative => {
            if ["previous_response_id", "conversation"]
                .iter()
                .any(|key| body.get(*key).is_some_and(|value| !value.is_null()))
            {
                return None;
            }
            ("instructions", "max_output_tokens")
        }
        ApiDialect::Translated => return None,
    };
    if let Some(system) = body.get(system_key).filter(|value| !value.is_null()) {
        request
            .messages
            .push(Message::text(Role::System, text_content(system, dialect)?));
    }
    if let Some(tokens) = body.get(output_key) {
        request.sampling.max_output_tokens = Some(u32::try_from(tokens.as_u64()?).ok()?);
    }
    let input = body.get(if dialect == ApiDialect::AnthropicNative {
        "messages"
    } else {
        "input"
    })?;
    if dialect == ApiDialect::ResponsesNative && input.is_string() {
        request
            .messages
            .push(Message::text(Role::User, input.as_str()?));
    } else {
        for message in input.as_array()? {
            if message.get("type").is_some_and(|kind| kind != "message") {
                return None;
            }
            let role = match message.get("role")?.as_str()? {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                "system" | "developer" if dialect == ApiDialect::ResponsesNative => Role::System,
                _ => return None,
            };
            request.messages.push(Message::text(
                role,
                text_content(message.get("content")?, dialect)?,
            ));
        }
    }
    if let Some(tools) = body.get("tools").filter(|value| !value.is_null()) {
        for tool in tools.as_array()? {
            request.tools.push(ToolDef {
                name: tool
                    .get("name")
                    .or_else(|| tool.get("type"))?
                    .as_str()?
                    .to_owned(),
                description: None,
                // Count the original declaration in context without forwarding
                // any provider tool vocabulary to the cloud classifier.
                parameters: tool.clone(),
            });
        }
    }
    let format = if dialect == ApiDialect::ResponsesNative {
        body.get("text").and_then(|text| text.get("format"))
    } else {
        body.get("output_config")
            .and_then(|output| output.get("format"))
    };
    if let Some(format) = format.filter(|value| !value.is_null()) {
        request.response_format = Some(match format.get("type")?.as_str()? {
            "text" => ResponseFormat::Text,
            "json_object" => ResponseFormat::JsonObject,
            "json_schema" => ResponseFormat::JsonSchema {
                json_schema: format.clone(),
            },
            _ => return None,
        });
    }
    Some(request)
}

fn text_content(content: &Value, dialect: ApiDialect) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_owned());
    }
    let mut text = String::new();
    for part in content.as_array()? {
        let kind = part.get("type")?.as_str()?;
        if kind != "text"
            && !(dialect == ApiDialect::ResponsesNative
                && matches!(kind, "input_text" | "output_text"))
        {
            return None;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(part.get("text")?.as_str()?);
    }
    Some(text)
}

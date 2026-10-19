//! Native Google Code Assist translation. The host owns credentials and I/O.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use south_component_conformance::{
    reference_gemini::GeminiReferenceV1, ComponentResultV1, ProviderComponentV1, StreamParserV1,
};
use south_provider_api::{ComponentMetadataV1, PROVIDER_WORLD};
use std::collections::{BTreeSet, VecDeque};
use token_station_protocol::ResponseFormat;
use token_station_protocol::{
    Auth, ChatRequest, ChatResponse, Content, ContentPart, ErrorCode, ErrorEnvelope, FinishReason,
    HttpMethod, HttpRequestDescriptor, HttpResponseParts, ModelCapability, ProviderConfig,
    SafeHeaders, StreamEvent, Usage,
};
#[cfg(target_arch = "wasm32")]
mod shell;
mod stream;
pub struct CodeAssist;
const LIMIT: usize = 4 * 1024 * 1024;
fn capability(s: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::Capability, 400, s)
}
fn protocol(s: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::ProviderProtocolError, 502, s)
}
fn checked(c: &ProviderConfig) -> ComponentResultV1<bool> {
    let ag = match c.provider.as_str() {
        "antigravity-plan" => true,
        "gemini-cli-plan" => false,
        _ => return Err(capability("Unknown Google subscription dialect.")),
    };
    let want = if ag {
        "https://daily-cloudcode-pa.googleapis.com"
    } else {
        "https://cloudcode-pa.googleapis.com"
    };
    if c.base_url.as_str() != want
        || c.auth.as_ref().map(|s| s.as_str()) != Some("local_subscription_token")
    {
        return Err(capability(
            "Google subscriptions require a fixed endpoint and a bound credential slot.",
        ));
    }
    let project = c
        .extensions
        .get("google_project")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if project.is_empty() || project.len() > 256 || project.chars().any(char::is_control) {
        return Err(capability("A verified Code Assist project is required."));
    }
    Ok(ag)
}
fn signed_id(id: Option<&str>, sig: &str, model: &str) -> ComponentResultV1<String> {
    if id.is_some_and(str::is_empty) || model.is_empty() {
        return Err(protocol("Code Assist returned an invalid tool identity."));
    }
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|_| protocol("Cannot create a unique Google tool identity."))?;
    let nonce = nonce
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let raw = json!([id, sig, model, nonce]).to_string();
    if raw.len() > 64 * 1024 {
        return Err(protocol("Google tool identity exceeds its size limit."));
    }
    Ok(format!("ca2_{}", URL_SAFE_NO_PAD.encode(raw)))
}
// Client reasoning presets and returned modelVersion labels can name the same
// underlying model differently. Retain version, family, Lite, and image identity.
fn tool_model_identity(model: &str) -> &str {
    for suffix in [
        "-extra-low",
        "-thinking",
        "-medium",
        "-high",
        "-low",
        "-tiered",
        "-agent",
        "-n",
        "-h",
        "-l",
    ] {
        if let Some(base) = model.strip_suffix(suffix) {
            if base.starts_with("gemini-") && (base.ends_with("-flash") || base.ends_with("-pro")) {
                return base;
            }
            if suffix == "-thinking" && base.starts_with("claude-") {
                return base;
            }
        }
    }
    model
}
fn decode_id(id: &str, model: &str) -> ComponentResultV1<(Option<String>, String)> {
    let (encoded, legacy) = if let Some(encoded) = id.strip_prefix("ca2_") {
        (encoded, false)
    } else if let Some(encoded) = id.strip_prefix("ca1_") {
        (encoded, true)
    } else {
        return Err(capability(
            "Foreign tool history cannot preserve Google thought signatures.",
        ));
    };
    if encoded.len() > 88 * 1024 {
        return Err(capability("Google tool identity exceeds its size limit."));
    }
    let raw = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| capability("Invalid Google tool identity."))?;
    if raw.len() > 64 * 1024 {
        return Err(capability("Google tool identity exceeds its size limit."));
    }
    let v: Value =
        serde_json::from_slice(&raw).map_err(|_| capability("Invalid Google tool identity."))?;
    if v.as_array()
        .is_none_or(|parts| parts.len() != if legacy { 3 } else { 4 })
        || (!legacy
            && v[3].as_str().is_none_or(|nonce| {
                nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
            }))
    {
        return Err(capability("Invalid Google tool identity."));
    }
    if v[2].as_str().map(tool_model_identity) != Some(tool_model_identity(model)) {
        return Err(capability("Google tool history belongs to another model."));
    }
    Ok((
        if !legacy && v[0].is_null() {
            None
        } else {
            Some(
                v[0].as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| capability("Invalid Google tool identity."))?
                    .into(),
            )
        },
        v[1].as_str()
            .ok_or_else(|| capability("Invalid Google tool identity."))?
            .into(),
    ))
}
fn validate_thought_part(part: &Value) -> ComponentResultV1<()> {
    if part.as_object().is_none_or(|fields| {
        fields
            .keys()
            .any(|key| !["text", "thought", "thoughtSignature"].contains(&key.as_str()))
    }) || part["thought"] != true
        || part.get("text").is_some_and(|text| !text.is_string())
        || part
            .get("thoughtSignature")
            .is_some_and(|signature| !signature.is_string())
        || (part.get("text").is_none() && part.get("thoughtSignature").is_none())
    {
        return Err(protocol("Code Assist returned an invalid thinking part."));
    }
    Ok(())
}
fn validate_output_part(part: &Value) -> ComponentResultV1<()> {
    let fields = part
        .as_object()
        .ok_or_else(|| capability("Code Assist returned an unsupported content part."))?;
    if fields
        .keys()
        .any(|key| !["text", "thought", "thoughtSignature", "functionCall"].contains(&key.as_str()))
    {
        return Err(capability(
            "Code Assist returned an unsupported content part.",
        ));
    }
    if fields
        .get("thought")
        .is_some_and(|thought| !thought.is_boolean())
        || fields
            .get("thoughtSignature")
            .is_some_and(|signature| !signature.is_string())
    {
        return Err(protocol("Code Assist returned invalid content metadata."));
    }
    if let Some(call) = fields.get("functionCall") {
        if fields.contains_key("text") {
            return Err(capability(
                "Code Assist returned mixed content in one part.",
            ));
        }
        let call = call
            .as_object()
            .ok_or_else(|| protocol("Code Assist returned an invalid function call."))?;
        if call
            .keys()
            .any(|key| !["name", "args", "id"].contains(&key.as_str()))
        {
            return Err(capability(
                "Code Assist returned an unsupported function call field.",
            ));
        }
        if call
            .get("name")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
            || call
                .get("args")
                .is_some_and(|arguments| !arguments.is_object())
            || call
                .get("id")
                .is_some_and(|id| !id.is_null() && id.as_str().is_none_or(str::is_empty))
        {
            return Err(protocol("Code Assist returned an invalid function call."));
        }
    } else if part["thought"] == true {
        validate_thought_part(part)?;
    } else if !part["text"].is_string() {
        return Err(capability(
            "Code Assist returned an unsupported content part.",
        ));
    }
    Ok(())
}
fn thinking_signature(parts: &[Value], model: &str) -> ComponentResultV1<String> {
    if model.is_empty() {
        return Err(protocol("Code Assist thinking has no model identity."));
    }
    for part in parts {
        validate_thought_part(part)?;
    }
    let raw = json!([model, parts]).to_string();
    if raw.len() > LIMIT {
        return Err(protocol("Code Assist thinking exceeds its size limit."));
    }
    Ok(format!("ca_th1_{}", URL_SAFE_NO_PAD.encode(raw)))
}
fn decode_thinking(signature: &str, thinking: &str, model: &str) -> ComponentResultV1<Vec<Value>> {
    let encoded = signature.strip_prefix("ca_th1_").ok_or_else(|| {
        capability("Foreign opaque thinking cannot be transferred into Code Assist.")
    })?;
    if encoded.len() > LIMIT.div_ceil(3) * 4 {
        return Err(capability(
            "Google thinking history exceeds its size limit.",
        ));
    }
    let raw = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| capability("Invalid Google thinking history."))?;
    if raw.len() > LIMIT {
        return Err(capability(
            "Google thinking history exceeds its size limit.",
        ));
    }
    let value: Value =
        serde_json::from_slice(&raw).map_err(|_| capability("Invalid Google thinking history."))?;
    if value.as_array().is_none_or(|fields| fields.len() != 2)
        || value[0].as_str().map(tool_model_identity) != Some(tool_model_identity(model))
    {
        return Err(capability(
            "Google thinking history belongs to another model.",
        ));
    }
    let parts = value[1]
        .as_array()
        .filter(|parts| !parts.is_empty())
        .ok_or_else(|| capability("Invalid Google thinking history."))?;
    let mut text = String::new();
    for part in parts {
        validate_thought_part(part).map_err(|_| capability("Invalid Google thinking history."))?;
        if let Some(part_text) = part["text"].as_str() {
            text.push_str(part_text);
        }
    }
    if text != thinking {
        return Err(capability(
            "Google thinking text does not match its preserved state.",
        ));
    }
    Ok(parts.clone())
}
fn unwrap_response(parts: &HttpResponseParts) -> ComponentResultV1<HttpResponseParts> {
    if parts.body.len() > LIMIT {
        return Err(protocol("Code Assist response exceeds its size limit."));
    }
    let v: Value = serde_json::from_str(&parts.body)
        .map_err(|_| protocol("Code Assist returned invalid JSON."))?;
    if v.get("error").is_some() {
        return Err(protocol("Code Assist returned an embedded error."));
    }
    let inner = v
        .get("response")
        .ok_or_else(|| protocol("Code Assist response envelope is missing."))?;
    Ok(HttpResponseParts {
        body: inner.to_string(),
        ..parts.clone()
    })
}
fn adjust_usage(u: &mut Usage) {
    u.output_tokens = u.output_tokens.saturating_add(u.reasoning_tokens);
}
impl ProviderComponentV1 for CodeAssist {
    fn metadata(&self) -> ComponentMetadataV1 {
        ComponentMetadataV1 {
            name: "provider-codeassist".into(),
            version: "2.1.0".into(),
            api_version: PROVIDER_WORLD.into(),
        }
    }
    fn model_capabilities(&self, c: &ProviderConfig) -> ComponentResultV1<Vec<ModelCapability>> {
        checked(c)?;
        Ok(c.models.clone())
    }
    fn build_http_request(
        &self,
        r: &ChatRequest,
        c: &ProviderConfig,
    ) -> ComponentResultV1<HttpRequestDescriptor> {
        let ag = checked(c)?;
        for key in [
            "previous_response_id",
            "conversation",
            "native_tools",
            "additional_tools",
            "hosted_tools",
            "web_search",
            "anthropic_server_tools",
            "parallel_tool_calls",
        ] {
            if r.extensions.get(key).is_some_and(|v| !v.is_null()) {
                return Err(capability("This Google subscription cannot preserve the requested native state or generation constraint."));
            }
        }
        if let Some(value) = r
            .extensions
            .get("responses_tool_strict")
            .filter(|value| !value.is_null())
        {
            let controls = value
                .as_object()
                .filter(|controls| controls.values().all(Value::is_boolean))
                .ok_or_else(|| capability("Tool strictness must contain boolean values"))?;
            if controls.values().any(|strict| strict == true) {
                return Err(capability(
                    "Code Assist cannot preserve strict function tool constraints.",
                ));
            }
        }
        let mut thinking_parts = VecDeque::new();
        for m in &r.messages {
            let responses_thinking = m
                .extensions
                .get("responses_reasoning_encrypted_content")
                .map(|value| {
                    value
                        .as_str()
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| capability("Invalid opaque Google thinking history."))
                })
                .transpose()?;
            if responses_thinking.is_some()
                && (m.role != token_station_protocol::Role::Assistant
                    || !matches!(&m.content, Some(Content::Parts(parts)) if parts.iter().filter(|part| matches!(part, ContentPart::Thinking { .. })).count() == 1))
            {
                return Err(capability(
                    "Opaque Google thinking history requires one assistant thinking part.",
                ));
            }
            if let Some(Content::Parts(ps)) = &m.content {
                if matches!(
                    m.role,
                    token_station_protocol::Role::System | token_station_protocol::Role::Tool
                ) && ps
                    .iter()
                    .any(|part| !matches!(part, ContentPart::Text { .. }))
                {
                    return Err(capability(
                        "Code Assist requires text system instructions and tool results.",
                    ));
                }
                for part in ps {
                    match part {
                        ContentPart::RedactedThinking { .. } => {
                            return Err(capability(
                                "Opaque thinking state cannot be transferred into Code Assist.",
                            ));
                        }
                        ContentPart::Thinking {
                            thinking,
                            signature,
                        } => {
                            if signature
                                .as_deref()
                                .zip(responses_thinking)
                                .is_some_and(|(signature, opaque)| signature != opaque)
                            {
                                return Err(capability(
                                    "Google thinking history contains conflicting opaque state.",
                                ));
                            }
                            let preserved = signature.as_deref().or(responses_thinking).map(|signature| {
                                if m.role != token_station_protocol::Role::Assistant {
                                    return Err(capability("Google thinking history requires an assistant message."));
                                }
                                decode_thinking(signature, thinking, &r.model)
                            }).transpose()?;
                            thinking_parts.push_back(preserved);
                        }
                        _ => {}
                    }
                }
            }
        }
        let calls = r
            .messages
            .iter()
            .flat_map(|message| message.tool_calls.iter())
            .collect::<Vec<_>>();
        let results = r
            .messages
            .iter()
            .filter_map(|message| message.tool_call_id.as_deref())
            .collect::<Vec<_>>();
        let mut call_ids = BTreeSet::new();
        for call in &calls {
            if !call_ids.insert(call.id.as_str()) {
                return Err(capability(
                    "Historical Google tool call IDs must be unique.",
                ));
            }
            if !serde_json::from_str::<Value>(&call.arguments).is_ok_and(|value| value.is_object())
            {
                return Err(capability(
                    "Google tool arguments must contain a complete JSON object.",
                ));
            }
        }
        let mut result_ids = BTreeSet::new();
        if results
            .iter()
            .any(|id| !call_ids.contains(id) || !result_ids.insert(*id))
            || call_ids != result_ids
        {
            return Err(capability(
                "Historical Google calls require exactly one matching result.",
            ));
        }
        let mut gc = c.clone();
        gc.provider = "gemini".into();
        let mut body = GeminiReferenceV1
            .build_http_request(r, &gc)?
            .body
            .ok_or_else(|| protocol("Gemini translation produced no body."))?;
        for content in body["contents"].as_array_mut().into_iter().flatten() {
            if let Some(parts) = content["parts"].as_array_mut() {
                let mut restored = Vec::new();
                for part in std::mem::take(parts) {
                    if part["thought"] == true {
                        if let Some(original) = thinking_parts
                            .pop_front()
                            .ok_or_else(|| protocol("Google thinking history is inconsistent."))?
                        {
                            restored.extend(original);
                        } else {
                            restored.push(part);
                        }
                    } else {
                        restored.push(part);
                    }
                }
                *parts = restored;
            }
        }
        if !thinking_parts.is_empty() {
            return Err(protocol("Google thinking history is inconsistent."));
        }
        if let Some(format) = &r.response_format {
            if !matches!(format, ResponseFormat::Text) {
                if body.get("generationConfig").is_none() {
                    body["generationConfig"] = json!({});
                }
                body["generationConfig"]["responseMimeType"] = json!("application/json");
                if let ResponseFormat::JsonSchema { json_schema } = format {
                    let schema = json_schema.get("schema").unwrap_or(json_schema);
                    if !schema.is_object() {
                        return Err(capability("Code Assist requires an object JSON schema."));
                    }
                    body["generationConfig"]["responseJsonSchema"] = schema.clone();
                }
            }
        }
        if let Some(effort) = r.extensions.get("reasoning_effort").and_then(Value::as_str) {
            let level =
                match effort {
                    "low" | "high" => effort,
                    _ => return Err(capability(
                        "This Code Assist component supports low and high reasoning effort only.",
                    )),
                };
            if body.get("generationConfig").is_none() {
                body["generationConfig"] = json!({});
            }
            let thinking = if r.model.starts_with("gemini-3") {
                json!({"includeThoughts":true,"thinkingLevel":level})
            } else if r.model.starts_with("gemini-2") {
                json!({"includeThoughts":true,"thinkingBudget":if level=="low"{1024}else{24576}})
            } else {
                return Err(capability(
                    "Reasoning effort for this Code Assist model cannot be preserved.",
                ));
            };
            body["generationConfig"]["thinkingConfig"] = thinking;
        }
        let mut call_index = 0usize;
        let mut result_index = 0usize;
        for content in body["contents"].as_array_mut().into_iter().flatten() {
            for p in content["parts"].as_array_mut().into_iter().flatten() {
                if p.get("functionCall").is_some() {
                    let call = calls
                        .get(call_index)
                        .ok_or_else(|| protocol("Google tool history is inconsistent."))?;
                    let (id, sig) = decode_id(&call.id, &r.model)?;
                    if let Some(id) = id {
                        p["functionCall"]["id"] = json!(id);
                    }
                    if !sig.is_empty() {
                        p["thoughtSignature"] = json!(sig);
                    }
                    call_index += 1;
                }
                if let Some(resp) = p.get_mut("functionResponse") {
                    let source_id = results
                        .get(result_index)
                        .ok_or_else(|| protocol("Google tool results are inconsistent."))?;
                    let (id, _) = decode_id(source_id, &r.model)?;
                    result_index += 1;
                    if let Some(id) = id {
                        resp["id"] = json!(id);
                    }
                    let old = resp["response"]
                        .get("content")
                        .cloned()
                        .unwrap_or(Value::Null);
                    resp["response"] = if ag {
                        json!({"result":old})
                    } else {
                        json!({"output":old})
                    };
                }
            }
        }
        let digest = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&r.messages)
                    .map_err(|_| protocol("Cannot encode the conversation."))?
            )
        );
        let mut env =
            json!({"model":r.model,"project":c.extensions["google_project"],"request":body});
        if ag {
            env["userAgent"] = json!("antigravity");
            env["requestType"] = json!("agent");
            env["requestId"] = json!(format!("agent-{}", &digest[..24]));
            env["request"]["sessionId"] = json!(format!(
                "-{}",
                u64::from_str_radix(
                    &format!(
                        "{:x}",
                        Sha256::digest(
                            serde_json::to_vec(
                                &r.messages
                                    .iter()
                                    .find(|m| m.role == token_station_protocol::Role::User)
                            )
                            .map_err(|_| protocol("Cannot encode conversation identity."))?
                        )
                    )[..15],
                    16
                )
                .unwrap_or(1)
            ));
        } else {
            env["user_prompt_id"] = json!(&digest[..24]);
        }
        let version = c
            .extensions
            .get("google_client_version")
            .and_then(Value::as_str)
            .unwrap_or(if ag { "2.9.1" } else { "0.61.0" });
        if version.len() > 64
            || !version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
        {
            return Err(capability("Invalid Google client version."));
        }
        let ua = if ag {
            format!("antigravity/hub/{version}")
        } else {
            format!("GeminiCLI/{version}/{} (Token Station; terminal)", r.model)
        };
        let method = if r.stream {
            "streamGenerateContent?alt=sse"
        } else {
            "generateContent"
        };
        let mut d = HttpRequestDescriptor::new(
            HttpMethod::Post,
            format!("{}/v1internal:{method}", c.base_url.as_str()),
        );
        d.body = Some(env);
        d.headers = SafeHeaders::try_new([
            ("content-type", "application/json"),
            ("user-agent", ua.as_str()),
        ])
        .map_err(|_| capability("Unsafe Google request headers."))?;
        d.auth = c.auth.clone().map(Auth::bearer);
        Ok(d)
    }
    fn parse_response(&self, p: &HttpResponseParts) -> ComponentResultV1<ChatResponse> {
        let inner = unwrap_response(p)?;
        let raw: Value = serde_json::from_str(&inner.body)
            .map_err(|_| protocol("Invalid Code Assist response."))?;
        for part in raw["candidates"][0]["content"]["parts"]
            .as_array()
            .into_iter()
            .flatten()
        {
            validate_output_part(part)?;
        }
        let mut out = GeminiReferenceV1.parse_response(&inner)?;
        let parts = raw["candidates"][0]["content"]["parts"].as_array();
        if parts
            .into_iter()
            .flatten()
            .any(|p| p.get("inlineData").is_some() || p.get("fileData").is_some())
        {
            return Err(capability(
                "Code Assist returned content that the current response contract cannot preserve.",
            ));
        }
        let thoughts = parts
            .into_iter()
            .flatten()
            .filter(|part| part["thought"] == true && part.get("functionCall").is_none())
            .cloned()
            .collect::<Vec<_>>();
        if !thoughts.is_empty() {
            let signature = thinking_signature(&thoughts, &out.model)?;
            let thinking = thoughts
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect::<String>();
            let mut content = match out.choices[0].message.content.take() {
                Some(Content::Text(text)) => vec![ContentPart::Text { text }],
                Some(Content::Parts(parts)) => parts
                    .into_iter()
                    .filter(|part| !matches!(part, ContentPart::Thinking { .. }))
                    .collect(),
                None => Vec::new(),
            };
            content.insert(
                0,
                ContentPart::Thinking {
                    thinking,
                    signature: Some(signature),
                },
            );
            out.choices[0].message.content = Some(Content::Parts(content));
        }
        // Non-function text signatures are optional transport metadata. Preserve
        // their exact values in buffered IR without mislabeling visible text as thinking.
        // https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures#signatures-in-non-functioncall-parts
        let optional_signatures = parts
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, part)| {
                part.get("text").is_some()
                    && part["thought"].as_bool() != Some(true)
                    && part.get("functionCall").is_none()
            })
            .filter_map(|(index, part)| {
                part.get("thoughtSignature")
                    .map(|signature| (index, part, signature))
            })
            .map(|(index, part, signature)| {
                if !signature.is_string() {
                    return Err(protocol(
                        "Code Assist returned an invalid optional text signature.",
                    ));
                }
                Ok(json!({"part_index":index,"text":part["text"],"thoughtSignature":signature}))
            })
            .collect::<ComponentResultV1<Vec<_>>>()?;
        if !optional_signatures.is_empty() {
            out.choices[0].message.extensions.insert(
                "codeassist_optional_text_signatures".into(),
                json!({"model":out.model,"parts":optional_signatures}),
            );
        }
        let wires = parts
            .into_iter()
            .flatten()
            .filter(|p| p.get("functionCall").is_some())
            .collect::<Vec<_>>();
        for (i, call) in out.choices[0].message.tool_calls.iter_mut().enumerate() {
            let wire = wires[i];
            call.id = signed_id(
                wire["functionCall"]["id"].as_str(),
                wire["thoughtSignature"].as_str().unwrap_or_default(),
                &out.model,
            )?;
        }
        adjust_usage(&mut out.usage);
        Ok(out)
    }
    fn map_provider_error(&self, p: &HttpResponseParts) -> ComponentResultV1<ErrorEnvelope> {
        GeminiReferenceV1.map_provider_error(p)
    }
    fn stream_parser(&self) -> Box<dyn StreamParserV1> {
        Box::new(stream::Parser::default())
    }
}

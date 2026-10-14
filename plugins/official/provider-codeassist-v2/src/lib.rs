//! Native Google Code Assist translation. The host owns credentials and I/O.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use south_component_conformance::{
    reference_gemini::GeminiReferenceV1, ComponentResultV1, ProviderComponentV1, StreamParserV1,
};
use south_provider_api::{ComponentMetadataV1, PROVIDER_WORLD};
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
fn signed_id(id: &str, sig: &str, model: &str) -> String {
    format!(
        "ca1_{}",
        URL_SAFE_NO_PAD.encode(json!([id, sig, model]).to_string())
    )
}
fn decode_id(id: &str, model: &str) -> ComponentResultV1<(String, String)> {
    let raw = URL_SAFE_NO_PAD
        .decode(id.strip_prefix("ca1_").ok_or_else(|| {
            capability("Foreign tool history cannot preserve Google thought signatures.")
        })?)
        .map_err(|_| capability("Invalid Google tool identity."))?;
    if raw.len() > 64 * 1024 {
        return Err(capability("Google tool identity exceeds its size limit."));
    }
    let v: Value =
        serde_json::from_slice(&raw).map_err(|_| capability("Invalid Google tool identity."))?;
    if v[2].as_str() != Some(model) {
        return Err(capability("Google tool history belongs to another model."));
    }
    Ok((
        v[0].as_str()
            .ok_or_else(|| capability("Invalid Google tool identity."))?
            .into(),
        v[1].as_str()
            .ok_or_else(|| capability("Invalid Google tool identity."))?
            .into(),
    ))
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
        for m in &r.messages {
            if let Some(Content::Parts(ps)) = &m.content {
                if ps.iter().any(|p| {
                    matches!(
                        p,
                        ContentPart::RedactedThinking { .. }
                            | ContentPart::Thinking {
                                signature: Some(_),
                                ..
                            }
                    )
                }) {
                    return Err(capability(
                        "Opaque thinking state cannot be transferred into Code Assist.",
                    ));
                }
            }
        }
        let mut gc = c.clone();
        gc.provider = "gemini".into();
        let mut body = GeminiReferenceV1
            .build_http_request(r, &gc)?
            .body
            .ok_or_else(|| protocol("Gemini translation produced no body."))?;
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
        for call in r.messages.iter().flat_map(|m| &m.tool_calls) {
            if serde_json::from_str::<Value>(&call.arguments).is_err() {
                return Err(capability(
                    "Google tool arguments must contain complete JSON.",
                ));
            }
        }
        let calls = r
            .messages
            .iter()
            .flat_map(|m| m.tool_calls.iter())
            .collect::<Vec<_>>();
        let results = r
            .messages
            .iter()
            .filter_map(|m| m.tool_call_id.as_deref())
            .collect::<Vec<_>>();
        if calls.len() != results.len()
            || calls
                .iter()
                .any(|call| results.iter().filter(|id| **id == call.id).count() != 1)
        {
            return Err(capability(
                "Historical Google calls require exactly one matching result.",
            ));
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
                    p["functionCall"]["id"] = json!(id);
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
                    resp["id"] = json!(id);
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
                wire["functionCall"]["id"].as_str().unwrap_or(&call.id),
                wire["thoughtSignature"].as_str().unwrap_or_default(),
                &out.model,
            );
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

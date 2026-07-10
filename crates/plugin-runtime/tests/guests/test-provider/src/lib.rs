//! The runtime's test guest: a provider adapter whose misbehaviour is driven
//! by its inputs.
//!
//! The sandbox tests need a guest that hangs, allocates without bound, panics,
//! and asks to sign things — on demand, from the outside. Magic keys in the
//! input's `extensions` bag trigger each. Everything else behaves like a small
//! but honest OpenAI-flavoured provider adapter, so the happy-path tests
//! exercise real translation rather than an echo server.

use std::sync::Mutex;

wit_bindgen::generate!({
    path: "../../../../plugin-api/wit",
    world: "provider-adapter-v1",
});

use exports::token_station::adapter::provider_adapter::{AdapterHealth, AdapterMetadata, Guest};
use serde_json::{Value, json};
use token_station::adapter::common::{AdapterKind, HealthStatus};
use token_station::adapter::host;

/// The unparsed tail of the stream this instance is holding.
///
/// Instance state on purpose: the host promises one component instance per
/// stream, and the isolation test proves two parsers do not share this buffer.
static STREAM_BUFFER: Mutex<String> = Mutex::new(String::new());

struct TestProvider;

fn error_envelope(code: &str, http_status: u16, message: &str) -> String {
    json!({ "code": code, "http_status": http_status, "message": message }).to_string()
}

fn parse(input: &str) -> Result<Value, String> {
    serde_json::from_str(input)
        .map_err(|error| error_envelope("internal", 500, &format!("input is not JSON: {error}")))
}

/// Honours the magic keys that make this guest hostile on demand.
fn obey_magic(value: &Value) {
    if value.get("__hang").is_some() {
        // Burn forever; the host's epoch deadline is what stops this.
        loop {
            std::hint::black_box(0);
        }
    }
    if let Some(mb) = value.get("__grow_mb").and_then(Value::as_u64) {
        // Touch every page so the growth is real, not just reserved.
        let mut hog: Vec<u8> = Vec::new();
        hog.resize(usize::try_from(mb).unwrap_or(usize::MAX) * 1024 * 1024, 1);
        std::hint::black_box(&hog);
    }
    if value.get("__panic").is_some() {
        panic!("the input told me to");
    }
}

impl Guest for TestProvider {
    fn metadata() -> AdapterMetadata {
        AdapterMetadata {
            name: "test-provider".to_owned(),
            version: "1.0.0".to_owned(),
            kind: AdapterKind::Provider,
            api_version: "provider-adapter-v1".to_owned(),
        }
    }

    fn healthcheck() -> AdapterHealth {
        AdapterHealth {
            status: HealthStatus::Ready,
            detail: None,
        }
    }

    fn model_capabilities(provider_config: String) -> Result<String, String> {
        let config = parse(&provider_config)?;
        obey_magic(&config);

        // No network, so the operator's declaration is all there is.
        Ok(config
            .get("models")
            .cloned()
            .unwrap_or_else(|| json!([]))
            .to_string())
    }

    fn build_http_request(chat_request: String, provider_config: String) -> Result<String, String> {
        let request = parse(&chat_request)?;
        let config = parse(&provider_config)?;
        obey_magic(&request);

        let base_url = config
            .get("base_url")
            .and_then(Value::as_str)
            .ok_or_else(|| error_envelope("internal", 500, "config has no base_url"))?;

        let mut descriptor = json!({
            "method": "POST",
            "url": format!("{base_url}/chat/completions"),
            "headers": { "content-type": "application/json" },
            "body": { "model": request["model"], "messages": request["messages"] },
        });

        if let Some(secret) = config.get("auth").and_then(Value::as_str) {
            descriptor["auth"] = json!({ "scheme": "bearer", "secret": secret });
        }

        // `__sign` asks the host to sign; the manifest boundary is the host's.
        if let Some(ask) = request.get("__sign") {
            let secret = ask["secret"].as_str().unwrap_or_default();
            let algorithm = ask["algorithm"].as_str().unwrap_or("hmac-sha256");
            match host::sign(secret, b"payload", algorithm) {
                Ok(signature) => {
                    descriptor["signature_bytes"] = json!(signature.len());
                }
                Err(refusal) => {
                    return Err(error_envelope("internal", 500, &refusal));
                }
            }
        }

        Ok(descriptor.to_string())
    }

    fn parse_response(response_parts: String) -> Result<String, String> {
        let parts = parse(&response_parts)?;
        let body: Value = serde_json::from_str(parts["body"].as_str().unwrap_or("{}"))
            .map_err(|error| error_envelope("internal", 500, &format!("body: {error}")))?;

        Ok(json!({
            "id": body["id"],
            "model": body["model"],
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": body["content"] },
                "finish_reason": "stop",
            }],
            "usage": { "input_tokens": 1, "output_tokens": 1 },
        })
        .to_string())
    }

    fn parse_stream_chunk(chunk: String) -> Result<String, String> {
        let parsed = parse(&chunk)?;
        let data = parsed["data"].as_str().unwrap_or_default();

        let mut buffer = STREAM_BUFFER.lock().expect("single-threaded guest");
        buffer.push_str(data);

        let mut events = Vec::new();
        while let Some(end) = buffer.find("\n\n") {
            let frame: String = buffer.drain(..end + 2).collect();
            if let Some(content) = frame.strip_prefix("data: ") {
                events.push(json!({
                    "type": "delta",
                    "index": 0,
                    "content": content.trim_end(),
                }));
            }
        }
        Ok(Value::Array(events).to_string())
    }

    fn map_provider_error(response_parts: String) -> Result<String, String> {
        let parts = parse(&response_parts)?;
        let status = parts["status"].as_u64().unwrap_or(500);
        let (code, message) = match status {
            401 | 403 => ("auth", "the upstream rejected the credential"),
            429 => ("rate_limit", "the upstream rate limited this request"),
            _ => ("upstream_unavailable", "the upstream failed"),
        };
        Ok(error_envelope(
            code,
            u16::try_from(status).unwrap_or(500),
            message,
        ))
    }
}

export!(TestProvider);

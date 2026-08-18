//! Test-only Agent adapter that makes crossing the WASM trust boundary observable.

wit_bindgen::generate!({
    path: "../../../../../crates/plugin-api/wit",
    world: "agent-adapter-v1",
});

use exports::token_station::adapter::agent_adapter::{
    AdapterHealth, AdapterMetadata, AgentProtocolCapability, Guest, MatchResult,
};
use serde_json::{Value, json};
use token_station::adapter::common::{AdapterKind, HealthStatus};

const PRIVATE_MARKER: &str = "token_station_private_no_attempt_fallback";

struct MarkerAgent;

fn parse(input: &str) -> Result<Value, String> {
    serde_json::from_str(input).map_err(|error| {
        json!({"code":"internal","http_status":500,"message":error.to_string()}).to_string()
    })
}

impl Guest for MarkerAgent {
    fn metadata() -> AdapterMetadata {
        AdapterMetadata {
            name: "marker-agent".to_owned(),
            version: "1.0.0".to_owned(),
            kind: AdapterKind::Agent,
            api_version: "agent-adapter-v1".to_owned(),
        }
    }

    fn healthcheck() -> AdapterHealth {
        AdapterHealth {
            status: HealthStatus::Ready,
            detail: None,
        }
    }

    fn supported_agent_protocols() -> Vec<AgentProtocolCapability> {
        vec![AgentProtocolCapability {
            protocol: "marker-test".to_owned(),
            agent_tools: Vec::new(),
        }]
    }

    fn match_inbound(request_head: String) -> MatchResult {
        let matched = parse(&request_head)
            .ok()
            .and_then(|head| head.get("path").cloned())
            .and_then(|path| path.as_str().map(|path| path == "/v1/chat/completions"))
            .unwrap_or(false);
        MatchResult {
            matched,
            protocol: matched.then(|| "marker-test".to_owned()),
        }
    }

    fn normalize_inbound(envelope: String) -> Result<String, String> {
        let envelope = parse(&envelope)?;
        let body = &envelope["body"];
        Ok(json!({
            "model": body["model"],
            "messages": body["messages"],
            "sampling": {},
            "stream": body["stream"].as_bool().unwrap_or(false)
        })
        .to_string())
    }

    fn extract_agent_hint(_envelope: String) -> Result<String, String> {
        Ok("[]".to_owned())
    }

    fn render_response(response: String, _context: String) -> Result<String, String> {
        Ok(response)
    }

    fn render_stream_event(event: String, _context: String) -> Result<String, String> {
        if parse(&event)?["type"] == "error" {
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        Ok(json!({"data": ""}).to_string())
    }

    fn map_inbound_error(error: String, _context: String) -> Result<String, String> {
        let error = parse(&error)?;
        Ok(json!({
            "saw_private_marker": error.get(PRIVATE_MARKER).is_some()
        })
        .to_string())
    }
}

export!(MarkerAgent);

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};
use south_provider_runtime::{CallErrorV1, LoadedComponentV1, NoSecretsV1};
use token_station_cli::south_component::{host_expectations, south_component_runtime};

fn component() -> LoadedComponentV1 {
    let package = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root")
        .join("plugins/official/provider-codeassist-v2");
    let status = Command::new("cargo")
        .args([
            "build",
            "--locked",
            "--offline",
            "--release",
            "--target",
            "wasm32-wasip2",
        ])
        .current_dir(&package)
        .status()
        .expect("cargo runs");
    assert!(status.success(), "Code Assist component builds");
    let wasm =
        std::fs::read(package.join("target/wasm32-wasip2/release/provider_codeassist_v2.wasm"))
            .expect("component reads");
    let manifest = std::fs::read_to_string(package.join("manifest.json")).expect("manifest reads");
    LoadedComponentV1::load_embedded(
        &south_component_runtime().expect("production runtime builds"),
        &manifest,
        &wasm,
        &host_expectations(),
        NoSecretsV1,
    )
    .expect("Code Assist loads through production admission gates")
}

fn stream_events(component: &LoadedComponentV1, wire: &str) -> Vec<Value> {
    let mut stream = component.open_stream().expect("stream opens");
    let mut events = Vec::new();
    for chunk in wire.as_bytes().chunks(1) {
        events.extend(
            serde_json::from_str::<Vec<Value>>(
                &stream.parse_chunk(chunk).expect("byte fragment parses"),
            )
            .expect("canonical events decode"),
        );
    }
    events.extend(
        serde_json::from_str::<Vec<Value>>(
            &stream.parse_chunk(&[]).expect("complete stream finishes"),
        )
        .expect("terminal events decode"),
    );
    events
}

fn assert_capability(error: CallErrorV1) {
    let CallErrorV1::Component(error) = error else {
        panic!("Expected a capability error, received {error:?}");
    };
    let error: Value = serde_json::from_str(&error).expect("component error decodes");
    assert_eq!(error["code"], "capability");
}

fn assert_unique_tools(component: &LoadedComponentV1, config: &str) {
    let tool_response = json!({"response":{
        "modelVersion":"gemini-3.8-flash-n",
        "candidates":[{"content":{"parts":[{"functionCall":{"name":"read","args":{}}}]},"finishReason":"STOP"}]
    }});
    let mut identities = BTreeSet::new();
    for streamed in [false, true] {
        let mut messages = Vec::new();
        for result in ["first result", "second result"] {
            let call = if streamed {
                let events = stream_events(component, &format!("data: {tool_response}\n\n"));
                let event = events
                    .iter()
                    .find(|event| event["type"] == "tool_call_delta")
                    .expect("stream returns a tool call");
                json!({"id":event["id"],"name":event["name"],"arguments":event["arguments_delta"]})
            } else {
                let response = component
                    .call_parse_response(
                        &json!({"status":200,"headers":{},"body":tool_response.to_string()})
                            .to_string(),
                    )
                    .expect("buffered tool response parses");
                let response: Value = serde_json::from_str(&response).expect("response decodes");
                response["choices"][0]["message"]["tool_calls"][0].clone()
            };
            let identity = call["id"].as_str().expect("tool ID is a string");
            assert!(identity.starts_with("ca2_"));
            assert!(
                identities.insert(identity.to_owned()),
                "Tool IDs must differ across component instances"
            );
            messages.push(json!({"role":"assistant","tool_calls":[call]}));
            messages.push(json!({"role":"tool","tool_call_id":identity,"content":result}));
        }
        let descriptor = component
            .call_build_http_request(
                &json!({"model":"gemini-3.8-flash-medium","messages":messages}).to_string(),
                config,
            )
            .expect("two tool rounds replay");
        let descriptor: Value = serde_json::from_str(&descriptor).expect("descriptor decodes");
        for index in [0, 2] {
            let parts = &descriptor["body"]["request"]["contents"];
            assert!(parts[index]["parts"][0]["functionCall"].get("id").is_none());
            assert!(
                parts[index + 1]["parts"][0]["functionResponse"]
                    .get("id")
                    .is_none()
            );
        }
    }
}

fn assert_signed_thinking(component: &LoadedComponentV1, config: &str) {
    let thoughts = vec![
        json!({"text":"first α","thought":true,"thoughtSignature":"signature-a"}),
        json!({"text":"second β","thought":true,"thoughtSignature":"signature-b"}),
        json!({"thought":true,"thoughtSignature":"signature-trailer"}),
    ];
    let mut wire = String::new();
    for thought in &thoughts {
        write!(wire, "data: {}\n\n", json!({"response":{"modelVersion":"gemini-3.8-flash-n","candidates":[{"content":{"parts":[thought]}}]}})).expect("SSE frame encodes");
    }
    write!(
        wire,
        "data: {}\n\n",
        json!({"response":{"candidates":[{"finishReason":"STOP"}]}})
    )
    .expect("terminal SSE frame encodes");
    let events = stream_events(component, &wire);
    let thinking = events
        .iter()
        .filter_map(|event| event["thinking_delta"].as_str())
        .collect::<String>();
    let signatures = events
        .iter()
        .filter_map(|event| event["signature_delta"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(thinking, "first αsecond β");
    assert_eq!(signatures.len(), 1);
    assert!(signatures[0].starts_with("ca_th1_"));
    let mut request = json!({"model":"gemini-3.8-flash-medium","messages":[
        {"role":"assistant","content":[{"type":"thinking","thinking":thinking,"signature":signatures[0]}]},
        {"role":"user","content":"continue"}
    ]});
    let descriptor = component
        .call_build_http_request(&request.to_string(), config)
        .expect("signed thinking replays through the real component");
    let descriptor: Value = serde_json::from_str(&descriptor).expect("descriptor decodes");
    assert_eq!(
        descriptor["body"]["request"]["contents"][0]["parts"],
        json!(thoughts)
    );
    request["model"] = json!("gemini-3.8-pro");
    assert_capability(
        component
            .call_build_http_request(&request.to_string(), config)
            .expect_err("foreign model thinking must fail"),
    );
    request["model"] = json!("gemini-3.8-flash-medium");
    request["messages"][0]["content"][0]["thinking"] = json!("modified history");
    assert_capability(
        component
            .call_build_http_request(&request.to_string(), config)
            .expect_err("changed thinking text must fail"),
    );
}

#[test]
fn real_wasm_preserves_unique_tool_and_signed_thinking_history() {
    let component = component();
    let config = json!({
        "provider":"gemini-cli-plan",
        "base_url":"https://cloudcode-pa.googleapis.com",
        "auth":"local_subscription_token",
        "google_project":"fixture-project"
    })
    .to_string();
    assert_unique_tools(&component, &config);
    assert_signed_thinking(&component, &config);
}

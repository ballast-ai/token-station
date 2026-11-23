use provider_chatgpt_plan_v2::ChatGptPlan;
use serde_json::{json, Value};
use south_component_conformance::ProviderComponentV1;
use token_station_protocol::{ChatRequest, ErrorCode, ProviderConfig};

fn config(endpoint: &str) -> ProviderConfig {
    serde_json::from_value(json!({"provider":"codex-plan","base_url":endpoint,"auth":"codex_access_token","codex_account_id":"account-fixture-123","codex_client_version":"0.162.0"})).unwrap()
}
fn request() -> ChatRequest {
    serde_json::from_value(json!({"model":"fixture-model","messages":[{"role":"system","content":"Keep the caller instructions."},{"role":"user","content":"Read the file"}],"tools":[{"name":"read","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}],"tool_choice":"required","parallel_tool_calls":false,"reasoning_effort":"low","response_format":{"type":"json_schema","json_schema":{"name":"result","schema":{"type":"object"},"strict":true}}})).unwrap()
}
#[test]
fn codex_has_bound_headers_and_preserves_constraints_in_responses_body() {
    let descriptor = ChatGptPlan
        .build_http_request(&request(), &config("https://chatgpt.com/backend-api/codex"))
        .unwrap();
    let v = serde_json::to_value(&descriptor).unwrap();
    assert_eq!(v["url"], "https://chatgpt.com/backend-api/codex/responses");
    assert_eq!(
        v["auth"],
        json!({"scheme":"bearer","secret":"codex_access_token"})
    );
    for (header, value) in [
        ("chatgpt-account-id", "account-fixture-123"),
        ("version", "0.162.0"),
        ("originator", "codex_cli_rs"),
        ("openai-beta", "responses=experimental"),
    ] {
        assert_eq!(v["headers"][header], value);
    }
    assert!(v["headers"]["user-agent"]
        .as_str()
        .unwrap()
        .starts_with("codex_cli_rs/0.162.0 "));
    assert!(v["headers"].get("authorization").is_none());
    let b = &v["body"];
    assert_eq!(b["instructions"], "Keep the caller instructions.");
    assert_eq!(b["tool_choice"], "required");
    assert_eq!(b["parallel_tool_calls"], false);
    assert_eq!(b["reasoning"]["effort"], "low");
    assert_eq!(b["text"]["format"]["type"], "json_schema");
    assert_eq!(b["tools"][0]["type"], "namespace");
    assert_eq!(b["tools"][0]["tools"][0]["name"], "read");
    assert_eq!(b["stream"], true);
    assert_eq!(b["store"], false);
    config("https://chatgpt.com/backend-api/codex")
        .authorize(&descriptor)
        .unwrap();
}
#[test]
fn codex_minimum_body_and_long_history_call_ids_remain_paired() {
    let long = "id".repeat(60);
    let r:ChatRequest=serde_json::from_value(json!({"model":"m","messages":[{"role":"assistant","tool_calls":[{"id":long,"name":"read","arguments":"{}"}]},{"role":"tool","tool_call_id":long,"content":"actual result"}]})).unwrap();
    let d = serde_json::to_value(
        ChatGptPlan
            .build_http_request(&r, &config("https://chatgpt.com/backend-api/codex"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(d["body"]["instructions"], "");
    assert_eq!(d["body"]["tool_choice"], "auto");
    assert_eq!(d["body"]["parallel_tool_calls"], true);
    let a = &d["body"]["input"][0]["call_id"];
    let b = &d["body"]["input"][1]["call_id"];
    assert_eq!(a, b);
    assert_eq!(a.as_str().unwrap().len(), 64);
    assert_eq!(d["body"]["input"][1]["output"], "actual result");
}
#[test]
fn codex_endpoint_slot_and_client_metadata_fail_closed() {
    for endpoint in [
        "http://chatgpt.com/backend-api/codex",
        "https://chatgpt.com.evil.test/backend-api/codex",
        "https://api.openai.com/v1",
        "https://chatgpt.com/backend-api/codex-other",
    ] {
        assert!(
            ChatGptPlan
                .build_http_request(&request(), &config(endpoint))
                .is_err(),
            "{endpoint}"
        );
    }
    for endpoint in [
        "http://127.0.0.1:9876/backend-api/codex",
        "http://[::1]:9876/backend-api/codex",
        "http://localhost:9876/v1",
    ] {
        assert!(ChatGptPlan
            .build_http_request(&request(), &config(endpoint))
            .is_ok());
    }
    for (key, value) in [
        (
            "codex_account_id",
            json!("account\r\nAuthorization: secret"),
        ),
        ("codex_account_id", Value::Null),
        ("codex_client_version", json!("0.162.0\r\nCookie: secret")),
        ("codex_client_version", json!("invalid")),
    ] {
        let mut c = config("https://chatgpt.com/backend-api/codex");
        c.extensions.insert(key.into(), value);
        let error = ChatGptPlan.build_http_request(&request(), &c).unwrap_err();
        assert!(!serde_json::to_string(&error).unwrap().contains("secret"));
    }
    let mut c = config("https://chatgpt.com/backend-api/codex");
    c.auth = Some(serde_json::from_value(json!("chatgpt_access_token")).unwrap());
    assert_eq!(
        ChatGptPlan
            .build_http_request(&request(), &c)
            .unwrap_err()
            .code,
        ErrorCode::Auth
    );
}

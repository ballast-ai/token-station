//! Regression tests at the public Gateway boundary with real protocol adapters.
#[allow(clippy::wildcard_imports)]
use super::*;
use token_station_metrics::{CostKind, Recorder, RequestRecord};
use token_station_protocol::{ErrorCode, StreamOutcome};

#[derive(Default)]
struct Records(Mutex<Vec<RequestRecord>>);
impl Recorder for Records {
    fn record(&self, record: &RequestRecord) {
        self.0.lock().unwrap().push(record.clone());
    }
}

struct TestDirectory(PathBuf);
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    // Drop the gateway's database handles before removing its directory.
    gateway: Gateway,
    records: Arc<Records>,
    directory: TestDirectory,
}

impl Fixture {
    fn new(agent: &str, upstreams: Value, targets: Value) -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let directory = TestDirectory(std::env::temp_dir().join(format!(
            "ts-audit-hardening-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        )));
        let mut document = json!({
            "version":1,"server":{"listen":"127.0.0.1:0"},
            "data":{"dir":directory.0,"metrics":false},
            "plugins":{"dir":plugins_dir(),"agents":[agent],"providers":{
                "openai-compatible":"provider-openai-compatible-v2",
                "anthropic":"provider-anthropic-v2"
            }},
            "pricing":{"version":7,"models":{
                "a/model":{"input_per_mtok":1_000_000,"output_per_mtok":1_000_000},
                "b/model":{"input_per_mtok":2_000_000,"output_per_mtok":2_000_000}
            }},
            "router":{"version":1,"pools":{},"default_pool":"main"}
        });
        document["upstreams"] = upstreams;
        document["router"]["pools"]["main"] = targets;
        let config: ClientConfig = serde_json::from_value(document).unwrap();
        config.validate().unwrap();
        let records = Arc::new(Records::default());
        let gateway = Gateway::new(&config, records.clone()).unwrap();
        Self {
            gateway,
            records,
            directory,
        }
    }

    fn last(&self) -> RequestRecord {
        self.records.0.lock().unwrap().last().unwrap().clone()
    }
}

fn offering(mock: &MockUpstream, native: Option<&str>) -> Value {
    let mut value = json!({
        "provider":if native == Some("anthropic-native") {"anthropic"} else {"openai-compatible"},
        "base_url":mock.base_url(),
        "models":[{"model":"model","tool":true,"tool_state":"verified","context_window":400_000}],
        "quota_plan":{"windows":[{"len_ms":3_600_000,"limit":10_000}]}
    });
    if let Some(native) = native {
        value["api_dialect"] = json!(native);
    }
    value
}

fn target(name: &str) -> Value {
    json!({"upstream":name,"model":"model"})
}

#[test]
fn audit_hardening_retries_preserve_observed_usage_and_unknown_cost() {
    let first = MockUpstream::start(vec![vec![http_json(
        200,
        r#"{"id":"invalid","choices":[],"usage":{"prompt_tokens":100,"completion_tokens":20}}"#,
    )]]);
    let second = MockUpstream::start(vec![vec![http_json(
        200,
        r#"{"id":"ok","model":"model","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#,
    )]]);
    let fixture = Fixture::new(
        "agent-openai",
        json!({"a":offering(&first,None),"b":offering(&second,None)}),
        json!([target("a"), target("b")]),
    );
    fixture.gateway.chat(
        "POST",
        "/v1/chat/completions",
        &[],
        json!({"model":"auto","messages":[{"role":"user","content":"hello"}]})
            .to_string()
            .as_bytes(),
        &mut |_| true,
    );
    let record = fixture.last();
    assert_eq!(record.status, 200);
    assert_eq!(record.attempts, 2);
    assert_eq!(record.usage.unwrap().input_tokens, 110);
    assert_eq!(record.usage.unwrap().output_tokens, 22);
    assert!(record.usage_observation.unwrap().incomplete);
    assert!(
        record
            .usage_observation
            .unwrap()
            .cost_requires_attempt_pricing
    );
    assert_eq!(record.cost_kind, CostKind::Unknown);
    assert_eq!(record.cost_micros, None);
    let quota = fixture.gateway.quota_snapshot(quota_audit_now_ms());
    let used = |name| {
        quota["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["upstream"] == name)
            .unwrap()["windows"][0]["used"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(
        (used("a"), used("b")),
        (120, 12),
        "attempt quota must not double count aggregate usage"
    );
    let database = fixture.directory.0.join("metrics.sqlite");
    let store = token_station_cli::store::SqliteStore::open(&database).unwrap();
    store.record(&record);
    let receipts = token_station_cli::store::recent_receipts(&database, 1).unwrap();
    assert_eq!(receipts[0].usage.unwrap().input_tokens, 110);
    assert_eq!(receipts[0].cost_micros, None);
    let stats = token_station_cli::stats::collect(&database, None, None).unwrap();
    assert_eq!(stats.total.input_tokens, 110);
    assert_eq!(stats.total.cost_micros, None);
}

#[test]
fn audit_hardening_native_failed_stream_is_not_success_or_replayed() {
    let sse = "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\"},\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}\n\n";
    let response = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",sse.len()).into_bytes();
    let first = MockUpstream::start(vec![vec![response]]);
    let backup = MockUpstream::start(Vec::new());
    let fixture = Fixture::new(
        "agent-openai-responses",
        json!({"a":offering(&first,Some("responses-native")),"b":offering(&backup,Some("responses-native"))}),
        json!([target("a"), target("b")]),
    );
    let mut wire = String::new();
    fixture.gateway.chat(
        "POST",
        "/v1/responses",
        &[],
        json!({"model":"auto","input":"hello","tools":[{"type":"web_search"}],"stream":true})
            .to_string()
            .as_bytes(),
        &mut |reply| {
            if let Reply::Chunk(chunk) = reply {
                wire.push_str(&chunk);
            }
            true
        },
    );
    assert_eq!(wire, sse, "the native error event must remain unchanged");
    let record = fixture.last();
    assert_eq!(record.status, 502);
    assert_eq!(record.error_code, Some(ErrorCode::UpstreamUnavailable));
    assert_eq!(
        record.attempt_records[0].stream_outcome,
        Some(StreamOutcome::FailedAfterPartial)
    );
    assert!(!record.usage_observation.unwrap().incomplete);
    assert_eq!(record.cost_micros, Some(12));
    assert_eq!(record.usage.unwrap().input_tokens, 10);
    assert_eq!(
        backup.hits(),
        0,
        "never replay after a native event reached the client"
    );
}

#[test]
fn audit_hardening_native_json_honors_rejected_delivery() {
    for anthropic in [false, true] {
        let mock = MockUpstream::start(vec![vec![http_json(
            200,
            r#"{"id":"reply","status":"completed","usage":{"input_tokens":10,"output_tokens":2}}"#,
        )]]);
        let fixture = Fixture::new(
            if anthropic {
                "agent-anthropic"
            } else {
                "agent-openai-responses"
            },
            json!({"a":offering(&mock,Some(if anthropic {"anthropic-native"} else {"responses-native"}))}),
            json!([target("a")]),
        );
        let request = if anthropic {
            native_server_tool_turn()
        } else {
            json!({"model":"auto","input":"hello","tools":[{"type":"web_search"}]})
        };
        fixture.gateway.chat(
            "POST",
            if anthropic {
                "/v1/messages"
            } else {
                "/v1/responses"
            },
            &[],
            request.to_string().as_bytes(),
            &mut |_| false,
        );
        let record = fixture.last();
        assert_eq!(record.status, 499, "native protocol anthropic={anthropic}");
        assert_eq!(
            record.attempt_records[0].stream_outcome,
            Some(StreamOutcome::ClientCancelled)
        );
        assert_eq!(
            record.usage.unwrap().input_tokens,
            10,
            "delivery cancellation must retain billed usage"
        );
    }
}

#[test]
fn audit_hardening_anthropic_fallback_skips_chat_and_keeps_native_search_profile() {
    let first = MockUpstream::start(vec![vec![http_json(
        503,
        r#"{"error":{"type":"api_error"}}"#,
    )]]);
    let incompatible = MockUpstream::start(vec![vec![http_json(
        404,
        r#"{"error":{"message":"not found"}}"#,
    )]]);
    let compatible = MockUpstream::start(vec![vec![http_json(
        200,
        r#"{"id":"native-profile","type":"message","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":10,"output_tokens":2}}"#,
    )]]);
    let mut profile = offering(&compatible, None);
    profile["native_search"] = json!({"api_dialect":"anthropic-native","base_url":format!("{}/search",compatible.base_url()),"auth":"bearer"});
    let fixture = Fixture::new(
        "agent-anthropic",
        json!({"a":offering(&first,Some("anthropic-native")),"b":offering(&incompatible,None),"c":profile}),
        json!([target("a"), target("b"), target("c")]),
    );
    let mut status = 0;
    fixture.gateway.chat(
        "POST",
        "/v1/messages",
        &[],
        native_server_tool_turn().to_string().as_bytes(),
        &mut |reply| {
            if let Reply::BeginJson(reply) = reply {
                status = reply.status;
            }
            true
        },
    );
    assert_eq!(
        incompatible.hits(),
        0,
        "a native payload cannot reach a Chat-only fallback"
    );
    assert_eq!(status, 200);
    assert_eq!(compatible.hits(), 1);
    assert_eq!(compatible.seen()[0].path, "/v1/search/messages");
    assert_eq!(fixture.last().attempts, 2);
}

#[test]
fn audit_hardening_output_limit_incomplete_preserves_native_success() {
    let sse = "data: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"},\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}\n\n";
    let response = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",sse.len()).into_bytes();
    let mock = MockUpstream::start(vec![vec![response]]);
    let fixture = Fixture::new(
        "agent-openai-responses",
        json!({"a":offering(&mock,Some("responses-native"))}),
        json!([target("a")]),
    );
    let mut wire = String::new();
    fixture.gateway.chat(
        "POST",
        "/v1/responses",
        &[],
        json!({"model":"auto","input":"hello","tools":[{"type":"web_search"}],"stream":true})
            .to_string()
            .as_bytes(),
        &mut |reply| {
            if let Reply::Chunk(chunk) = reply {
                wire.push_str(&chunk);
            }
            true
        },
    );
    assert_eq!(wire, sse);
    let record = fixture.last();
    assert_eq!(record.status, 200);
    assert_eq!(record.error_code, None);
    assert_eq!(
        record.attempt_records[0].stream_outcome,
        Some(StreamOutcome::Complete)
    );
    assert!(
        !record.usage_observation.unwrap().incomplete,
        "a known output limit does not make reported token counts unknown"
    );
    assert_eq!(record.cost_micros, Some(12));
}

#[test]
fn audit_hardening_rejected_retry_preserves_final_target_pricing() {
    let first = MockUpstream::start(vec![vec![http_json(
        503,
        r#"{"error":{"message":"overloaded"}}"#,
    )]]);
    let second = MockUpstream::start(vec![vec![http_json(
        200,
        r#"{"id":"ok","model":"model","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#,
    )]]);
    let fixture = Fixture::new(
        "agent-openai",
        json!({"a":offering(&first,None),"b":offering(&second,None)}),
        json!([target("a"), target("b")]),
    );
    fixture.gateway.chat(
        "POST",
        "/v1/chat/completions",
        &[],
        json!({"model":"auto","messages":[{"role":"user","content":"hello"}]})
            .to_string()
            .as_bytes(),
        &mut |_| true,
    );
    let record = fixture.last();
    assert_eq!(record.usage.unwrap().input_tokens, 10);
    assert!(!record.usage_observation.unwrap().incomplete);
    assert_eq!(
        record.cost_micros,
        Some(24),
        "only a known rejected round can be excluded from aggregate pricing"
    );
    assert_eq!(record.cost_kind, CostKind::Estimated);
}

#[test]
fn audit_hardening_sync_native_json_failure_preserves_wire_without_replay() {
    for (status, error, failed) in [
        ("failed", json!({"code":"server_error"}), true),
        ("completed", json!({"code":"server_error"}), true),
        ("incomplete", Value::Null, false),
    ] {
        for hosted_search in [false, true] {
            let response = json!({
                "id":"resp_sync_terminal","object":"response","status":status,
                "background":false,"error":error,"output":[],
                "incomplete_details":{"reason":"max_output_tokens"},
                "usage":{"input_tokens":10,"output_tokens":2}
            })
            .to_string();
            let mock = MockUpstream::start(vec![vec![http_json(200, &response)]]);
            let backup = MockUpstream::start(Vec::new());
            let mut primary = offering(&mock, Some("responses-native"));
            let mut secondary = offering(&backup, Some("responses-native"));
            primary["models"][0]["vision"] = json!(true);
            secondary["models"][0]["vision"] = json!(true);
            let fixture = Fixture::new(
                "agent-openai-responses",
                json!({"a":primary,"b":secondary}),
                json!([target("a"), target("b")]),
            );
            let mut request =
                json!({"model":"auto","input":"hello","stream":false,"background":false});
            if hosted_search {
                request["tools"] = json!([{"type":"web_search"}]);
            } else {
                // Ordinary Responses image turns use the normalized native path.
                request["input"] = json!([{"type":"message","role":"user","content":[
                    {"type":"input_image","image_url":format!("data:image/png;base64,{VISION_REVIEW_PNG}")}
                ]}]);
            }
            let mut wire = Vec::new();
            fixture.gateway.chat(
                "POST",
                "/v1/responses",
                &[],
                request.to_string().as_bytes(),
                &mut |reply| {
                    if let Reply::BeginJson(reply) = reply {
                        wire.push((reply.status, reply.body));
                    }
                    true
                },
            );
            assert_eq!(
                wire,
                vec![(200, response)],
                "native JSON must remain unchanged"
            );
            assert_eq!(mock.seen()[0].body["background"], false);
            assert_eq!(mock.seen()[0].body["stream"], false);
            assert_eq!(
                backup.hits(),
                0,
                "do not replay an explicitly failed response"
            );
            let record = fixture.last();
            assert_eq!(record.status, if failed { 502 } else { 200 });
            assert_eq!(
                record.error_code,
                failed.then_some(ErrorCode::UpstreamUnavailable)
            );
            assert_eq!(
                record.attempt_records[0].stream_outcome,
                Some(if failed {
                    StreamOutcome::FailedAfterPartial
                } else {
                    StreamOutcome::Complete
                })
            );
            assert_eq!(record.usage.unwrap().input_tokens, 10);
            assert!(!record.usage_observation.unwrap().incomplete);
            assert_eq!(record.cost_micros, Some(12));
        }
    }
}

#[test]
fn audit_hardening_failed_terminal_requires_its_own_complete_usage() {
    let preliminary = "data: {\"type\":\"response.in_progress\",\"response\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}\n\n";
    for stream in [false, true] {
        for usage in [
            Value::Null,
            json!({"input_tokens":10}),
            json!({"output_tokens":2}),
            json!({"input_tokens":10,"output_tokens":-1}),
            json!({"input_tokens":10,"output_tokens":2,"output_tokens_details":{"reasoning_tokens":-1}}),
        ] {
            let body = json!({"status":"failed","error":{"code":"server_error"},"usage":usage});
            let response = if stream {
                // Earlier usage cannot certify fields absent from the terminal.
                let sse = format!(
                    "{preliminary}data: {}\n\n",
                    json!({"type":"response.failed","response":body})
                );
                format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}", sse.len()).into_bytes()
            } else {
                http_json(200, &body.to_string())
            };
            let mock = MockUpstream::start(vec![vec![response]]);
            let fixture = Fixture::new(
                "agent-openai-responses",
                json!({"a":offering(&mock,Some("responses-native"))}),
                json!([target("a")]),
            );
            fixture.gateway.chat(
                "POST", "/v1/responses", &[],
                json!({"model":"auto","input":"hello","tools":[{"type":"web_search"}],"stream":stream}).to_string().as_bytes(),
                &mut |_| true,
            );
            let record = fixture.last();
            assert_eq!(record.status, 502);
            assert_eq!(record.cost_kind, CostKind::Unknown);
            assert_eq!(record.cost_micros, None);
        }
    }
}

#[test]
fn audit_hardening_failed_terminal_with_truncated_or_abandoned_stream_keeps_unknown_cost() {
    let event = "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}";
    for sse in [
        // The native line parser sees a terminal, but the accounting frame is incomplete.
        format!("data: {{\"usage\":{{\"input_tokens\":10,\"output_tokens\":2}}}}\n\n{event}"),
        format!("{event}\n\n{}", "x".repeat(1024 * 1024 + 1)),
    ] {
        let response = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}", sse.len()).into_bytes();
        let mock = MockUpstream::start(vec![vec![response]]);
        let fixture = Fixture::new(
            "agent-openai-responses",
            json!({"a":offering(&mock,Some("responses-native"))}),
            json!([target("a")]),
        );
        fixture.gateway.chat(
            "POST",
            "/v1/responses",
            &[],
            json!({"model":"auto","input":"hello","tools":[{"type":"web_search"}],"stream":true})
                .to_string()
                .as_bytes(),
            &mut |_| true,
        );
        let record = fixture.last();
        assert_eq!(record.status, 502);
        assert!(record.usage_observation.unwrap().incomplete);
        assert_eq!(record.cost_kind, CostKind::Unknown);
        assert_eq!(record.cost_micros, None);
    }
}

#[test]
fn audit_hardening_failed_terminal_usage_must_parse_as_one_complete_frame() {
    let preliminary = "data: {\"type\":\"response.in_progress\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n";
    let terminal = "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"usage\":{\"input_tokens\":100,\"output_tokens\":20}}}\ndata: garbage\n\n";
    for suffix in ["", preliminary] {
        let sse = format!("{preliminary}{terminal}{suffix}");
        let response = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}", sse.len()).into_bytes();
        let mock = MockUpstream::start(vec![vec![response]]);
        let fixture = Fixture::new(
            "agent-openai-responses",
            json!({"a":offering(&mock,Some("responses-native"))}),
            json!([target("a")]),
        );
        fixture.gateway.chat(
            "POST",
            "/v1/responses",
            &[],
            json!({"model":"auto","input":"hello","tools":[{"type":"web_search"}],"stream":true})
                .to_string()
                .as_bytes(),
            &mut |_| true,
        );
        let record = fixture.last();
        assert_eq!(record.status, 502);
        assert!(record.usage_observation.unwrap().incomplete);
        assert_eq!(record.cost_kind, CostKind::Unknown);
        assert_eq!(record.cost_micros, None);
    }
}

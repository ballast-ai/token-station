//! Jev routing behavior with a bounded loopback classifier and no cloud access.

#[allow(clippy::wildcard_imports)]
use super::*;
use crate::jev::JevController;
use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use token_station_protocol::{AgentHint, HintKind, Role};
use token_station_router_core::{DecidedBy, Health, HintRoute, Match, Rule};

pub(super) struct Endpoint {
    pub(super) url: String,
    hits: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    #[cfg(feature = "builtin-plugins")]
    seen: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Endpoint {
    #[allow(clippy::needless_pass_by_value)] // Own the fixture's synthetic JSON response.
    pub(super) fn new(status: u16, body: Value) -> Self {
        Self::with_body(status, "application/json", body.to_string())
    }

    fn with_body(status: u16, content_type: &str, body: String) -> Self {
        Self::with_delay(status, content_type, body, Duration::ZERO)
    }

    fn with_delay(status: u16, content_type: &str, body: String, delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_hits = Arc::clone(&hits);
        let worker_stop = Arc::clone(&stop);
        let content_type = content_type.to_owned();
        #[cfg(feature = "builtin-plugins")]
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        #[cfg(feature = "builtin-plugins")]
        let worker_seen = Arc::clone(&seen);
        let worker = std::thread::spawn(move || {
            while !worker_stop.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 4096];
                while let Ok(count) = stream.read(&mut buffer) {
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&request[..end]).to_lowercase();
                        let length = header
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .and_then(|value| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                worker_hits.fetch_add(1, Ordering::SeqCst);
                #[cfg(feature = "builtin-plugins")]
                worker_seen
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(request).unwrap());
                std::thread::sleep(delay);
                let response = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            url,
            hits,
            stop,
            worker: Some(worker),
            #[cfg(feature = "builtin-plugins")]
            seen,
        }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.worker.take().unwrap().join().unwrap();
    }
}

struct Fixture {
    data: PathBuf,
    endpoint: Endpoint,
    controller: Arc<JevController>,
    gateway: Gateway,
}

impl Fixture {
    fn new(enabled: bool) -> Self {
        Self::with_response(
            enabled,
            200,
            json!({
                "model": "jev-latest", "usage": {"input_tokens":1,"output_tokens":1},
                "answers": {"tier": {"type":"choice", "choice":"high", "confidence":0.9,
                    "probabilities":{"low":0.05,"medium":0.05,"high":0.9}}}
            }),
        )
    }

    fn with_response(enabled: bool, status: u16, response: Value) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let data = std::env::temp_dir().join(format!(
            "ts-jev-gateway-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&data).unwrap();
        let endpoint = Endpoint::new(status, response);
        let controller = JevController::for_test(&data, &endpoint.url);
        controller.save_key("synthetic-jev-key").unwrap();
        if enabled {
            controller.set_enabled(true).unwrap();
        }
        let mut config: ClientConfig = serde_json::from_str(crate::EXAMPLE_CONFIG).unwrap();
        config.data.dir.clone_from(&data);
        let gateway = Gateway {
            agents: Vec::new(),
            skipped_agents: Vec::new(),
            home_router: None,
            home_dynamic_router: None,
            agent_routers: std::sync::RwLock::new(BTreeMap::new()),
            supported_agent_ids: BTreeSet::new(),
            upstreams: BTreeMap::new(),
            local_upstreams: BTreeSet::new(),
            free_upstreams: BTreeSet::new(),
            catalog: Vec::new(),
            image_observations: std::sync::Mutex::new(BTreeMap::new()),
            health: std::sync::Mutex::new(HealthTracker::new(HealthPolicy {
                eject_after: 3,
                cooldown: Duration::from_secs(1),
            })),
            quota: std::sync::Mutex::new(crate::quota_tracker::QuotaTracker::new(
                std::collections::HashMap::new(),
            )),
            admission: Admission::new(config.concurrency),
            pricing: config.pricing.clone(),
            secrets: SecretStore::from_config(&config, &data),
            egress: EgressPolicy::new(config.egress),
            south_runtime: None,
            recorder: Arc::new(token_station_metrics::NoopRecorder),
            body_log: None,
            semantic: None,
            jev: Arc::clone(&controller),
            search: crate::search::SearchController::shared(&config.data.dir),
        };
        Self {
            data,
            endpoint,
            controller,
            gateway,
        }
    }

    fn route(
        &self,
        router: &Router,
        request: &ChatRequest,
        hints: &[AgentHint],
        candidates: &[Candidate],
    ) -> Decision {
        self.gateway
            .route_with_semantics(
                &RequestContext::detached(Duration::from_secs(3), Duration::from_secs(1)),
                router,
                request,
                hints,
                candidates,
                "test",
            )
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.controller.set_enabled(false);
        let _ = std::fs::remove_dir_all(&self.data);
    }
}

fn target(model: &str) -> UpstreamModel {
    UpstreamModel::new(UpstreamRef::new("fixture").unwrap(), model)
}

fn config() -> RouterConfig {
    serde_json::from_value(json!({"version":1,"pools":{
        "tier_low":[target("low")],"tier_mid":[target("medium")],"tier_high":[target("high")]},
        "default_pool":"tier_low"}))
    .unwrap()
}

fn candidates() -> Vec<Candidate> {
    ["low", "medium", "high"]
        .map(|model| {
            Candidate::new(
                target(model),
                ModelCapability {
                    model: model.into(),
                    tool: true,
                    json_schema: true,
                    context_window: 128_000,
                    ..ModelCapability::default()
                },
                Health::Healthy,
            )
        })
        .to_vec()
}

fn request() -> ChatRequest {
    ChatRequest::new(
        "auto",
        vec![Message::text(Role::User, "Explain this function")],
    )
}

#[test]
fn jev_disabled_preserves_the_entire_baseline_without_egress() {
    let fixture = Fixture::new(false);
    let router = Router::new(config()).unwrap();
    assert_eq!(
        fixture.route(&router, &request(), &[], &candidates()),
        router.route(&request(), &[], &candidates()).unwrap()
    );
    assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 0);
}

#[test]
fn jev_valid_tier_uses_the_router_and_retains_stream_and_tool_requirements() {
    let fixture = Fixture::new(true);
    let router = Router::new(config()).unwrap();
    let mut request = request();
    request.stream = true;
    request.tools.push(ToolDef {
        name: "read_file".into(),
        description: None,
        parameters: json!({}),
    });
    let original = request.clone();
    let decision = fixture.route(&router, &request, &[], &candidates());
    assert_eq!(decision.chosen, target("high"));
    assert_eq!(decision.decided_by, DecidedBy::Classifier);
    assert_eq!(request, original);
    assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 1);
}

#[test]
fn jev_exact_auto_stays_local_and_the_host_dynamic_variant_can_classify() {
    let fixture = Fixture::new(true);
    let mut config = config();
    config.honor_exact_model = true;
    let (exact, dynamic) = router_with_dynamic_variant(config, "fixture").unwrap();
    let request = request();
    let mut candidates = candidates();
    let mut literal_auto = candidates[0].clone();
    literal_auto.target = target("auto");
    literal_auto.capability.model = "auto".into();
    candidates.push(literal_auto);
    let baseline = exact.route(&request, &[], &candidates).unwrap();
    assert_eq!(fixture.route(&exact, &request, &[], &candidates), baseline);
    assert_eq!(baseline.chosen, target("auto"));
    assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 0);
    let decision = fixture.route(&dynamic, &request, &[], &candidates);
    assert_eq!(decision.chosen, target("high"));
    assert_eq!(decision.decided_by, DecidedBy::Classifier);
    assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 1);
}

#[test]
fn applied_cloud_classification_keeps_the_host_schema_receipt_estimate() {
    let fixture = Fixture::new(true);
    let router = Router::new(config()).unwrap();
    let mut request = request();
    request.tools.push(ToolDef {
        name: "inspect".into(),
        description: Some("Inspect a document".into()),
        parameters: json!({"description":"schema ".repeat(2000)}),
    });
    request.response_format = Some(token_station_protocol::ResponseFormat::JsonSchema {
        json_schema: json!({"description":"output ".repeat(1000)}),
    });
    let candidates = candidates();
    let baseline = fixture
        .gateway
        .route_with_mode(&router, &request, &[], &candidates, "fixture")
        .unwrap();
    assert!(baseline.features.estimated_input_tokens > 1000);
    let decision = fixture.route(&router, &request, &[], &candidates);
    assert_eq!(decision.decided_by, DecidedBy::Classifier);
    assert_eq!(decision.features, baseline.features);
}

#[test]
fn jev_explicit_pins_rules_hints_and_non_tier_modes_have_no_egress() {
    let fixture = Fixture::new(true);
    for mode in ["pin", "rule", "hint", "direct", "quota"] {
        let mut config = config();
        let mut request = request();
        let mut hints = Vec::new();
        match mode {
            "pin" => {
                config.honor_exact_model = true;
                request.model = "medium".into();
            }
            "rule" => config.rules.push(Rule {
                id: "explain".into(),
                matcher: Match {
                    keywords_any: vec!["function".into()],
                    ..Match::default()
                },
                route_to: "tier_mid".into(),
            }),
            "hint" => {
                config.hint_routes.push(HintRoute {
                    kind: HintKind::StepType,
                    value: "explain".into(),
                    route_to: "tier_mid".into(),
                });
                hints.push(AgentHint {
                    kind: HintKind::StepType,
                    value: "explain".into(),
                    extensions: BTreeMap::new(),
                });
            }
            "direct" => {
                config.pools.remove("tier_mid");
            }
            _ => {
                config.routing_mode = RoutingMode::QuotaFirst;
                config.quota_accounts = vec![target("low")];
            }
        }
        let router = Router::new(config).unwrap();
        let expected = fixture
            .gateway
            .route_with_mode(&router, &request, &hints, &candidates(), "test")
            .unwrap();
        assert_eq!(
            fixture.route(&router, &request, &hints, &candidates()),
            expected,
            "{mode}"
        );
    }
    assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 0);
}

#[test]
fn jev_local_only_never_sends_even_when_cloud_fallback_is_allowed() {
    let fixture = Fixture::new(true);
    let mut config = config();
    config.local_only = true;
    config.allow_cloud_fallback = true;
    let router = Router::new(config).unwrap();
    let candidates = candidates()
        .into_iter()
        .map(|candidate| candidate.local(true))
        .collect::<Vec<_>>();
    assert_eq!(
        fixture.route(&router, &request(), &[], &candidates),
        router.route(&request(), &[], &candidates).unwrap()
    );
    assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 0);
}

#[test]
fn jev_unusable_tiers_keep_the_complete_baseline() {
    let fixture = Fixture::new(true);
    let router = Router::new(config()).unwrap();
    for constraint in ["missing", "tool", "schema"] {
        let mut request = request();
        let mut candidates = candidates();
        match constraint {
            "missing" => {
                candidates.pop();
            }
            "tool" => {
                candidates[2].capability.tool = false;
                request.tools.push(ToolDef {
                    name: "read".into(),
                    description: None,
                    parameters: json!({}),
                });
            }
            _ => {
                candidates[2].capability.json_schema = false;
                request.response_format = Some(ResponseFormat::JsonObject);
            }
        }
        assert_eq!(
            fixture.route(&router, &request, &[], &candidates),
            fixture
                .gateway
                .route_with_mode(&router, &request, &[], &candidates, "test")
                .unwrap(),
            "{constraint}"
        );
    }
    assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 3);
}

#[test]
fn jev_preserves_core_health_and_context_ranking_within_the_suggested_tier() {
    let fixture = Fixture::new(true);
    let mut config = config();
    config
        .pools
        .get_mut("tier_high")
        .unwrap()
        .push(target("healthy-fitting"));
    let router = Router::new(config).unwrap();
    for constraint in ["health", "context"] {
        let mut candidates = candidates();
        let mut alternative = candidates[2].clone();
        alternative.target = target("healthy-fitting");
        candidates.push(alternative);
        if constraint == "health" {
            candidates[2].health = Health::Unavailable;
        } else {
            candidates[2].capability.context_window = 1;
        }
        let decision = fixture.route(&router, &request(), &[], &candidates);
        assert_eq!(decision.chosen, target("healthy-fitting"));
        assert_eq!(
            decision,
            router
                .route_with_classifier_pool(&request(), &[], &candidates, Some("tier_high"))
                .unwrap()
        );
    }
}

#[test]
fn jev_remote_failure_keeps_the_complete_baseline() {
    let fixture = Fixture::with_response(true, 503, json!({"error":"synthetic unavailable"}));
    let router = Router::new(config()).unwrap();
    assert_eq!(
        fixture.route(&router, &request(), &[], &candidates()),
        router.route(&request(), &[], &candidates()).unwrap()
    );
    assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 1);
}

#[test]
fn jev_free_tier_never_adds_paid_recovery_targets() {
    let mut fixture = Fixture::new(true);
    fixture.gateway.free_upstreams.insert("fixture".into());
    let paid = UpstreamModel::new(UpstreamRef::new("paid").unwrap(), "low");
    let mut config = config();
    config.pools.insert("tier_low".into(), vec![paid.clone()]);
    config.recovery = token_station_router_core::RecoveryPolicy::Ordered {
        pools: vec!["tier_low".into()],
    };
    let router = Router::new(config).unwrap();
    let mut candidates = candidates();
    candidates[0].target = paid;
    let decision = fixture.route(&router, &request(), &[], &candidates);
    assert_eq!(decision.chosen, target("high"));
    assert!(decision.fallbacks.is_empty());
}

#[test]
fn jev_native_projection_preserves_body_and_routing_requirements() {
    for (dialect, body) in [
        (
            ApiDialect::AnthropicNative,
            json!({"model":"auto","stream":true,"max_tokens":2048,"system":"private instruction","messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}],"tools":[{"type":"web_search_20250305","name":"web_search"}]}),
        ),
        (
            ApiDialect::ResponsesNative,
            json!({"model":"auto","stream":true,"max_output_tokens":2048,"instructions":"private instruction","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}],"tools":[{"type":"web_search"}],"text":{"format":{"type":"json_schema","schema":{"type":"object"}}}}),
        ),
    ] {
        let original = body.clone();
        let projected = jev_routing::native_request(&body, dialect, "auto").unwrap();
        assert_eq!(body, original);
        assert!(projected.stream);
        assert_eq!(projected.sampling.max_output_tokens, Some(2048));
        assert_eq!(projected.tools.len(), 1);
        assert_eq!(projected.messages[0].role, Role::System);
        assert_eq!(
            projected.messages.last().unwrap(),
            &Message::text(Role::User, "hello")
        );
        if dialect == ApiDialect::ResponsesNative {
            assert!(matches!(
                projected.response_format,
                Some(ResponseFormat::JsonSchema { .. })
            ));
        }
    }
}

#[test]
fn jev_native_opaque_or_multimodal_shapes_are_explicitly_unsupported() {
    for body in [
        json!({"input":"hello","previous_response_id":"resp_private"}),
        json!({"input":"hello","conversation":"conv_private"}),
        json!({"input":[{"type":"function_call_output","output":"private result","call_id":"x"}]}),
        json!({"input":[{"role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,eA=="}]}]}),
    ] {
        assert!(jev_routing::native_request(&body, ApiDialect::ResponsesNative, "auto").is_none());
    }
    for content in [
        json!([{"type":"document","source":{"type":"text","data":"private"}}]),
        json!([{"type":"tool_result","tool_use_id":"x","content":"private result"}]),
        json!([{"type":"thinking","thinking":"private reasoning"}]),
    ] {
        assert!(
            jev_routing::native_request(
                &json!({"messages":[{"role":"user","content":content}]}),
                ApiDialect::AnthropicNative,
                "auto"
            )
            .is_none()
        );
    }
}

#[cfg(feature = "builtin-plugins")]
mod native_transport {
    #[allow(clippy::wildcard_imports)]
    use super::*;

    #[derive(Default)]
    struct RecordedRequests(std::sync::Mutex<Vec<Value>>);

    impl token_station_metrics::Recorder for RecordedRequests {
        fn record(&self, record: &token_station_metrics::RequestRecord) {
            self.0
                .lock()
                .unwrap()
                .push(serde_json::to_value(record).unwrap());
        }
    }

    fn long_messages(skipped: bool) -> Value {
        json!([
            {"role":"user", "content":"INITIAL CONSTRAINT: preserve all original data"},
            {"role":"assistant", "content":"OMITTED HISTORY ".repeat(2048)},
            {"role":"user", "content":if skipped {"LATEST QUESTION ".repeat(2048)} else {"LATEST QUESTION: explain the invariant".into()}}
        ])
    }

    fn assert_input_record(records: &RecordedRequests, skipped: bool) {
        let rows = records.0.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["status"], 200);
        assert_eq!(rows[0]["error_code"], Value::Null);
        assert_eq!(
            rows[0]["classifier_input"],
            json!({
                "classifier":"jev", "handling":if skipped {"skipped"} else {"reduced"}, "reason":"byte_limit"
            })
        );
        let encoded = rows[0]["classifier_input"].to_string();
        assert!(!encoded.contains("LATEST QUESTION"));
        assert!(!encoded.contains("synthetic-provider-key"));
    }

    fn gateway(
        fixture: &Fixture,
        low: &Endpoint,
        high: &Endpoint,
        dialect: &str,
        high_dialect: &str,
    ) -> Gateway {
        let key = fixture.data.join("provider-key");
        std::fs::write(&key, "synthetic-provider-key").unwrap();
        let upstream = |endpoint: &Endpoint, wire: &str, model: &str| {
            json!({
                "provider": if wire=="anthropic-native" {"anthropic"} else {"openai-compatible"},
                "api_dialect":wire,"base_url":endpoint.url.trim_end_matches("/v1/systemone"),
                "auth":{"slot":"provider_api_key","file":key},
                "models":[{"model":model,"tool":true,"vision":true,"json_schema":true,"context_window":128_000}]
            })
        };
        let config:ClientConfig=serde_json::from_value(json!({
            "version":1,"server":{"listen":"127.0.0.1:0"},"data":{"dir":fixture.data,"metrics":false},
            "plugins":{"dir":fixture.data.join("unused-plugins"),"agents":["agent-anthropic","agent-openai-responses","agent-openai"],
                "providers":{"anthropic":"provider-anthropic-v2","openai-compatible":"provider-openai-compatible-v2"}},
            "upstreams":{"low":upstream(low,dialect,"low"),"high":upstream(high,high_dialect,"high")},
            "router":{"version":1,"pools":{"tier_low":[{"upstream":"low","model":"low"}],
                "tier_mid":[{"upstream":"low","model":"low"}],"tier_high":[{"upstream":"high","model":"high"}]},
                "default_pool":"tier_low"}
        })).unwrap();
        Gateway::new(&config, Arc::new(token_station_metrics::NoopRecorder)).unwrap()
    }

    fn body(dialect: &str, stream: bool) -> Value {
        if dialect == "anthropic-native" {
            json!({"model":"auto","stream":stream,"max_tokens":2048,"system":"synthetic-private-system",
                "messages":[{"role":"user","content":[{"type":"text","text":"Explain this function"}]}],
                "tools":[{"type":"web_search_20250305","name":"web_search"}],
                "metadata":{"user_id":"synthetic-private-user"}})
        } else {
            json!({"model":"auto","stream":stream,"max_output_tokens":2048,"instructions":"synthetic-private-system",
                "input":[{"role":"user","content":[{"type":"input_text","text":"Explain this function"}]}],
                "tools":[{"type":"web_search"}],"include":["web_search_call.action.sources"],
                "metadata":{"user_id":"synthetic-private-user"}})
        }
    }

    fn answer(dialect: &str, stream: bool) -> Endpoint {
        let response = if dialect == "anthropic-native" {
            json!({"id":"msg_fixture","type":"message","role":"assistant","model":"high","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":1}})
        } else {
            json!({"id":"resp_fixture","object":"response","status":"completed","model":"high","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok","annotations":[]}]}],"usage":{"input_tokens":5,"output_tokens":1,"total_tokens":6}})
        };
        if !stream {
            return Endpoint::new(200, response);
        }
        let wire = if dialect == "anthropic-native" {
            format!(
                "event: message_start\ndata: {}\n\nevent: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":1}}}}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n",
                json!({"type":"message_start","message":response})
            )
        } else {
            format!(
                "event: response.completed\ndata: {}\n\n",
                json!({"type":"response.completed","response":response})
            )
        };
        Endpoint::with_body(200, "text/event-stream", wire)
    }

    fn send(gateway: &Gateway, dialect: &str, body: &Value) {
        let mut status = None;
        gateway.chat(
            "POST",
            if dialect == "anthropic-native" {
                "/v1/messages"
            } else {
                "/v1/responses"
            },
            &[],
            body.to_string().as_bytes(),
            &mut |reply| {
                match reply {
                    Reply::BeginJson(reply) => status = Some(reply.status),
                    Reply::BeginStream => status = Some(200),
                    Reply::Chunk(_) => {}
                }
                true
            },
        );
        assert_eq!(status, Some(200));
    }

    fn assert_payload(endpoint: &Endpoint, mut expected: Value, model: &str, dialect: &str) {
        let seen = endpoint.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let (headers, body) = seen[0].split_once("\r\n\r\n").unwrap();
        assert!(
            headers
                .lines()
                .next()
                .unwrap()
                .contains(if dialect == "anthropic-native" {
                    "/messages "
                } else {
                    "/responses "
                })
        );
        expected["model"] = json!(model);
        assert_eq!(serde_json::from_str::<Value>(body).unwrap(), expected);
    }

    #[test]
    fn jev_native_text_and_hosted_tools_preserve_protocol_payload_and_stream() {
        for dialect in ["anthropic-native", "responses-native"] {
            for stream in [false, true] {
                let fixture = Fixture::new(true);
                let low = answer(dialect, stream);
                let high = answer(dialect, stream);
                let gateway = gateway(&fixture, &low, &high, dialect, dialect);
                let body = body(dialect, stream);
                send(&gateway, dialect, &body);
                assert_payload(&high, body, "high", dialect);
                assert_eq!(low.hits.load(Ordering::SeqCst), 0);
                assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 1);
                let classified = fixture.endpoint.seen.lock().unwrap();
                assert!(!classified[0].contains("synthetic-private-system"));
                assert!(!classified[0].contains("synthetic-private-user"));
                assert!(!classified[0].contains("synthetic-provider-key"));
            }
        }
    }

    #[test]
    fn jev_native_input_limits_only_change_the_classifier_copy_and_record_neutral_diagnostics() {
        for dialect in ["anthropic-native", "responses-native"] {
            for skipped in [false, true] {
                for stream in [false, true] {
                    let fixture = Fixture::new(true);
                    let low = answer(dialect, stream);
                    let high = answer(dialect, stream);
                    let mut gateway = gateway(&fixture, &low, &high, dialect, dialect);
                    let records = Arc::new(RecordedRequests::default());
                    gateway.recorder = records.clone();
                    let mut body = body(dialect, stream);
                    body[if dialect == "anthropic-native" {
                        "messages"
                    } else {
                        "input"
                    }] = long_messages(skipped);
                    send(&gateway, dialect, &body);
                    if skipped {
                        assert_payload(&low, body, "low", dialect);
                        assert_eq!(high.hits.load(Ordering::SeqCst), 0);
                        assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 0);
                    } else {
                        assert_payload(&high, body, "high", dialect);
                        assert_eq!(low.hits.load(Ordering::SeqCst), 0);
                        assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 1);
                        let seen = fixture.endpoint.seen.lock().unwrap();
                        assert!(seen[0].contains("INITIAL CONSTRAINT"));
                        assert!(seen[0].contains("LATEST QUESTION"));
                        assert!(!seen[0].contains("OMITTED HISTORY"));
                        assert!(!seen[0].contains("synthetic-private-system"));
                    }
                    assert_input_record(&records, skipped);
                }
            }
        }
    }

    #[test]
    fn jev_canonical_input_limits_preserve_the_complete_generation_messages() {
        for skipped in [false, true] {
            let fixture = Fixture::new(true);
            let reply = json!({"id":"chat_fixture","object":"chat.completion","model":"selected",
                "choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":5,"completion_tokens":1,"total_tokens":6}});
            let low = Endpoint::new(200, reply.clone());
            let high = Endpoint::new(200, reply);
            let mut gateway = gateway(&fixture, &low, &high, "translated", "translated");
            let records = Arc::new(RecordedRequests::default());
            gateway.recorder = records.clone();
            let messages = long_messages(skipped);
            let body = json!({"model":"auto", "messages":messages, "stream":false});
            let mut status = None;
            gateway.chat(
                "POST",
                "/v1/chat/completions",
                &[],
                body.to_string().as_bytes(),
                &mut |reply| {
                    if let Reply::BeginJson(reply) = reply {
                        status = Some(reply.status);
                    }
                    true
                },
            );
            assert_eq!(status, Some(200));
            let served = if skipped { &low } else { &high };
            let seen = served.seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            let (_, wire) = seen[0].split_once("\r\n\r\n").unwrap();
            let upstream: Value = serde_json::from_str(wire).unwrap();
            assert_eq!(upstream["messages"], messages);
            assert_eq!(
                fixture.endpoint.hits.load(Ordering::SeqCst),
                u64::from(!skipped)
            );
            assert_input_record(&records, skipped);
        }
    }

    #[test]
    fn jev_native_rejects_cross_protocol_tiers_and_preserves_baseline() {
        for dialect in ["anthropic-native", "responses-native"] {
            let fixture = Fixture::new(true);
            let low = answer(dialect, false);
            let high = answer(dialect, false);
            let gateway = gateway(&fixture, &low, &high, dialect, "translated");
            let body = body(dialect, false);
            send(&gateway, dialect, &body);
            assert_payload(&low, body, "low", dialect);
            assert_eq!(high.hits.load(Ordering::SeqCst), 0);
            assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn jev_native_opaque_history_keeps_generation_body_and_does_not_classify() {
        for dialect in ["anthropic-native", "responses-native"] {
            let fixture = Fixture::new(true);
            let low = answer(dialect, false);
            let high = answer(dialect, false);
            let gateway = gateway(&fixture, &low, &high, dialect, dialect);
            let mut body = body(dialect, false);
            if dialect == "anthropic-native" {
                body["messages"][0]["content"].as_array_mut().unwrap().push(json!({"type":"tool_result","tool_use_id":"call_fixture","content":"synthetic-private-tool-result"}));
            } else {
                body["previous_response_id"] = json!("resp_private");
            }
            send(&gateway, dialect, &body);
            assert_payload(&low, body, "low", dialect);
            assert_eq!(high.hits.load(Ordering::SeqCst), 0);
            assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn jev_declined_anthropic_native_probe_classifies_only_once_in_canonical_path() {
        let fixture = Fixture::new(true);
        let low = answer("anthropic-native", false);
        let high = answer("anthropic-native", false);
        let gateway = gateway(
            &fixture,
            &low,
            &high,
            "anthropic-native",
            "anthropic-native",
        );
        let mut body = body("anthropic-native", false);
        body.as_object_mut().unwrap().remove("tools");
        send(&gateway, "anthropic-native", &body);
        assert_eq!(high.hits.load(Ordering::SeqCst), 1);
        assert_eq!(low.hits.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn jev_native_failures_keep_the_off_mode_route_for_long_text() {
        for dialect in ["anthropic-native", "responses-native"] {
            for failure in ["unauthorized", "timeout", "local_only"] {
                let mut fixture =
                    Fixture::with_response(false, 401, json!({"error":"synthetic rejection"}));
                if failure == "timeout" {
                    fixture.endpoint = Endpoint::with_delay(
                        401,
                        "application/json",
                        "{}".into(),
                        Duration::from_millis(1700),
                    );
                    JevController::for_test(&fixture.data, &fixture.endpoint.url);
                }
                let low = answer(dialect, false);
                let high = answer(dialect, false);
                let mut gateway = gateway(&fixture, &low, &high, dialect, dialect);
                let mut config = gateway.home_router.as_ref().unwrap().config().clone();
                config.heuristic = Some(token_station_router_core::Heuristic {
                    weights: serde_json::from_value(json!({"tokens_per_point":1,"per_tool":0,"json_schema":0,"image":0,"per_code_block":0,"per_extra_turn":0})).unwrap(),
                    threshold: 10,
                    above: "tier_high".into(),
                    below: "tier_low".into(),
                    bands: Vec::new(),
                });
                config.local_only = failure == "local_only";
                gateway
                    .local_upstreams
                    .extend(["low".into(), "high".into()]);
                gateway.home_router = Some(Arc::new(Router::new(config).unwrap()));
                let mut body = body(dialect, false);
                let content = if dialect == "anthropic-native" {
                    &mut body["messages"][0]["content"][0]["text"]
                } else {
                    &mut body["input"][0]["content"][0]["text"]
                };
                *content = json!("long native conversation ".repeat(512));
                send(&gateway, dialect, &body);
                assert_eq!(low.hits.load(Ordering::SeqCst), 1);
                assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 0);
                fixture.controller.set_enabled(true).unwrap();
                send(&gateway, dialect, &body);
                assert_eq!(
                    low.hits.load(Ordering::SeqCst),
                    2,
                    "{dialect} {failure} changed the baseline"
                );
                assert_eq!(high.hits.load(Ordering::SeqCst), 0);
                assert_eq!(
                    fixture.endpoint.hits.load(Ordering::SeqCst),
                    u64::from(failure != "local_only")
                );
                assert_eq!(
                    fixture.controller.status().last_outcome,
                    Some(match failure {
                        "unauthorized" => crate::jev::Outcome::Unauthorized,
                        "timeout" => crate::jev::Outcome::Timeout,
                        _ => crate::jev::Outcome::LocalOnly,
                    })
                );
            }
        }
    }

    #[test]
    fn jev_native_preserves_rules_that_match_the_original_probe() {
        for dialect in ["anthropic-native", "responses-native"] {
            let fixture = Fixture::new(true);
            let low = answer(dialect, false);
            let high = answer(dialect, false);
            let mut gateway = gateway(&fixture, &low, &high, dialect, dialect);
            let mut config = gateway.home_router.as_ref().unwrap().config().clone();
            config.rules.push(Rule {
                id: "original-probe-rule".into(),
                matcher: Match {
                    estimated_input_tokens_below: Some(30),
                    ..Match::default()
                },
                route_to: "tier_low".into(),
            });
            gateway.home_router = Some(Arc::new(Router::new(config).unwrap()));
            let mut body = body(dialect, false);
            if dialect == "anthropic-native" {
                body["messages"][0]["content"][0]["text"] = json!("native text ".repeat(128));
            } else {
                body["input"][0]["content"][0]["text"] = json!("native text ".repeat(128));
            }
            send(&gateway, dialect, &body);
            assert_payload(&low, body, "low", dialect);
            assert_eq!(high.hits.load(Ordering::SeqCst), 0);
            assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 0);
            assert_eq!(
                fixture.controller.status().last_outcome,
                Some(crate::jev::Outcome::Overridden)
            );
        }
    }

    #[test]
    fn jev_native_rerouting_checks_the_full_projection_schema_and_context() {
        for dialect in ["anthropic-native", "responses-native"] {
            for requirement in ["schema", "context"] {
                let fixture = Fixture::new(true);
                let low = answer(dialect, false);
                let high = answer(dialect, false);
                let mut gateway = gateway(&fixture, &low, &high, dialect, dialect);
                let (_, capability) = gateway
                    .catalog
                    .iter_mut()
                    .find(|(target, _)| target.upstream.as_str() == "high")
                    .unwrap();
                let mut body = body(dialect, false);
                if requirement == "schema" {
                    capability.json_schema = false;
                    capability.json_schema_state = Some(CapabilityState::Unsupported);
                    let schema =
                        json!({"format":{"type":"json_schema","schema":{"type":"object"}}});
                    if dialect == "anthropic-native" {
                        body["output_config"] = schema;
                    } else {
                        body["text"] = schema;
                    }
                } else {
                    capability.context_window = 1;
                    let mut config = gateway.home_router.as_ref().unwrap().config().clone();
                    let low_target = config.pools["tier_low"][0].clone();
                    config.pools.get_mut("tier_high").unwrap().push(low_target);
                    gateway.home_router = Some(Arc::new(Router::new(config).unwrap()));
                }
                send(&gateway, dialect, &body);
                assert_payload(&low, body, "low", dialect);
                assert_eq!(high.hits.load(Ordering::SeqCst), 0);
                assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 1);
                assert_eq!(
                    fixture.controller.status().last_outcome,
                    Some(if requirement == "schema" {
                        crate::jev::Outcome::NoRoute
                    } else {
                        crate::jev::Outcome::Applied
                    })
                );
            }
        }
    }
}

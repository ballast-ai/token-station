use super::*;
use serde_json::{Value, json};
use std::io::Write;
use std::net::TcpListener;
use std::sync::atomic::AtomicU64;
use token_station_protocol::Message;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static IDS: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "token-station-jev-{}-{}",
            std::process::id(),
            IDS.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn request() -> ChatRequest {
    ChatRequest::new("auto", vec![Message::text(Role::User, "Explain sorting")])
}
fn context() -> RequestContext {
    RequestContext::detached(Duration::from_secs(5), Duration::from_secs(5))
}
fn prediction() -> Value {
    json!({
        "model":"jev-latest", "usage":{"input_tokens":12,"output_tokens":1},
        "answers":{"tier":{"type":"choice","choice":"high","confidence":0.8,
            "probabilities":{"low":0.1,"medium":0.1,"high":0.8}}}
    })
}

fn fixture(status: u16, body: String, delay: Duration) -> (String, mpsc::Receiver<String>) {
    fixture_headers(status, body, delay, String::new())
}

fn fixture_headers(
    status: u16,
    body: String,
    delay: Duration,
    headers: String,
) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut tunnel = String::new();
        let mut chunk = [0; 4096];
        loop {
            let count = socket.read(&mut chunk).unwrap();
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(end) = bytes.windows(4).position(|slice| slice == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&bytes[..end]);
                if head.starts_with("CONNECT ") {
                    tunnel = format!("{head}\r\n\r\n");
                    socket
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .unwrap();
                    bytes.clear();
                    continue;
                }
                let length: usize = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse().ok())
                    })
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let _ = sender.send(format!("{tunnel}{}", String::from_utf8(bytes).unwrap()));
        std::thread::sleep(delay);
        let response = format!(
            "HTTP/1.1 {status} Test\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(response.as_bytes());
    });
    (endpoint, receiver)
}

fn configured(dir: &Scratch, endpoint: &str) -> Arc<JevController> {
    let controller = JevController::for_test(&dir.0, endpoint);
    controller.save_key("synthetic-key").unwrap();
    controller.set_enabled(true).unwrap();
    controller
}

#[test]
fn defaults_are_disabled_and_shared_without_disclosing_credentials() {
    let dir = Scratch::new();
    let controller = JevController::shared(&dir.0);
    assert!(Arc::ptr_eq(&controller, &JevController::shared(&dir.0)));
    assert!(!controller.status().enabled);
    assert!(!controller.status().has_key);
    assert!(controller.set_enabled(true).is_err());
    assert!(controller.save_key("\r\nAuthorization: secret").is_err());
    controller.save_key("synthetic-key").unwrap();
    let encoded = serde_json::to_string(&controller.status()).unwrap();
    assert!(!encoded.contains("synthetic-key"));
    assert!(controller.status().has_key);
    assert!(!controller.status().enabled);
}

#[test]
fn settings_persist_and_clear_preserves_unrelated_credentials() {
    let dir = Scratch::new();
    crate::secrets::store_set(&dir.0, "provider", "provider_api_key", "other-key").unwrap();
    let controller = JevController::shared(&dir.0);
    controller.save_key("synthetic-key").unwrap();
    controller.set_enabled(true).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in ["jev-settings.json", "secrets.json"] {
            assert_eq!(
                std::fs::metadata(dir.0.join(path))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    drop(controller);
    let controller = JevController::shared(&dir.0);
    assert!(controller.status().enabled);
    assert!(controller.clear_key().unwrap().last_outcome == Some(Outcome::Disabled));
    assert!(!controller.status().enabled);
    assert!(!controller.status().has_key);
    assert_eq!(
        crate::secrets::store_get(&dir.0, "provider", "provider_api_key").unwrap(),
        "other-key"
    );
    drop(controller);
    assert!(!JevController::shared(&dir.0).status().enabled);
}

#[test]
fn invalid_settings_and_corrupt_secrets_fail_closed_and_errors_stay_static() {
    let dir = Scratch::new();
    std::fs::write(dir.0.join("jev-settings.json"), b"private malformed input").unwrap();
    let controller = JevController::shared(&dir.0);
    assert!(!controller.status().enabled);
    assert_eq!(controller.status().last_outcome, Some(Outcome::Invalid));
    controller.save_key("synthetic-key").unwrap();
    assert!(controller.set_enabled(true).unwrap().enabled);
    std::fs::write(dir.0.join("secrets.json"), b"private broken secret").unwrap();
    let error = controller.save_key("replacement").unwrap_err();
    assert!(!error.contains("private"));
    assert!(!error.contains("replacement"));
    assert_eq!(
        std::fs::read(dir.0.join("secrets.json")).unwrap(),
        b"private broken secret"
    );
    assert!(
        controller
            .classify(&request(), &context(), &EgressConfig::default())
            .is_none()
    );
    assert_eq!(controller.status().last_outcome, Some(Outcome::MissingKey));
}

#[test]
fn valid_classification_uses_fixed_contract_and_only_projects_allowed_text() {
    let dir = Scratch::new();
    let (endpoint, captured) = fixture(200, prediction().to_string(), Duration::ZERO);
    let controller = configured(&dir, &endpoint);
    let mut input = request();
    input.messages = serde_json::from_value(json!([
        {"role":"system","content":"PRIVATE SYSTEM"},
        {"role":"user","content":"Explain sorting"},
        {"role":"assistant","content":[{"type":"thinking","thinking":"PRIVATE THOUGHT"},{"type":"text","text":"Compare algorithms"}]},
        {"role":"tool","content":"PRIVATE TOOL","tool_call_id":"a"},
        {"role":"user","content":"Continue"}
    ])).unwrap();
    input.tools =
        serde_json::from_value(json!([{"name":"private_function","parameters":{}}])).unwrap();
    let suggestion = controller
        .classify(&input, &context(), &EgressConfig::default())
        .unwrap();
    assert_eq!(suggestion.tier, Tier::High);
    assert!(controller.is_current(&suggestion));
    controller.finish(suggestion, true);
    assert_eq!(controller.status().last_outcome, Some(Outcome::Applied));
    let wire = captured.recv().unwrap();
    assert!(wire.starts_with("POST /v1/systemone HTTP/1.1"));
    assert!(
        wire.to_ascii_lowercase()
            .contains("authorization: bearer synthetic-key")
    );
    let payload: Value = serde_json::from_str(wire.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(payload["model"], "jev-latest");
    assert_eq!(payload["questions"]["tier"]["type"], "choice");
    assert_eq!(
        payload["questions"]["tier"]["criteria"]
            .as_object()
            .unwrap()
            .len(),
        3
    );
    assert!(payload["state"].as_str().unwrap().contains("Continue"));
    assert!(!wire.contains("PRIVATE"));
    assert!(!wire.contains("private_function"));
    assert!(
        !serde_json::to_string(&controller.status())
            .unwrap()
            .contains("sorting")
    );
}

#[test]
fn disabled_local_only_unsupported_and_cancelled_requests_do_not_connect() {
    let dir = Scratch::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
    let controller = JevController::for_test(&dir.0, &endpoint);
    assert!(
        controller
            .classify(&request(), &context(), &EgressConfig::default())
            .is_none()
    );
    controller.save_key("synthetic-key").unwrap();
    controller.set_enabled(true).unwrap();
    let mut local = request();
    local.extensions.insert("local_only".into(), json!(true));
    assert!(
        controller
            .classify(&local, &context(), &EgressConfig::default())
            .is_none()
    );
    assert_eq!(controller.status().last_outcome, Some(Outcome::LocalOnly));
    let mut image = request();
    image.messages[0].content = Some(Content::Parts(vec![ContentPart::Unknown(
        json!({"type":"image","data":"private"}),
    )]));
    assert!(
        controller
            .classify(&image, &context(), &EgressConfig::default())
            .is_none()
    );
    assert_eq!(controller.status().last_outcome, Some(Outcome::Unsupported));
    let cancelled = context();
    cancelled.cancel();
    assert!(
        controller
            .classify(&request(), &cancelled, &EgressConfig::default())
            .is_none()
    );
    assert_eq!(controller.status().last_outcome, Some(Outcome::Cancelled));
    assert!(listener.accept().is_err());
}

fn input_diagnostic(ctx: &RequestContext) -> Value {
    serde_json::to_value(ctx.classifier_input()).unwrap()
}

#[test]
fn short_projection_is_unchanged_and_reports_only_closed_input_fields() {
    let mut input = request();
    input.messages = serde_json::from_value(json!([
        {"role":"system","content":"PRIVATE SYSTEM"},
        {"role":"user","content":"  Earlier task  "},
        {"role":"assistant","content":[{"type":"text","text":" first "},{"type":"thinking","thinking":"PRIVATE THOUGHT"},{"type":"text","text":" second "}]},
        {"role":"tool","content":"PRIVATE TOOL"},
        {"role":"user","content":" Latest task "}
    ])).unwrap();
    let before = input.clone();
    let ctx = context();
    assert_eq!(
        project(&input, &ctx).unwrap(),
        "User: Earlier task\n\nAssistant: first\nsecond\n\nUser: Latest task"
    );
    assert_eq!(input, before);
    assert_eq!(
        input_diagnostic(&ctx),
        json!({"classifier":"jev","handling":"full","reason":null})
    );
}

#[test]
fn oversized_history_reserves_complete_latest_user_anchor_and_recent_turns() {
    let mut input = request();
    input.messages = vec![
        Message::text(Role::User, "INITIAL CONSTRAINT"),
        Message::text(Role::Assistant, "old".repeat(MAX_TEXT_BYTES)),
        Message::text(Role::User, "RECENT CONTEXT"),
        Message::text(Role::Assistant, "RECENT ANSWER"),
        Message::text(Role::User, "完整最新任务🙂"),
        Message::text(Role::Assistant, "trailing".repeat(MAX_TEXT_BYTES)),
    ];
    let before = input.clone();
    let ctx = context();
    let projected = project(&input, &ctx).unwrap();
    assert_eq!(
        projected,
        "User: INITIAL CONSTRAINT\n\nUser: RECENT CONTEXT\n\nAssistant: RECENT ANSWER\n\nUser: 完整最新任务🙂"
    );
    assert!(projected.len() <= MAX_TEXT_BYTES);
    assert_eq!(input, before);
    assert_eq!(
        input_diagnostic(&ctx),
        json!({"classifier":"jev","handling":"reduced","reason":"byte_limit"})
    );
}

#[test]
fn latest_user_must_fit_completely_at_the_exact_rendered_byte_boundary() {
    let mut input = request();
    let text = format!(
        "{}中",
        "x".repeat(MAX_TEXT_BYTES - "User: ".len() - "中".len())
    );
    input.messages = vec![Message::text(Role::User, &text)];
    let ctx = context();
    assert_eq!(project(&input, &ctx).unwrap(), format!("User: {text}"));
    assert_eq!(input_diagnostic(&ctx)["handling"], "full");
    input.messages[0] = Message::text(Role::User, format!("{text}x"));
    let ctx = context();
    assert_eq!(project(&input, &ctx), Err(Outcome::Unsupported));
    assert_eq!(
        input_diagnostic(&ctx),
        json!({"classifier":"jev","handling":"skipped","reason":"byte_limit"})
    );
}

#[test]
fn latest_user_precedes_an_anchor_that_cannot_fit_and_keeps_complete_parts() {
    let mut input = request();
    input.messages = vec![
        Message::text(Role::User, "EARLIEST".repeat(MAX_TEXT_BYTES)),
        Message::text(Role::Assistant, "nearby"),
        Message {
            content: Some(Content::Parts(vec![
                ContentPart::Text {
                    text: "first".into(),
                },
                ContentPart::Thinking {
                    thinking: "PRIVATE".into(),
                    signature: None,
                },
                ContentPart::Text {
                    text: "second".into(),
                },
            ])),
            ..Message::text(Role::User, "")
        },
    ];
    let ctx = context();
    assert_eq!(
        project(&input, &ctx).unwrap(),
        "Assistant: nearby\n\nUser: first\nsecond"
    );
    assert_eq!(input_diagnostic(&ctx)["handling"], "reduced");
}

#[test]
fn history_selection_prefers_anchor_then_recent_complete_messages() {
    let anchor = format!("ANCHOR{}", "a".repeat(6000));
    let recent = format!("RECENT{}", "r".repeat(6000));
    let input = ChatRequest::new(
        "auto",
        vec![
            Message::text(Role::User, &anchor),
            Message::text(Role::Assistant, format!("OLDER{}", "o".repeat(6000))),
            Message::text(Role::Assistant, &recent),
            Message::text(Role::User, "LATEST"),
        ],
    );
    let ctx = context();
    assert_eq!(
        project(&input, &ctx).unwrap(),
        format!("User: {anchor}\n\nAssistant: {recent}\n\nUser: LATEST")
    );
    assert_eq!(input_diagnostic(&ctx)["handling"], "reduced");
}

#[test]
fn unsupported_history_and_empty_latest_user_are_not_reported_as_length_limits() {
    let mut input = request();
    input.messages = vec![
        Message::text(Role::User, "old".repeat(MAX_TEXT_BYTES)),
        Message::text(Role::User, " "),
    ];
    let ctx = context();
    assert_eq!(project(&input, &ctx), Err(Outcome::Unsupported));
    assert!(ctx.classifier_input().is_none());
    input.messages[1] = Message::text(Role::User, "current");
    input.messages[0].content = Some(Content::Parts(vec![ContentPart::Unknown(
        json!({"type":"image"}),
    )]));
    let ctx = context();
    assert_eq!(project(&input, &ctx), Err(Outcome::Unsupported));
    assert!(ctx.classifier_input().is_none());
    input.messages.clear();
    assert_eq!(project(&input, &context()), Err(Outcome::Unsupported));
}

#[test]
fn reduced_input_diagnostic_survives_network_failure_and_is_request_scoped() {
    let dir = Scratch::new();
    let (endpoint, captured) = fixture(401, "{}".into(), Duration::ZERO);
    let controller = configured(&dir, &endpoint);
    let mut input = request();
    input.messages.insert(
        0,
        Message::text(Role::Assistant, "PRIVATE OMITTED".repeat(MAX_TEXT_BYTES)),
    );
    let before = input.clone();
    let ctx = context();
    assert!(
        controller
            .classify(&input, &ctx, &EgressConfig::default())
            .is_none()
    );
    let wire = captured.recv().unwrap();
    assert!(!wire.contains("PRIVATE OMITTED"));
    assert_eq!(input, before);
    assert_eq!(
        controller.status().last_outcome,
        Some(Outcome::Unauthorized)
    );
    assert_eq!(
        input_diagnostic(&ctx),
        json!({"classifier":"jev","handling":"reduced","reason":"byte_limit"})
    );
    assert!(context().classifier_input().is_none());
}

#[test]
fn oversized_latest_user_does_not_open_a_connection() {
    let dir = Scratch::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
    let controller = configured(&dir, &endpoint);
    let input = ChatRequest::new(
        "auto",
        vec![Message::text(Role::User, "x".repeat(MAX_TEXT_BYTES))],
    );
    let before = input.clone();
    let ctx = context();
    assert!(
        controller
            .classify(&input, &ctx, &EgressConfig::default())
            .is_none()
    );
    assert!(listener.accept().is_err());
    assert_eq!(input, before);
    assert_eq!(input_diagnostic(&ctx)["handling"], "skipped");
}

#[test]
fn validation_rejects_unknown_tiers_bad_probabilities_and_low_confidence() {
    assert_eq!(parse_prediction(&prediction().to_string()), Ok(Tier::High));
    for (pointer, value, expected) in [
        ("/answers/tier/choice", json!("arbitrary"), Outcome::Invalid),
        (
            "/answers/tier/confidence",
            json!(0.69),
            Outcome::LowConfidence,
        ),
        ("/answers/tier/confidence", json!(1.1), Outcome::Invalid),
        (
            "/answers/tier/probabilities/high",
            json!(0.4),
            Outcome::Invalid,
        ),
        (
            "/answers/tier/probabilities/high",
            json!(-0.1),
            Outcome::Invalid,
        ),
        ("/answers/tier/type", json!("number"), Outcome::Invalid),
    ] {
        let mut value_json = prediction();
        *value_json.pointer_mut(pointer).unwrap() = value;
        assert_eq!(parse_prediction(&value_json.to_string()), Err(expected));
    }
    let mut extra = prediction();
    extra["answers"]["tier"]["probabilities"]["other"] = json!(0);
    assert_eq!(parse_prediction(&extra.to_string()), Err(Outcome::Invalid));
    let mut inconsistent = prediction();
    inconsistent["answers"]["tier"]["choice"] = json!("low");
    assert_eq!(
        parse_prediction(&inconsistent.to_string()),
        Err(Outcome::Invalid)
    );
    assert_eq!(
        parse_prediction("{\"private\":\"input\"}"),
        Err(Outcome::Invalid)
    );
}

#[test]
fn http_failures_and_response_limits_return_content_free_fallbacks() {
    for (status, body, outcome) in [
        (401, "PRIVATE BODY".into(), Outcome::Unauthorized),
        (403, "PRIVATE BODY".into(), Outcome::Unauthorized),
        (429, "PRIVATE BODY".into(), Outcome::RateLimited),
        (503, "PRIVATE BODY".into(), Outcome::Unavailable),
        (302, "PRIVATE BODY".into(), Outcome::Unavailable),
        (200, "PRIVATE BODY".into(), Outcome::Invalid),
        (200, " ".repeat(MAX_RESPONSE_BYTES + 1), Outcome::Invalid),
    ] {
        let dir = Scratch::new();
        let (endpoint, captured) = fixture(status, body, Duration::ZERO);
        let controller = configured(&dir, &endpoint);
        assert!(
            controller
                .classify(&request(), &context(), &EgressConfig::default())
                .is_none()
        );
        captured.recv().unwrap();
        assert_eq!(controller.status().last_outcome, Some(outcome));
        assert!(
            !serde_json::to_string(&controller.status())
                .unwrap()
                .contains("PRIVATE")
        );
    }
}

#[test]
fn connection_test_uses_synthetic_text_without_enabling_routing() {
    let dir = Scratch::new();
    let (endpoint, captured) = fixture(200, prediction().to_string(), Duration::ZERO);
    let controller = JevController::for_test(&dir.0, &endpoint);
    controller.save_key("synthetic-key").unwrap();
    let status = controller
        .test_connection(&EgressConfig::default())
        .unwrap();
    assert_eq!(status.last_outcome, Some(Outcome::Ready));
    assert!(!status.enabled);
    assert!(captured.recv().unwrap().contains("connection test"));
}

#[test]
fn context_deadline_cancellation_and_rotation_suppress_late_results() {
    let dir = Scratch::new();
    let (endpoint, captured) = fixture(200, prediction().to_string(), Duration::from_millis(300));
    let controller = configured(&dir, &endpoint);
    let ctx = RequestContext::detached(Duration::from_millis(40), Duration::from_secs(5));
    let started = Instant::now();
    assert!(
        controller
            .classify(&request(), &ctx, &EgressConfig::default())
            .is_none()
    );
    assert!(started.elapsed() < Duration::from_millis(250));
    assert_eq!(controller.status().last_outcome, Some(Outcome::Timeout));
    captured.recv().unwrap();

    for rotate in [false, true] {
        let dir = Scratch::new();
        let (endpoint, captured) =
            fixture(200, prediction().to_string(), Duration::from_millis(200));
        let controller = configured(&dir, &endpoint);
        let ctx = Arc::new(context());
        let worker_controller = Arc::clone(&controller);
        let worker_ctx = Arc::clone(&ctx);
        let worker = std::thread::spawn(move || {
            worker_controller.classify(&request(), &worker_ctx, &EgressConfig::default())
        });
        captured.recv().unwrap();
        if rotate {
            controller.save_key("new-key").unwrap();
        } else {
            ctx.cancel();
        }
        assert!(worker.join().unwrap().is_none());
        if !rotate {
            assert_eq!(controller.status().last_outcome, Some(Outcome::Cancelled));
        }
    }
}

#[test]
fn disable_and_key_removal_invalidate_completed_suggestions() {
    for remove in [false, true] {
        let dir = Scratch::new();
        let (endpoint, captured) = fixture(200, prediction().to_string(), Duration::ZERO);
        let controller = configured(&dir, &endpoint);
        let suggestion = controller
            .classify(&request(), &context(), &EgressConfig::default())
            .unwrap();
        captured.recv().unwrap();
        if remove {
            controller.clear_key().unwrap();
        } else {
            controller.set_enabled(false).unwrap();
        }
        assert!(!controller.is_current(&suggestion));
        controller.finish(suggestion, true);
        assert_eq!(controller.status().last_outcome, Some(Outcome::Disabled));
    }
}

#[test]
fn bounded_workers_report_busy_without_starting_more_network_calls() {
    let dir = Scratch::new();
    let controller = configured(&dir, "http://127.0.0.1:1/v1/systemone");
    controller.workers.store(MAX_WORKERS, Ordering::Release);
    assert!(
        controller
            .classify(&request(), &context(), &EgressConfig::default())
            .is_none()
    );
    assert_eq!(controller.status().last_outcome, Some(Outcome::Busy));
    controller.workers.store(0, Ordering::Release);
}

#[test]
fn concurrent_network_workers_are_bounded_until_the_http_work_finishes() {
    let dir = Scratch::new();
    let controller = JevController::shared(&dir.0);
    controller.save_key("synthetic-key").unwrap();
    controller.set_enabled(true).unwrap();
    let mut handles = Vec::new();
    for _ in 0..MAX_WORKERS {
        let (endpoint, captured) =
            fixture(200, prediction().to_string(), Duration::from_millis(900));
        JevController::for_test(&dir.0, &endpoint);
        let worker = Arc::clone(&controller);
        handles.push(std::thread::spawn(move || {
            worker.classify(&request(), &context(), &EgressConfig::default())
        }));
        captured.recv_timeout(Duration::from_secs(2)).unwrap();
    }
    assert!(
        controller
            .classify(&request(), &context(), &EgressConfig::default())
            .is_none()
    );
    assert_eq!(controller.status().last_outcome, Some(Outcome::Busy));
    for handle in handles {
        assert!(handle.join().unwrap().is_some());
    }
    assert_eq!(controller.workers.load(Ordering::Acquire), 0);
}

#[test]
fn global_network_timeout_is_bounded_even_without_a_context_deadline() {
    let dir = Scratch::new();
    let (endpoint, captured) = fixture(200, prediction().to_string(), Duration::from_millis(1800));
    let controller = configured(&dir, &endpoint);
    let started = Instant::now();
    assert!(
        controller
            .classify(&request(), &context(), &EgressConfig::default())
            .is_none()
    );
    assert!(started.elapsed() < Duration::from_millis(1750));
    captured.recv().unwrap();
    assert_eq!(controller.status().last_outcome, Some(Outcome::Timeout));
}

#[test]
fn redirects_never_forward_the_key_or_request_to_another_endpoint() {
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    for code in [301, 302, 307, 308] {
        let dir = Scratch::new();
        let (endpoint, captured) = fixture_headers(
            code,
            String::new(),
            Duration::ZERO,
            format!(
                "Location: http://{}/capture\r\n",
                target.local_addr().unwrap()
            ),
        );
        let controller = configured(&dir, &endpoint);
        assert!(
            controller
                .classify(&request(), &context(), &EgressConfig::default())
                .is_none()
        );
        captured.recv().unwrap();
        assert_eq!(controller.status().last_outcome, Some(Outcome::Unavailable));
        assert!(target.accept().is_err());
    }
}

#[test]
fn explicit_proxy_policy_and_no_proxy_are_respected() {
    let dir = Scratch::new();
    let destination = TcpListener::bind("127.0.0.1:0").unwrap();
    destination.set_nonblocking(true).unwrap();
    let destination_url = format!("http://{}/v1/systemone", destination.local_addr().unwrap());
    let (proxy_endpoint, captured) = fixture(200, prediction().to_string(), Duration::ZERO);
    let proxy_url = proxy_endpoint.trim_end_matches("/v1/systemone").to_owned();
    let controller = configured(&dir, &destination_url);
    let egress = EgressConfig {
        mode: crate::config::EgressMode::Http,
        proxy_url: Some(proxy_url),
        ..EgressConfig::default()
    };
    let suggestion = controller
        .classify(&request(), &context(), &egress)
        .unwrap();
    assert_eq!(suggestion.tier, Tier::High);
    let proxy_request = captured.recv().unwrap();
    assert!(
        proxy_request.starts_with(&format!("POST {destination_url} HTTP/1.1"))
            || (proxy_request.starts_with(&format!(
                "CONNECT {} HTTP/1.1",
                destination.local_addr().unwrap()
            )) && proxy_request.contains("POST /v1/systemone HTTP/1.1"))
    );
    assert!(destination.accept().is_err());

    let (endpoint, direct_request) = fixture(200, prediction().to_string(), Duration::ZERO);
    JevController::for_test(&dir.0, &endpoint);
    let bypass = EgressConfig {
        mode: crate::config::EgressMode::Http,
        proxy_url: Some("http://127.0.0.1:1".into()),
        no_proxy: vec!["127.0.0.1".into()],
        ..EgressConfig::default()
    };
    assert!(
        controller
            .classify(&request(), &context(), &bypass)
            .is_some()
    );
    assert!(
        direct_request
            .recv()
            .unwrap()
            .starts_with("POST /v1/systemone HTTP/1.1")
    );
}

#[test]
fn invalid_proxy_configuration_fails_without_direct_fallback() {
    let dir = Scratch::new();
    let destination = TcpListener::bind("127.0.0.1:0").unwrap();
    destination.set_nonblocking(true).unwrap();
    let controller = configured(
        &dir,
        &format!("http://{}/v1/systemone", destination.local_addr().unwrap()),
    );
    let egress = EgressConfig {
        mode: crate::config::EgressMode::Http,
        proxy_url: Some("http://credential:secret@127.0.0.1:1".into()),
        ..EgressConfig::default()
    };
    assert!(
        controller
            .classify(&request(), &context(), &egress)
            .is_none()
    );
    assert_eq!(controller.status().last_outcome, Some(Outcome::Unavailable));
    assert!(destination.accept().is_err());
    assert!(
        !serde_json::to_string(&controller.status())
            .unwrap()
            .contains("credential")
    );
}

#[test]
fn ambient_proxy_variables_are_ignored() {
    const CHILD: &str = "TOKEN_STATION_JEV_PROXY_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "jev::tests::ambient_proxy_variables_are_ignored"])
            .env(CHILD, "1")
            .env("HTTP_PROXY", "http://127.0.0.1:1")
            .env("HTTPS_PROXY", "http://127.0.0.1:1")
            .env("ALL_PROXY", "http://127.0.0.1:1")
            .env("http_proxy", "http://127.0.0.1:1")
            .env("https_proxy", "http://127.0.0.1:1")
            .env("all_proxy", "http://127.0.0.1:1")
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        return;
    }
    let dir = Scratch::new();
    let (endpoint, captured) = fixture(200, prediction().to_string(), Duration::ZERO);
    let controller = configured(&dir, &endpoint);
    assert!(
        controller
            .classify(&request(), &context(), &EgressConfig::default())
            .is_some()
    );
    captured.recv().unwrap();
}

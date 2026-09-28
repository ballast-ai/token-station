use super::*;
use serde_json::json;
use token_station_protocol::{ChatRequest, Message};

fn request(messages: serde_json::Value) -> ChatRequest {
    let messages: Vec<Message> = serde_json::from_value(messages).unwrap();
    ChatRequest::new("auto", messages)
}

#[test]
fn projection_keeps_conversation_and_excludes_scaffolding_and_tool_payloads() {
    let input = request(json!([
        {"role":"system","content":"PRIVATE SYSTEM"},
        {"role":"user","content":"Design a consistent payment protocol"},
        {"role":"assistant","content":"Consider failures"},
        {"role":"tool","content":"PRIVATE TOOL","tool_call_id":"a"},
        {"role":"user","content":"Continue"}
    ]));
    let text = project(&input).unwrap();
    assert!(text.contains("payment protocol"));
    assert!(text.ends_with("User: Continue"));
    assert!(!text.contains("PRIVATE"));
}

#[test]
fn projection_never_reuses_history_for_an_empty_latest_user_turn() {
    let input = request(json!([
        {"role":"user","content":"Earlier task"},
        {"role":"user","content":" "}
    ]));
    assert_eq!(project(&input), Err(Outcome::Unsupported));
}

#[test]
fn projection_skips_images_even_when_text_is_present() {
    let input = request(json!([{"role":"user","content":[
        {"type":"text","text":"What is in this image?"},
        {"type":"image_url","image_url":{"url":"https://example.invalid/image"}}
    ]}]));
    assert_eq!(project(&input), Err(Outcome::Unsupported));
}

#[test]
fn projection_does_not_silently_cut_a_large_user_request() {
    let input = request(json!([{"role":"user","content":"x".repeat(MAX_TEXT_BYTES + 1)}]));
    assert_eq!(project(&input), Err(Outcome::Unsupported));
}

#[test]
fn model_output_only_accepts_closed_tier_and_matching_request_id() {
    assert_eq!(
        parse_prediction(r#"{"id":7,"status":"ok","tier":"high"}"#, 7),
        Ok(Tier::High)
    );
    assert_eq!(
        parse_prediction(r#"{"id":8,"status":"ok","tier":"high"}"#, 7),
        Err(Outcome::Invalid)
    );
    assert_eq!(
        parse_prediction(r#"{"id":7,"status":"ok","tier":"private request text"}"#, 7),
        Err(Outcome::Invalid)
    );
    assert_eq!(
        parse_prediction(r#"{"id":7,"status":"unsupported"}"#, 7),
        Err(Outcome::Unsupported)
    );
}

#[test]
fn observation_serialization_has_no_request_text_or_model_score_field() {
    let record = Observation {
        id: 1,
        mode: Mode::Observe,
        baseline_tier: Some(Tier::Low),
        suggested_tier: Some(Tier::High),
        applied: false,
        latency_ms: Some(20),
        outcome: Outcome::Observed,
    };
    let value = serde_json::to_value(record).unwrap();
    let keys: Vec<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "applied",
            "baseline_tier",
            "id",
            "latency_ms",
            "mode",
            "outcome",
            "suggested_tier"
        ]
    );
}

#[cfg(unix)]
struct Fixture {
    directory: PathBuf,
    controller: Arc<SemanticController>,
}
#[cfg(unix)]
impl Fixture {
    fn new(body: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "token-station-semantic-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let root = directory.join("semantic-runtime");
        std::fs::create_dir_all(root.join(".venv/bin")).unwrap();
        std::fs::create_dir_all(root.join("models/scx")).unwrap();
        for name in ["assets.json", "prepared.json"] {
            std::fs::write(root.join(name), b"fixture").unwrap();
        }
        std::fs::write(
            root.join(".venv/bin/python"),
            "#!/bin/sh\nexec /usr/bin/python3 \"$@\"\n",
        )
        .unwrap();
        std::fs::set_permissions(
            root.join(".venv/bin/python"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        for file in [
            "config.json",
            "tokenizer.json",
            "model.safetensors",
            "tokenizer_config.json",
            "chat_template.jinja",
            "README.md",
        ] {
            std::fs::write(root.join("models/scx").join(file), b"test").unwrap();
        }
        std::fs::write(root.join("worker.py"),format!("import json,sys,time,os\nprint(json.dumps({{'event':'ready'}}),flush=True)\nfor line in sys.stdin:\n job=json.loads(line)\n {body}\n")).unwrap();
        let controller = SemanticController::shared(&directory);
        Self {
            directory,
            controller,
        }
    }
    fn start(&self, mode: Mode) {
        self.controller.set_mode(mode).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while self.controller.status().state != "ready" {
            assert!(Instant::now() < until, "{:?}", self.controller.status());
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn context() -> RequestContext {
        RequestContext::detached(Duration::from_secs(10), Duration::from_secs(10))
    }
}
#[cfg(unix)]
impl Drop for Fixture {
    fn drop(&mut self) {
        self.controller.set_mode(Mode::Off).unwrap();
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[test]
#[cfg(unix)]
fn active_result_is_applied_only_after_gateway_commit_and_off_stops_child() {
    let fixture =
        Fixture::new("print(json.dumps({'id':job['id'],'status':'ok','tier':'high'}),flush=True)");
    fixture.start(Mode::Route);
    let input = request(json!([{"role":"user","content":"Explain consensus"}]));
    let suggestion = fixture
        .controller
        .classify(&input, Some(Tier::Low), &Fixture::context())
        .unwrap();
    assert_eq!(suggestion.tier, Tier::High);
    assert!(fixture.controller.status().observations.is_empty());
    fixture.controller.finish(suggestion, true);
    let status = fixture.controller.status();
    assert_eq!(status.counts.disagreements, 1);
    assert!(status.observations[0].applied);
    let process = Arc::clone(
        &fixture
            .controller
            .inner
            .lock()
            .unwrap()
            .worker
            .as_ref()
            .unwrap()
            .process,
    );
    fixture.controller.set_mode(Mode::Off).unwrap();
    assert!(process.child.lock().unwrap().is_none());
    assert!(
        fixture
            .controller
            .classify(&input, None, &Fixture::context())
            .is_none()
    );
}

#[test]
#[cfg(unix)]
fn observe_never_waits_for_inference_and_overload_is_bounded() {
    let fixture = Fixture::new(
        "time.sleep(0.15); print(json.dumps({'id':job['id'],'status':'ok','tier':'medium'}),flush=True)",
    );
    fixture.start(Mode::Observe);
    let input = request(json!([{"role":"user","content":"Explain consensus"}]));
    let started = Instant::now();
    assert!(
        fixture
            .controller
            .classify(&input, Some(Tier::Low), &Fixture::context())
            .is_none()
    );
    assert!(started.elapsed() < Duration::from_millis(100));
    assert!(
        fixture
            .controller
            .classify(&input, None, &Fixture::context())
            .is_none()
    );
    let until = Instant::now() + Duration::from_secs(3);
    while fixture.controller.status().counts.classified == 0 {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    let status = fixture.controller.status();
    assert!(
        status
            .observations
            .iter()
            .any(|row| row.outcome == Outcome::Busy)
    );
    assert!(
        status
            .observations
            .iter()
            .any(|row| row.outcome == Outcome::Observed && !row.applied)
    );
}

#[test]
#[cfg(unix)]
fn soft_timeout_does_not_apply_late_results_and_off_invalidates_suggestions() {
    let fixture = Fixture::new(
        "time.sleep(0.6); print(json.dumps({'id':job['id'],'status':'ok','tier':'high'}),flush=True)",
    );
    fixture.start(Mode::Route);
    let input = request(json!([{"role":"user","content":"Explain consensus"}]));
    let started = Instant::now();
    assert!(
        fixture
            .controller
            .classify(&input, None, &Fixture::context())
            .is_none()
    );
    assert!(started.elapsed() < Duration::from_millis(550));
    std::thread::sleep(Duration::from_millis(300));
    let status = fixture.controller.status();
    assert_eq!(status.observations.len(), 1);
    assert_eq!(status.observations[0].outcome, Outcome::Timeout);
    assert_eq!(status.counts.classified, 0);
}

#[test]
#[cfg(unix)]
fn malformed_output_and_cancelled_requests_fall_back_without_echoing_content() {
    let fixture = Fixture::new(
        "print(json.dumps({'id':job['id'],'status':'ok','tier':'SECRET'}),flush=True)",
    );
    fixture.start(Mode::Route);
    let input = request(json!([{"role":"user","content":"SECRET"}]));
    assert!(
        fixture
            .controller
            .classify(&input, None, &Fixture::context())
            .is_none()
    );
    assert_eq!(
        fixture.controller.status().observations[0].outcome,
        Outcome::Invalid
    );
    assert!(
        !serde_json::to_string(&fixture.controller.status())
            .unwrap()
            .contains("SECRET")
    );
    let ctx = RequestContext::detached(Duration::ZERO, Duration::ZERO);
    assert!(fixture.controller.classify(&input, None, &ctx).is_none());
}

#[test]
#[cfg(unix)]
fn mode_change_discards_inflight_observation_and_recent_history_is_bounded() {
    let fixture = Fixture::new(
        "time.sleep(0.1); print(json.dumps({'id':job['id'],'status':'ok','tier':'high'}),flush=True)",
    );
    fixture.start(Mode::Observe);
    let input = request(json!([{"role":"user","content":"Explain consensus"}]));
    let _ = fixture
        .controller
        .classify(&input, None, &Fixture::context());
    fixture.controller.set_mode(Mode::Off).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    assert!(fixture.controller.status().observations.is_empty());
    fixture.start(Mode::Route);
    for _ in 0..100 {
        fixture.controller.bypass(None, Outcome::Unsupported);
    }
    assert_eq!(fixture.controller.status().observations.len(), 64);
    assert_eq!(fixture.controller.status().counts.fallbacks, 100);
}

#[test]
#[cfg(unix)]
fn hard_watchdog_covers_a_child_that_stops_reading_escaped_input() {
    let fixture = Fixture::new("pass");
    std::fs::write(
        fixture.directory.join("semantic-runtime/worker.py"),
        "import json,time\nprint(json.dumps({'event':'ready'}),flush=True)\ntime.sleep(60)\n",
    )
    .unwrap();
    fixture.start(Mode::Route);
    // JSON escaping expands this admitted projection beyond a pipe buffer.
    let input = request(json!([{"role":"user","content":"\0".repeat(15_000)}]));
    let started = Instant::now();
    assert!(
        fixture
            .controller
            .classify(&input, None, &Fixture::context())
            .is_none()
    );
    assert!(started.elapsed() < Duration::from_millis(550));
    let deadline = Instant::now() + Duration::from_secs(4);
    while fixture.controller.status().state != "error" {
        assert!(
            Instant::now() < deadline,
            "worker watchdog did not terminate the stalled child"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let inner = fixture.controller.inner.lock().unwrap();
    assert!(
        inner
            .worker
            .as_ref()
            .unwrap()
            .process
            .child
            .lock()
            .unwrap()
            .is_none()
    );
}

#[test]
#[cfg(unix)]
fn cancelled_preparation_cannot_report_an_incomplete_model_as_ready() {
    let fixture = Fixture::new("pass");
    assert!(fixture.controller.status().model_ready);
    std::fs::remove_file(fixture.directory.join("semantic-runtime/prepared.json")).unwrap();
    assert!(!fixture.controller.status().model_ready);
    assert!(fixture.controller.set_mode(Mode::Observe).is_err());
    assert_eq!(fixture.controller.status().state, "unprepared");
}

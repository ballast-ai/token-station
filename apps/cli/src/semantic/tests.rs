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
    let text = project(
        &input,
        &RequestContext::detached(Duration::from_secs(10), Duration::from_secs(10)),
    )
    .unwrap();
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
    assert_eq!(
        project(
            &input,
            &RequestContext::detached(Duration::from_secs(10), Duration::from_secs(10))
        ),
        Err(Outcome::Unsupported)
    );
}

#[test]
fn projection_skips_images_even_when_text_is_present() {
    let input = request(json!([{"role":"user","content":[
        {"type":"text","text":"What is in this image?"},
        {"type":"image_url","image_url":{"url":"https://example.invalid/image"}}
    ]}]));
    assert_eq!(
        project(
            &input,
            &RequestContext::detached(Duration::from_secs(10), Duration::from_secs(10))
        ),
        Err(Outcome::Unsupported)
    );
}

#[test]
fn projection_does_not_silently_cut_a_large_user_request() {
    let input = request(json!([{"role":"user","content":"x".repeat(MAX_TEXT_BYTES + 1)}]));
    assert_eq!(
        project(
            &input,
            &RequestContext::detached(Duration::from_secs(10), Duration::from_secs(10))
        ),
        Err(Outcome::Unsupported)
    );
}

#[test]
fn model_output_only_accepts_closed_tier_and_matching_request_id() {
    assert_eq!(
        parse_prediction(r#"{"id":7,"status":"ok","tier":"high"}"#, 7),
        Ok(Prediction::Tier(Tier::High))
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
            "#!/bin/sh\nexec /usr/bin/python3 -u \"$(dirname \"$0\")/../../fixture-worker.py\"\n",
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
        std::fs::write(root.join("worker.py"), b"old managed worker").unwrap();
        std::fs::write(root.join("fixture-worker.py"),format!("import json,sys,time,os\nprint(json.dumps({{'event':'ready'}}),flush=True)\nfor line in sys.stdin:\n job=json.loads(line)\n {body}\n")).unwrap();
        let controller = SemanticController::shared(&directory);
        // No fixture may invoke the production downloader, even on a regression.
        *controller.preparation_script.lock().unwrap() = Some(root.join("prepare-fixture.py"));
        Self {
            directory,
            controller,
        }
    }
    fn start(&self, mode: Mode) {
        self.controller.set_mode(mode).unwrap();
        self.wait_for_state("ready");
    }
    fn wait_for_state(&self, state: &str) {
        let until = Instant::now() + Duration::from_secs(10);
        while self.controller.status().state != state {
            assert!(Instant::now() < until, "{:?}", self.controller.status());
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn prepare_with(&self, body: &str) {
        let root = self.directory.join("semantic-runtime");
        std::fs::remove_file(root.join("prepared.json")).unwrap();
        let script = root.join("prepare-fixture.py");
        std::fs::write(
            &script,
            format!("import pathlib,sys,time\nroot=pathlib.Path(sys.argv[1])\n{body}\n"),
        )
        .unwrap();
        *self.controller.preparation_script.lock().unwrap() = Some(script);
    }
    fn reconstruct(&mut self) {
        self.controller.set_mode(Mode::Off).unwrap();
        let old = std::mem::replace(
            &mut self.controller,
            SemanticController::shared(&self.directory.join("unused-controller")),
        );
        let previous = Arc::downgrade(&old);
        drop(old);
        let until = Instant::now() + Duration::from_secs(10);
        while previous.strong_count() > 0 {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        self.controller = SemanticController::shared(&self.directory);
        *self.controller.preparation_script.lock().unwrap() =
            Some(self.directory.join("semantic-runtime/prepare-fixture.py"));
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
fn saved_off_survives_reconstruction_and_on_can_restart_the_worker() {
    let mut fixture =
        Fixture::new("print(json.dumps({'id':job['id'],'status':'ok','tier':'high'}),flush=True)");
    assert!(fixture.controller.start_automatic_route().unwrap().enabled);
    fixture.wait_for_state("ready");
    let status = fixture.controller.set_enabled(false).unwrap();
    assert!(!status.enabled);
    assert_eq!(status.mode, Mode::Off);
    token_station_private_fs::verify_private_file(
        &fixture.directory.join("semantic-settings.json"),
    )
    .unwrap();
    fixture.reconstruct();
    let status = fixture.controller.start_automatic_route().unwrap();
    assert!(!status.enabled);
    assert_eq!(status.state, "off");
    assert!(fixture.controller.set_enabled(true).unwrap().enabled);
    fixture.wait_for_state("ready");
    let input = request(json!([{"role":"user","content":"Explain consensus"}]));
    assert_eq!(
        fixture
            .controller
            .classify(&input, None, &Fixture::context())
            .unwrap()
            .tier,
        Tier::High
    );
    // Shutdown stops the process without changing the saved choice.
    fixture.reconstruct();
    assert!(fixture.controller.start_automatic_route().unwrap().enabled);
    fixture.wait_for_state("ready");
}

#[test]
fn unavailable_status_never_reports_enabled() {
    assert!(!Status::unavailable().enabled);
}

#[test]
#[cfg(unix)]
fn saved_off_does_not_prepare_missing_assets_on_restart() {
    let mut fixture = Fixture::new("pass");
    fixture.controller.set_enabled(false).unwrap();
    std::fs::remove_file(fixture.directory.join("semantic-runtime/prepared.json")).unwrap();
    fixture.reconstruct();
    let status = fixture.controller.start_automatic_route().unwrap();
    assert!(!status.enabled);
    assert_eq!(status.mode, Mode::Off);
    assert_eq!(status.state, "unprepared");
    assert!(status.error.is_none());
}

#[test]
#[cfg(unix)]
fn preference_write_failure_preserves_both_choice_and_runtime() {
    for enabled in [false, true] {
        let fixture = Fixture::new(
            "print(json.dumps({'id':job['id'],'status':'ok','tier':'high'}),flush=True)",
        );
        fixture.controller.set_enabled(enabled).unwrap();
        if enabled {
            fixture.wait_for_state("ready");
        }
        let path = fixture.directory.join("semantic-settings.json");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let before = serde_json::to_value(fixture.controller.status()).unwrap();
        assert!(fixture.controller.set_enabled(!enabled).is_err());
        assert_eq!(
            serde_json::to_value(fixture.controller.status()).unwrap(),
            before
        );
        let input = request(json!([{"role":"user","content":"Explain consensus"}]));
        let suggestion = fixture
            .controller
            .classify(&input, None, &Fixture::context());
        assert_eq!(
            suggestion.map(|suggestion| suggestion.tier),
            enabled.then_some(Tier::High)
        );
    }
}

#[test]
#[cfg(unix)]
fn invalid_preferences_stay_off_until_an_explicit_choice_repairs_them() {
    for contents in [
        "{}".to_owned(),
        r#"{"enabled":"false"}"#.to_owned(),
        r#"{"enabled":true,"unknown":false}"#.to_owned(),
        "x".repeat(2048),
    ] {
        let fixture = Fixture::new("pass");
        std::fs::write(fixture.directory.join("semantic-settings.json"), contents).unwrap();
        let status = fixture.controller.start_automatic_route().unwrap();
        assert!(!status.enabled);
        assert_eq!(status.mode, Mode::Off);
        assert_eq!(status.state, "error");
        assert!(
            status
                .error
                .as_deref()
                .unwrap()
                .contains("settings are invalid")
        );
        let status = fixture.controller.set_enabled(false).unwrap();
        assert!(!status.enabled);
        assert!(status.error.is_none());
    }
}

#[test]
#[cfg(unix)]
fn a_non_file_preference_keeps_classification_off_without_panicking() {
    let fixture = Fixture::new("pass");
    std::fs::create_dir(fixture.directory.join("semantic-settings.json")).unwrap();
    let status = fixture.controller.start_automatic_route().unwrap();
    assert!(!status.enabled);
    assert_eq!(status.state, "error");
    assert_eq!(status.mode, Mode::Off);
    assert!(status.error.is_some());
}

#[test]
#[cfg(unix)]
fn repeated_on_preserves_the_active_worker_and_repeated_off_stays_off() {
    let fixture = Fixture::new(
        "count=globals().get('count',0)+1; print(json.dumps({'id':job['id'],'status':'ok','tier':'low' if count == 1 else 'high'}),flush=True)",
    );
    fixture.controller.set_enabled(true).unwrap();
    fixture.wait_for_state("ready");
    let input = request(json!([{"role":"user","content":"Explain consensus"}]));
    assert_eq!(
        fixture
            .controller
            .classify(&input, None, &Fixture::context())
            .unwrap()
            .tier,
        Tier::Low
    );
    fixture.controller.set_enabled(true).unwrap();
    assert_eq!(
        fixture
            .controller
            .classify(&input, None, &Fixture::context())
            .unwrap()
            .tier,
        Tier::High
    );
    for _ in 0..2 {
        let status = fixture.controller.set_enabled(false).unwrap();
        assert!(!status.enabled);
        assert_eq!(status.state, "off");
    }
}

#[test]
#[cfg(unix)]
fn enabled_intent_survives_preparation_and_off_cancels_before_a_fresh_on() {
    let fixture =
        Fixture::new("print(json.dumps({'id':job['id'],'status':'ok','tier':'high'}),flush=True)");
    fixture.prepare_with("(root/'started').write_text('started')\nwhile not (root/'continue').exists(): time.sleep(0.01)\n(root/'prepared.json').write_text('fixture')");
    let status = fixture.controller.set_enabled(true).unwrap();
    assert!(status.enabled);
    assert_eq!(status.state, "preparing");
    assert_eq!(status.mode, Mode::Off);
    let preparation = Arc::downgrade(
        fixture
            .controller
            .inner
            .lock()
            .unwrap()
            .preparation
            .as_ref()
            .unwrap(),
    );
    let until = Instant::now() + Duration::from_secs(10);
    while !fixture.directory.join("semantic-runtime/started").exists() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(fixture.controller.set_enabled(true).unwrap().enabled);
    assert!(!fixture.controller.set_enabled(false).unwrap().enabled);
    while preparation.upgrade().is_some() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fixture.controller.status().mode, Mode::Off);
    assert!(fixture.controller.status().error.is_none());
    std::fs::write(
        fixture.directory.join("semantic-runtime/continue"),
        b"continue",
    )
    .unwrap();
    assert!(fixture.controller.set_enabled(true).unwrap().enabled);
    fixture.wait_for_state("ready");
    let input = request(json!([{"role":"user","content":"Explain consensus"}]));
    assert_eq!(
        fixture
            .controller
            .classify(&input, None, &Fixture::context())
            .unwrap()
            .tier,
        Tier::High
    );
}

#[test]
#[cfg(unix)]
fn enabling_during_manual_preparation_starts_route_after_completion() {
    let fixture = Fixture::new("pass");
    fixture.prepare_with("time.sleep(0.15)\n(root/'prepared.json').write_text('fixture')");
    assert!(!fixture.controller.prepare().unwrap().enabled);
    assert!(fixture.controller.set_enabled(true).unwrap().enabled);
    fixture.wait_for_state("ready");
    assert_eq!(fixture.controller.status().mode, Mode::Route);
}

#[test]
#[cfg(unix)]
fn automatic_start_uses_prepared_assets_once_and_preserves_a_later_off() {
    let fixture = Fixture::new(
        "count=globals().get('count',0)+1; print(json.dumps({'id':job['id'],'status':'ok','tier':'low' if count == 1 else 'high'}),flush=True)",
    );
    assert_eq!(fixture.controller.status().mode, Mode::Off);
    let started = Instant::now();
    let status = fixture.controller.start_automatic_route().unwrap();
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(status.mode, Mode::Route);
    fixture.wait_for_state("ready");
    let input = request(json!([{"role":"user","content":"Explain consensus"}]));
    assert_eq!(
        fixture
            .controller
            .classify(&input, None, &Fixture::context())
            .unwrap()
            .tier,
        Tier::Low
    );
    fixture.controller.start_automatic_route().unwrap();
    assert_eq!(
        fixture
            .controller
            .classify(&input, None, &Fixture::context())
            .unwrap()
            .tier,
        Tier::High
    );
    fixture.controller.set_mode(Mode::Off).unwrap();
    assert_eq!(
        fixture.controller.start_automatic_route().unwrap().mode,
        Mode::Off
    );
    assert!(
        fixture
            .controller
            .classify(&input, None, &Fixture::context())
            .is_none()
    );
}

#[test]
#[cfg(unix)]
fn automatic_start_prepares_missing_assets_then_serves_requests() {
    let fixture =
        Fixture::new("print(json.dumps({'id':job['id'],'status':'ok','tier':'high'}),flush=True)");
    fixture.prepare_with("time.sleep(0.15)\n(root/'prepared.json').write_text('fixture')");
    let started = Instant::now();
    assert_eq!(
        fixture.controller.start_automatic_route().unwrap().state,
        "preparing"
    );
    assert!(started.elapsed() < Duration::from_millis(100));
    fixture.controller.start_automatic_route().unwrap();
    fixture.wait_for_state("ready");
    assert_eq!(fixture.controller.status().mode, Mode::Route);
    let input = request(json!([{"role":"user","content":"Explain consensus"}]));
    let suggestion = fixture
        .controller
        .classify(&input, None, &Fixture::context())
        .unwrap();
    assert_eq!(suggestion.tier, Tier::High);
}

#[test]
#[cfg(unix)]
fn automatic_preparation_failure_keeps_fallback_and_does_not_retry() {
    let fixture = Fixture::new("pass");
    fixture.prepare_with("sys.exit(7)");
    fixture.controller.start_automatic_route().unwrap();
    fixture.wait_for_state("error");
    let status = fixture.controller.status();
    assert!(status.enabled);
    assert_eq!(status.mode, Mode::Off);
    assert!(!status.model_ready);
    assert!(
        status
            .error
            .as_deref()
            .unwrap()
            .contains("preparation failed")
    );
    let input = request(json!([{"role":"user","content":"Explain consensus"}]));
    assert!(
        fixture
            .controller
            .classify(&input, None, &Fixture::context())
            .is_none()
    );
    assert_eq!(
        fixture.controller.start_automatic_route().unwrap().state,
        "error"
    );
}

#[test]
#[cfg(unix)]
fn automatic_preparation_requires_complete_assets_even_after_successful_exit() {
    let fixture = Fixture::new("pass");
    fixture.prepare_with("pass");
    fixture.controller.start_automatic_route().unwrap();
    fixture.wait_for_state("error");
    assert_eq!(fixture.controller.status().mode, Mode::Off);
    assert!(!fixture.controller.status().model_ready);
}

#[test]
#[cfg(unix)]
fn manual_preparation_stays_off_and_takes_precedence_over_automatic_start() {
    let fixture = Fixture::new("pass");
    fixture.prepare_with("(root/'prepared.json').write_text('fixture')");
    fixture.controller.prepare().unwrap();
    fixture.controller.start_automatic_route().unwrap();
    fixture.wait_for_state("off");
    let status = fixture.controller.status();
    assert_eq!(status.mode, Mode::Off);
    assert!(status.model_ready);
    assert!(status.error.is_none());
}

#[test]
#[cfg(unix)]
fn cancelled_automatic_preparation_cannot_replace_off_or_a_new_observe_worker() {
    for next_mode in [Mode::Off, Mode::Observe] {
        let fixture = Fixture::new(
            "print(json.dumps({'id':job['id'],'status':'ok','tier':'high'}),flush=True)",
        );
        fixture.prepare_with("(root/'prepared.json').write_text('fixture')\ntime.sleep(30)");
        fixture.controller.start_automatic_route().unwrap();
        let preparation = Arc::downgrade(
            fixture
                .controller
                .inner
                .lock()
                .unwrap()
                .preparation
                .as_ref()
                .unwrap(),
        );
        // The fixture has reached a prepared filesystem, but its process is
        // still running. Cancel before its delayed completion reaches the host.
        let until = Instant::now() + Duration::from_secs(10);
        while !fixture.controller.status().model_ready {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        fixture.controller.set_mode(Mode::Off).unwrap();
        if next_mode == Mode::Observe {
            fixture.start(Mode::Observe);
        }
        // Wait for the old supervisor to finish before checking its effects.
        while preparation.upgrade().is_some() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        let status = fixture.controller.status();
        assert_eq!(status.mode, next_mode);
        assert_eq!(
            status.state,
            if next_mode == Mode::Off {
                "off"
            } else {
                "ready"
            }
        );
        assert!(status.error.is_none());
        assert_eq!(
            fixture.controller.start_automatic_route().unwrap().mode,
            next_mode
        );
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
        fixture.directory.join("semantic-runtime/fixture-worker.py"),
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

#[test]
#[cfg(unix)]
fn byte_limit_diagnostic_preserves_existing_admission_and_request() {
    let mut input = request(json!([{"role":"user","content":"x".repeat(MAX_TEXT_BYTES - 12)}]));
    let ctx = Fixture::context();
    assert!(project(&input, &ctx).is_ok());
    input.messages[0] = Message::text(Role::User, "x".repeat(MAX_TEXT_BYTES - 11));
    let before = input.clone();
    assert_eq!(project(&input, &ctx), Err(Outcome::Unsupported));
    assert_eq!(input, before);
    assert_eq!(
        serde_json::to_value(ctx.classifier_input()).unwrap(),
        json!({"classifier":"scx","handling":"skipped","reason":"byte_limit"})
    );
    input.messages.push(Message::text(Role::User, " "));
    let ctx = Fixture::context();
    assert_eq!(project(&input, &ctx), Err(Outcome::Unsupported));
    assert!(ctx.classifier_input().is_none());
}

#[test]
#[cfg(unix)]
fn unsupported_history_is_not_misreported_as_a_byte_limit() {
    let input = request(json!([
        {"role":"user","content":"x".repeat(MAX_TEXT_BYTES)},
        {"role":"assistant","content":[{"type":"image_url","image_url":{"url":"private"}}]},
        {"role":"user","content":"current"}
    ]));
    let ctx = Fixture::context();
    assert_eq!(project(&input, &ctx), Err(Outcome::Unsupported));
    assert!(ctx.classifier_input().is_none());
}

#[test]
#[cfg(unix)]
fn route_records_exact_token_limit_without_confusing_other_unsupported_input() {
    for (reason, expected) in [("token_limit", true), ("reserved_marker", false)] {
        let fixture = Fixture::new(&format!(
            "print(json.dumps({{'id':job['id'],'status':'unsupported','reason':'{reason}'}}),flush=True)"
        ));
        fixture.start(Mode::Route);
        let ctx = Fixture::context();
        let input = request(json!([{"role":"user","content":"PRIVATE TASK"}]));
        assert!(fixture.controller.classify(&input, None, &ctx).is_none());
        let diagnostic = serde_json::to_value(ctx.classifier_input()).unwrap();
        if expected {
            assert_eq!(
                diagnostic,
                json!({"classifier":"scx","handling":"skipped","reason":"token_limit"})
            );
        } else {
            assert!(diagnostic.is_null());
        }
        assert!(
            !serde_json::to_string(&diagnostic)
                .unwrap()
                .contains("PRIVATE")
        );
        assert_eq!(
            fixture.controller.status().observations[0].outcome,
            Outcome::Unsupported
        );
    }
}

#[test]
#[cfg(unix)]
fn observe_does_not_wait_or_write_a_late_token_diagnostic() {
    let fixture = Fixture::new(
        "time.sleep(0.15); print(json.dumps({'id':job['id'],'status':'unsupported','reason':'token_limit'}),flush=True)",
    );
    fixture.start(Mode::Observe);
    let ctx = Fixture::context();
    let input = request(json!([{"role":"user","content":"task"}]));
    let started = Instant::now();
    assert!(fixture.controller.classify(&input, None, &ctx).is_none());
    assert!(started.elapsed() < Duration::from_millis(100));
    assert!(ctx.classifier_input().is_none());
    std::thread::sleep(Duration::from_millis(250));
    assert!(ctx.classifier_input().is_none());
    assert_eq!(
        fixture.controller.status().observations[0].outcome,
        Outcome::Unsupported
    );
}

#[test]
#[cfg(unix)]
fn existing_prepared_runtime_refreshes_only_the_managed_worker_before_start() {
    let fixture =
        Fixture::new("print(json.dumps({'id':job['id'],'status':'ok','tier':'high'}),flush=True)");
    let root = fixture.directory.join("semantic-runtime");
    let preserved = [
        ".venv/bin/python",
        "prepared.json",
        "assets.json",
        "models/scx/model.safetensors",
    ]
    .map(|name| (name, std::fs::read(root.join(name)).unwrap()));
    fixture.start(Mode::Route);
    assert_eq!(
        std::fs::read_to_string(root.join("worker.py")).unwrap(),
        include_str!("../../../../scripts/scx-runtime/worker.py")
    );
    token_station_private_fs::verify_private_file(&root.join("worker.py")).unwrap();
    for (name, bytes) in preserved {
        assert_eq!(std::fs::read(root.join(name)).unwrap(), bytes, "{name}");
    }
}

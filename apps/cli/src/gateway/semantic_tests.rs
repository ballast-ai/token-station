//! Gateway routing tests with an offline, deterministic classifier process.
#![cfg(unix)]

#[allow(clippy::wildcard_imports)]
use super::*;
use crate::semantic::{Mode, Outcome, SemanticController, Status, Tier};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use token_station_protocol::{AgentHint, HintKind, Role};
use token_station_router_core::{DecidedBy, Health, HintRoute, Match, Rule};

struct Fixture {
    data: PathBuf,
    controller: Arc<SemanticController>,
    gateway: Gateway,
}

impl Fixture {
    fn new(mode: Mode) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let data = std::env::temp_dir().join(format!(
            "token-station-semantic-gateway-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&data).unwrap();
        let runtime = data.join("semantic-runtime");
        std::fs::create_dir_all(runtime.join(".venv/bin")).unwrap();
        std::fs::create_dir_all(runtime.join("models/scx")).unwrap();
        for name in ["assets.json", "prepared.json"] {
            std::fs::write(runtime.join(name), b"fixture").unwrap();
        }
        let python = runtime.join(".venv/bin/python");
        std::fs::write(
            &python,
            b"#!/bin/sh\nexec /usr/bin/python3 -u \"$(dirname \"$0\")/../../fixture-worker.py\"\n",
        )
        .unwrap();
        std::fs::set_permissions(python, std::fs::Permissions::from_mode(0o700)).unwrap();
        for name in [
            "config.json",
            "tokenizer.json",
            "model.safetensors",
            "tokenizer_config.json",
            "chat_template.jinja",
            "README.md",
        ] {
            std::fs::write(runtime.join("models/scx").join(name), b"test placeholder").unwrap();
        }
        std::fs::write(
            runtime.join("fixture-worker.py"),
            r#"import json
import sys
print(json.dumps({"event": "ready"}), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    print(json.dumps({"id": request["id"], "status": "ok", "tier": "high"}), flush=True)
"#,
        )
        .unwrap();
        std::fs::write(runtime.join("worker.py"), b"# managed worker placeholder\n").unwrap();
        let controller = SemanticController::shared(&data);
        let gateway = gateway(&data, &controller);
        let fixture = Self {
            data,
            controller,
            gateway,
        };
        fixture.controller.set_mode(mode).unwrap();
        fixture.wait_for(|status| status.state == "ready");
        fixture
    }

    fn wait_for(&self, predicate: impl Fn(&Status) -> bool) -> Status {
        wait_for_status(&self.controller, Duration::from_secs(5), predicate)
    }

    fn route(&self, router: &Router, request: &ChatRequest, candidates: &[Candidate]) -> Decision {
        self.gateway
            .route_with_semantics(
                &RequestContext::detached(Duration::from_secs(5), Duration::from_secs(1)),
                router,
                request,
                &[],
                candidates,
                "fixture-session",
            )
            .unwrap()
    }
}

fn wait_for_status(
    controller: &SemanticController,
    timeout: Duration,
    predicate: impl Fn(&Status) -> bool,
) -> Status {
    let deadline = Instant::now() + timeout;
    loop {
        let status = controller.status();
        if predicate(&status) {
            return status;
        }
        assert!(Instant::now() < deadline, "classifier state: {status:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.controller.set_mode(Mode::Off);
        let _ = std::fs::remove_dir_all(&self.data);
    }
}

fn gateway(data: &Path, controller: &Arc<SemanticController>) -> Gateway {
    let mut config: ClientConfig = serde_json::from_str(crate::EXAMPLE_CONFIG).unwrap();
    config.data.dir = data.to_path_buf();
    // No adapters or upstream transport are constructed. Only the actual
    // gateway routing seam, policy filters, and classifier child are used.
    Gateway {
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
        secrets: SecretStore::from_config(&config, &config.data.dir),
        egress: EgressPolicy::new(config.egress),
        south_runtime: None,
        recorder: Arc::new(token_station_metrics::NoopRecorder),
        body_log: None,
        semantic: None,
        jev: crate::jev::JevController::shared(&config.data.dir),
    }
    .with_semantic_routing(Arc::clone(controller))
}

fn target(model: &str) -> UpstreamModel {
    UpstreamModel::new(UpstreamRef::new("fixture").unwrap(), model)
}

fn config() -> RouterConfig {
    serde_json::from_value(json!({
        "version": 1,
        "pools": {
            "tier_low": [target("low-model")],
            "tier_mid": [target("mid-model")],
            "tier_high": [target("high-model")]
        },
        "default_pool": "tier_low"
    }))
    .unwrap()
}

fn candidate(model: &str) -> Candidate {
    Candidate::new(
        target(model),
        ModelCapability {
            model: model.to_owned(),
            tool: true,
            vision: true,
            context_window: 128_000,
            ..ModelCapability::default()
        },
        Health::Healthy,
    )
}

fn candidates() -> Vec<Candidate> {
    ["low-model", "mid-model", "high-model"]
        .map(candidate)
        .to_vec()
}

fn request(text: &str) -> ChatRequest {
    ChatRequest::new("auto", vec![Message::text(Role::User, text)])
}

#[test]
fn observation_preserves_the_complete_route_and_records_the_async_suggestion() {
    let fixture = Fixture::new(Mode::Observe);
    let router = Router::new(config()).unwrap();
    let request = request("Explain this function");
    let candidates = candidates();
    let baseline = router.route(&request, &[], &candidates).unwrap();

    assert_eq!(fixture.route(&router, &request, &candidates), baseline);
    let status = fixture.wait_for(|status| status.observations.len() == 1);
    let observation = &status.observations[0];
    assert_eq!(observation.outcome, Outcome::Observed);
    assert_eq!(observation.baseline_tier, Some(Tier::Low));
    assert_eq!(observation.suggested_tier, Some(Tier::High));
    assert!(!observation.applied);
    assert_eq!(status.counts.disagreements, 1);
}

#[test]
fn route_mode_applies_the_suggested_pool_with_classifier_attribution() {
    let fixture = Fixture::new(Mode::Route);
    let router = Router::new(config()).unwrap();
    let request = request("Explain this function");
    let candidates = candidates();
    let baseline = router.route(&request, &[], &candidates).unwrap();
    let decision = fixture.route(&router, &request, &candidates);

    assert_eq!(decision.chosen, target("high-model"));
    assert_eq!(decision.pool, "tier_high");
    assert_eq!(decision.decided_by, DecidedBy::Classifier);
    assert_eq!(decision.features, baseline.features);
    let status = fixture.controller.status();
    assert_eq!(status.observations[0].outcome, Outcome::Applied);
    assert!(status.observations[0].applied);
}

#[test]
fn exact_auto_does_not_classify_but_the_host_dynamic_variant_does() {
    let fixture = Fixture::new(Mode::Route);
    let mut config = config();
    config.honor_exact_model = true;
    let (exact, dynamic) = router_with_dynamic_variant(config, "fixture").unwrap();
    let request = request("Explain this function");
    let mut candidates = candidates();
    let mut literal_auto = candidates[0].clone();
    literal_auto.target = target("auto");
    literal_auto.capability.model = "auto".into();
    candidates.push(literal_auto);
    let baseline = exact.route(&request, &[], &candidates).unwrap();
    assert_eq!(fixture.route(&exact, &request, &candidates), baseline);
    assert_eq!(baseline.chosen, target("auto"));
    assert_eq!(fixture.controller.status().counts.classified, 0);
    assert_eq!(
        fixture.controller.status().observations[0].outcome,
        Outcome::Overridden
    );
    let decision = fixture.route(&dynamic, &request, &candidates);
    assert_eq!(decision.chosen, target("high-model"));
    assert_eq!(decision.decided_by, DecidedBy::Classifier);
    assert_eq!(fixture.controller.status().counts.classified, 1);
}

#[test]
fn applied_local_classification_keeps_the_host_schema_receipt_estimate() {
    let fixture = Fixture::new(Mode::Route);
    let router = Router::new(config()).unwrap();
    let mut request = request("Explain this function");
    request.tools.push(ToolDef {
        name: "inspect".into(),
        description: Some("Inspect a document".into()),
        parameters: json!({"description":"schema ".repeat(2000)}),
    });
    request.response_format = Some(token_station_protocol::ResponseFormat::JsonSchema {
        json_schema: json!({"description":"output ".repeat(1000)}),
    });
    let mut candidates = candidates();
    for candidate in &mut candidates {
        candidate.capability.json_schema = true;
    }
    let baseline = fixture
        .gateway
        .route_with_mode(&router, &request, &[], &candidates, "fixture")
        .unwrap();
    assert!(baseline.features.estimated_input_tokens > 1000);
    let decision = fixture.route(&router, &request, &candidates);
    assert_eq!(decision.decided_by, DecidedBy::Classifier);
    assert_eq!(decision.features, baseline.features);
}

#[test]
fn exact_models_user_rules_and_agent_hints_keep_precedence() {
    let fixture = Fixture::new(Mode::Route);
    let candidates = candidates();
    for precedence in ["exact", "rule", "hint"] {
        let mut config = config();
        let mut request = request("Explain this function");
        let mut hints = Vec::new();
        match precedence {
            "exact" => {
                config.honor_exact_model = true;
                request.model = "mid-model".into();
            }
            "rule" => config.rules.push(Rule {
                id: "explain".into(),
                matcher: Match {
                    keywords_any: vec!["Explain".into()],
                    ..Match::default()
                },
                route_to: "tier_mid".into(),
            }),
            _ => {
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
        }
        let router = Router::new(config).unwrap();
        let baseline = router.route(&request, &hints, &candidates).unwrap();
        let decision = fixture
            .gateway
            .route_with_semantics(
                &RequestContext::detached(Duration::from_secs(5), Duration::from_secs(1)),
                &router,
                &request,
                &hints,
                &candidates,
                "fixture-session",
            )
            .unwrap();
        assert_eq!(decision, baseline, "{precedence}");
        assert_eq!(decision.chosen, target("mid-model"));
    }
    let status = fixture.controller.status();
    assert_eq!(status.counts.classified, 0);
    assert_eq!(status.observations.len(), 3);
    assert!(
        status
            .observations
            .iter()
            .all(|row| row.outcome == Outcome::Overridden)
    );
}

#[test]
fn image_and_oversized_text_inputs_keep_the_existing_route() {
    let fixture = Fixture::new(Mode::Route);
    let router = Router::new(config()).unwrap();
    let candidates = candidates();
    let image = ChatRequest::new(
        "auto",
        serde_json::from_value(json!([
            {"role":"user", "content":[
                {"type":"text", "text":"Describe this image"},
                {"type":"image_url", "image_url":{"url":"data:image/png;base64,cGl4ZWxz"}}
            ]}
        ]))
        .unwrap(),
    );
    for request in [image, request(&"x".repeat(20_000))] {
        let baseline = fixture
            .gateway
            .route_with_mode(&router, &request, &[], &candidates, "fixture-session")
            .unwrap();
        assert_eq!(fixture.route(&router, &request, &candidates), baseline);
    }
    let status = fixture.controller.status();
    assert_eq!(status.counts.fallbacks, 2);
    assert!(
        status
            .observations
            .iter()
            .all(|row| row.outcome == Outcome::Unsupported && row.suggested_tier.is_none())
    );
}

#[test]
fn a_classifier_suggestion_still_filters_incompatible_tool_models() {
    let fixture = Fixture::new(Mode::Route);
    let mut config = config();
    config
        .pools
        .get_mut("tier_high")
        .unwrap()
        .push(target("tool-model"));
    let router = Router::new(config).unwrap();
    let mut candidates = candidates();
    candidates[2].capability.tool = false;
    candidates.push(candidate("tool-model"));
    let mut request = request("Read a file");
    request.tools.push(ToolDef {
        name: "read_file".into(),
        description: None,
        parameters: json!({}),
    });

    let decision = fixture.route(&router, &request, &candidates);
    assert_eq!(decision.decided_by, DecidedBy::Classifier);
    assert_eq!(decision.chosen, target("tool-model"));
    assert!(!decision.fallbacks.contains(&target("high-model")));
}

#[test]
fn an_unavailable_suggested_pool_preserves_the_baseline_decision() {
    let fixture = Fixture::new(Mode::Route);
    let router = Router::new(config()).unwrap();
    let request = request("Explain this function");
    let candidates = vec![candidate("low-model"), candidate("mid-model")];
    let baseline = router.route(&request, &[], &candidates).unwrap();

    assert_eq!(fixture.route(&router, &request, &candidates), baseline);
    let status = fixture.controller.status();
    assert_eq!(status.observations[0].outcome, Outcome::NoRoute);
    assert_eq!(status.observations[0].suggested_tier, Some(Tier::High));
    assert!(!status.observations[0].applied);
    assert_eq!(status.counts.fallbacks, 1);
}

#[test]
fn turning_off_removes_routing_influence_and_stops_new_observations() {
    let fixture = Fixture::new(Mode::Route);
    let router = Router::new(config()).unwrap();
    let request = request("Explain this function");
    let candidates = candidates();
    assert_eq!(
        fixture.route(&router, &request, &candidates).pool,
        "tier_high"
    );
    fixture.controller.set_mode(Mode::Off).unwrap();
    let before = fixture.controller.status();

    assert_eq!(
        fixture.route(&router, &request, &candidates),
        router.route(&request, &[], &candidates).unwrap()
    );
    let after = fixture.controller.status();
    assert_eq!(after.state, "off");
    assert_eq!(after.observations.len(), before.observations.len());
    assert_eq!(after.counts.classified, before.counts.classified);
}

#[test]
fn non_tier_and_quota_routes_do_not_use_classifier_suggestions() {
    let fixture = Fixture::new(Mode::Route);
    let request = request("Explain this function");
    let candidates = candidates();
    for quota in [false, true] {
        let mut config = config();
        if quota {
            config.routing_mode = RoutingMode::QuotaFirst;
            config.quota_accounts = vec![target("low-model")];
        } else {
            config.pools.remove("tier_mid");
        }
        let router = Router::new(config).unwrap();
        let baseline = fixture
            .gateway
            .route_with_mode(&router, &request, &[], &candidates, "fixture-session")
            .unwrap();
        assert_eq!(fixture.route(&router, &request, &candidates), baseline);
    }
    let status = fixture.controller.status();
    assert_eq!(status.counts.classified, 0);
    assert!(
        status
            .observations
            .iter()
            .all(|row| row.outcome == Outcome::Overridden)
    );
}

#[test]
fn an_enabled_jev_failure_does_not_invoke_the_ready_scx_classifier() {
    let fixture = Fixture::new(Mode::Route);
    let endpoint = super::jev_tests::Endpoint::new(503, json!({"error":"synthetic unavailable"}));
    let jev = crate::jev::JevController::for_test(&fixture.data, &endpoint.url);
    jev.save_key("synthetic-jev-key").unwrap();
    jev.set_enabled(true).unwrap();
    let router = Router::new(config()).unwrap();
    let request = request("Explain this function");
    let candidates = candidates();
    let before = fixture.controller.status();
    assert_eq!(
        fixture.route(&router, &request, &candidates),
        router.route(&request, &[], &candidates).unwrap()
    );
    let after = fixture.controller.status();
    assert_eq!(after.counts.classified, before.counts.classified);
    assert_eq!(after.observations.len(), before.observations.len());
    assert_eq!(
        jev.status().last_outcome,
        Some(crate::jev::Outcome::Unavailable)
    );
    jev.set_enabled(false).unwrap();
    assert_eq!(
        fixture.route(&router, &request, &candidates).pool,
        "tier_high"
    );
}

#[test]
#[ignore = "requires a prepared local model and exclusive model access"]
fn prepared_local_model_reaches_the_gateway_and_receipt_without_upstream_calls() {
    struct StopOnDrop(Arc<SemanticController>);
    impl Drop for StopOnDrop {
        fn drop(&mut self) {
            let _ = self.0.set_mode(Mode::Off);
        }
    }

    let data =
        PathBuf::from(std::env::var_os("TOKEN_STATION_SCX_TEST_DATA_DIR").expect(
            "Set TOKEN_STATION_SCX_TEST_DATA_DIR to the isolated experimental data directory",
        ));
    assert!(data.is_absolute() && data.is_dir());
    let controller = SemanticController::shared(&data);
    let _stop = StopOnDrop(Arc::clone(&controller));
    let gateway = gateway(&data, &controller);
    let router = Router::new(config()).unwrap();
    let candidates = candidates();
    // This exact task returned High in the independent offline worker smoke
    // test. This test checks the ML-to-gateway bridge, not general accuracy.
    let request = request(
        "Prove that every continuous function on a compact metric space is uniformly continuous. Give a rigorous proof by contradiction, justify each use of compactness, and explain why the result fails without compactness.",
    );
    let baseline = router.route(&request, &[], &candidates).unwrap();
    assert_eq!(baseline.pool, "tier_low");

    for mode in [Mode::Observe, Mode::Route] {
        let previous_rows = controller.status().observations.len();
        controller.set_mode(mode).unwrap();
        // The actual worker performs its own warmup before its ready event.
        wait_for_status(&controller, Duration::from_secs(100), |status| {
            status.state == "ready"
        });
        let decision = gateway
            .route_with_semantics(
                &RequestContext::detached(Duration::from_secs(10), Duration::from_secs(1)),
                &router,
                &request,
                &[],
                &candidates,
                "real-model-fixture",
            )
            .unwrap();
        let status = wait_for_status(&controller, Duration::from_secs(5), |status| {
            status.observations.len() > previous_rows
        });
        let observation = status.observations.last().unwrap();
        assert_eq!(observation.suggested_tier, Some(Tier::High), "{status:?}");
        if mode == Mode::Observe {
            assert_eq!(decision, baseline);
            assert_eq!(observation.outcome, Outcome::Observed);
            assert!(!observation.applied);
        } else {
            assert_eq!(decision.pool, "tier_high");
            assert_eq!(decision.chosen, target("high-model"));
            assert_eq!(decision.decided_by, DecidedBy::Classifier);
            assert_eq!(observation.outcome, Outcome::Applied);
            assert!(observation.applied);
            let receipt = serde_json::to_value(DecisionRecord::from(&decision)).unwrap();
            assert_eq!(receipt["decided_by"], json!({"tier": "classifier"}));
            assert_eq!(receipt["pool"], "tier_high");
        }
        eprintln!(
            "Real SCX gateway bridge: mode={mode:?}, pool={}, latency_ms={:?}",
            decision.pool, observation.latency_ms
        );
    }
    controller.set_mode(Mode::Off).unwrap();
    assert_eq!(controller.status().state, "off");
}

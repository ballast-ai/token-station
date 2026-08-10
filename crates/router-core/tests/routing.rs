//! The four layers, in the order they are promised to fire, and the one
//! property that outlives every request: a decision cannot carry content.

use std::collections::BTreeMap;

use token_station_protocol::{
    AgentHint, CapabilityState, ChatRequest, ErrorCode, HintKind, Message, ModelCapability,
    ResponseFormat, Role, ToolDef,
};
use token_station_router_core::{
    Candidate, DecidedBy, Health, Heuristic, HintRoute, Match, NoRoute, QuotaConfig, QuotaState,
    RecoveryPolicy, ResetWindow, Router, RouterConfig, RoutingMode, Rule, UnmetRequirement,
    UpstreamModel, UpstreamRef, Weights,
};

fn target(upstream: &str, model: &str) -> UpstreamModel {
    UpstreamModel::new(UpstreamRef::new(upstream).expect("valid reference"), model)
}

fn config() -> RouterConfig {
    let mut pools = BTreeMap::new();
    pools.insert("cheap".to_owned(), vec![target("ollama_local", "llama3.3")]);
    pools.insert(
        "sota".to_owned(),
        vec![
            target("openai_personal", "gpt-5.5"),
            target("anthropic_personal", "claude-opus-4-8"),
        ],
    );

    RouterConfig {
        version: 1,
        pools,
        rules: vec![Rule {
            id: "proofs-need-the-good-model".to_owned(),
            matcher: Match {
                keywords_any: vec!["证明".to_owned(), "proof".to_owned()],
                ..Match::default()
            },
            route_to: "sota".to_owned(),
        }],
        hint_routes: vec![HintRoute {
            kind: HintKind::StepType,
            value: "summarize".to_owned(),
            route_to: "cheap".to_owned(),
        }],
        heuristic: Some(Heuristic {
            weights: Weights {
                tokens_per_point: 100,
                per_tool: 20,
                json_schema: 10,
                image: 15,
                per_code_block: 8,
                per_extra_turn: 3,
                per_reasoning_marker: 0,
                per_technical_term: 0,
                per_simple_indicator: 0,
                per_code_keyword: 0,
                per_math_term: 0,
                per_creative_term: 0,
                per_multi_step_point: 0,
                per_question: 0,
                system_format: 0,
                max_output_tokens_per_point: 0,
            },
            threshold: 40,
            above: "sota".to_owned(),
            below: "cheap".to_owned(),
            bands: Vec::new(),
        }),
        default_pool: "cheap".to_owned(),
        assumed_context_window: 8_192,
        honor_exact_model: false,
        recovery: RecoveryPolicy::Strict,
        local_only: false,
        allow_cloud_fallback: false,
        routing_mode: RoutingMode::Tiered,
        quota: QuotaConfig::default(),
        quota_accounts: Vec::new(),
    }
}

/// The shared config, but locked to local upstreams. `cheap` holds the local
/// model; `sota` holds only cloud upstreams.
fn local_only_config(allow_cloud_fallback: bool) -> RouterConfig {
    RouterConfig {
        local_only: true,
        allow_cloud_fallback,
        ..config()
    }
}

fn router() -> Router {
    Router::new(config()).expect("the example config validates")
}

fn capable(context_window: u32) -> ModelCapability {
    ModelCapability {
        tool: true,
        vision: true,
        json_schema: true,
        tool_state: Some(CapabilityState::Declared),
        vision_state: Some(CapabilityState::Declared),
        json_schema_state: Some(CapabilityState::Declared),
        context_window,
        ..ModelCapability::default()
    }
}

fn tool() -> ToolDef {
    ToolDef {
        name: "get_weather".to_owned(),
        description: None,
        parameters: serde_json::json!({}),
    }
}

fn candidates() -> Vec<Candidate> {
    vec![
        // A local model: tools, yes; vision and schema-constrained output, no.
        Candidate::new(
            target("ollama_local", "llama3.3"),
            ModelCapability {
                tool: true,
                tool_state: Some(CapabilityState::Declared),
                context_window: 128_000,
                ..ModelCapability::default()
            },
            Health::Healthy,
        )
        .local(true),
        Candidate::new(
            target("openai_personal", "gpt-5.5"),
            capable(400_000),
            Health::Healthy,
        ),
        Candidate::new(
            target("anthropic_personal", "claude-opus-4-8"),
            capable(200_000),
            Health::Healthy,
        ),
    ]
}

fn ask(text: &str) -> ChatRequest {
    ChatRequest::new("auto", vec![Message::text(Role::User, text)])
}

#[test]
fn a_rule_beats_a_hint_that_would_have_said_otherwise() {
    let hints = [AgentHint::new(HintKind::StepType, "summarize")];
    let decision = router()
        .route(&ask("请给出这个引理的证明"), &hints, &candidates())
        .expect("routable");

    assert_eq!(decision.pool, "sota");
    assert_eq!(
        decision.decided_by,
        DecidedBy::Rule {
            rule: "proofs-need-the-good-model".to_owned()
        },
        "the operator's rule is the override the user is promised"
    );
}

#[test]
fn a_hint_beats_the_heuristic_that_would_have_said_otherwise() {
    // Long enough to score above the threshold on its own.
    let long = "x".repeat(40_000);
    let hints = [AgentHint::new(HintKind::StepType, "summarize")];

    let unhinted = router()
        .route(&ask(&long), &[], &candidates())
        .expect("routable");
    let hinted = router()
        .route(&ask(&long), &hints, &candidates())
        .expect("routable");

    assert_eq!(unhinted.pool, "sota", "the heuristic alone would escalate");
    assert_eq!(
        hinted.pool, "cheap",
        "the agent knows it is only summarising"
    );
    assert_eq!(
        hinted.decided_by,
        DecidedBy::Hint {
            kind: HintKind::StepType,
            value: "summarize".to_owned()
        }
    );
}

#[test]
fn the_heuristic_scores_a_request_nothing_else_claimed() {
    // 20,000 ASCII characters ≈ 5,000 estimated tokens → 50 points, over the
    // threshold of 40. The advertised tool is fixed per-agent scaffolding and no
    // longer contributes to the score, so the escalation is driven purely by the
    // conversation content.
    let mut request = ask(&"x".repeat(20_000));
    request.tools = vec![tool()];

    let decision = router()
        .route(&request, &[], &candidates())
        .expect("routable");

    assert_eq!(
        decision.decided_by,
        DecidedBy::Heuristic {
            score: 50,
            threshold: 40
        }
    );
    assert_eq!(decision.pool, "sota");
}

#[test]
fn a_trivial_turn_stays_cheap_under_a_heavy_agent_harness() {
    // The regression that motivated scoring on conversation content only: an
    // agent (e.g. OpenCode) wraps a one-word user turn in a multi-thousand-token
    // system prompt and a dozen advertised tools. Neither the system prompt nor
    // the tool count is the user's request; a simple greeting must still land in `cheap`.
    let mut request = ChatRequest::new(
        "auto",
        vec![
            Message::text(Role::System, "You are a coding agent. ".repeat(400)),
            Message::text(Role::User, "你好"),
        ],
    );
    request.tools = (0..12).map(|_| tool()).collect();

    let decision = router()
        .route(&request, &[], &candidates())
        .expect("routable");

    assert_eq!(decision.pool, "cheap", "a greeting is not a hard request");
    assert!(
        matches!(decision.decided_by, DecidedBy::Heuristic { score, .. } if score < 40),
        "the score must reflect the two-character turn, not the harness: {:?}",
        decision.decided_by
    );
}

#[test]
fn the_threshold_escalates_at_the_boundary_not_past_it() {
    // 16,000 ASCII characters ≈ 4,000 tokens → exactly 40 points.
    let at = router()
        .route(&ask(&"x".repeat(16_000)), &[], &candidates())
        .expect("routable");
    // 4,000 characters ≈ 1,000 tokens → 10 points.
    let under = router()
        .route(&ask(&"x".repeat(4_000)), &[], &candidates())
        .expect("routable");

    assert_eq!(at.pool, "sota", "`score >= threshold` escalates");
    assert_eq!(under.pool, "cheap");
}

#[test]
fn the_router_never_silently_leaves_the_pool_it_was_told_to_use() {
    // The heuristic scores this low and picks `cheap`, whose only model reports
    // no schema-constrained output. Escalating to `sota` anyway would be a
    // silent override of the operator's routing table — exactly the "did you
    // quietly swap my model" suspicion the transparency design exists to answer.
    // So the router refuses, names the pool and names what it could not do.
    let mut request = ask("hello");
    request.response_format = Some(ResponseFormat::JsonObject);

    let refused = router()
        .route(&request, &[], &candidates())
        .expect_err("`cheap` cannot honour a JSON schema");

    assert_eq!(
        refused,
        NoRoute::Unsatisfiable {
            pool: "cheap".to_owned(),
            reason: UnmetRequirement::JsonSchema,
        }
    );
}

#[test]
fn routing_is_pure_and_therefore_replayable() {
    let request = ask("请给出这个引理的证明");
    let first = router()
        .route(&request, &[], &candidates())
        .expect("routable");
    let second = router()
        .route(&request, &[], &candidates())
        .expect("routable");

    assert_eq!(
        first, second,
        "an audit log that cannot be replayed proves nothing"
    );
}

#[test]
fn a_decision_cannot_carry_the_prompt_that_produced_it() {
    // Every string the caller controls is this canary. A keyword rule reads the
    // message text on the way past — and must remember nothing of it.
    const CANARY: &str = "PROMPT-CONTENT-CANARY";

    let mut request = ChatRequest::new(
        CANARY,
        vec![Message::text(Role::User, format!("{CANARY} 证明"))],
    );
    request.tools = vec![ToolDef {
        name: CANARY.to_owned(),
        description: Some(CANARY.to_owned()),
        parameters: serde_json::json!({ "secret": CANARY }),
    }];
    let hints = [AgentHint::new(HintKind::StepType, CANARY)];

    let decision = router()
        .route(&request, &hints, &candidates())
        .expect("routable");
    let serialized = serde_json::to_string(&decision).expect("serializable decision");

    assert!(
        !serialized.contains(CANARY),
        "the decision record is what the metrics store persists: {serialized}"
    );
    assert_eq!(
        decision.decided_by,
        DecidedBy::Rule {
            rule: "proofs-need-the-good-model".to_owned()
        },
        "the keyword rule did read the prompt; it simply did not keep it"
    );
}

#[test]
fn a_degraded_upstream_is_a_fallback_not_a_first_choice() {
    let mut candidates = candidates();
    candidates[1].health = Health::Degraded; // openai_personal, first in the pool

    let decision = router()
        .route(&ask("请给出证明"), &[], &candidates)
        .expect("routable");

    assert_eq!(
        decision.chosen,
        target("anthropic_personal", "claude-opus-4-8")
    );
    assert_eq!(
        decision.fallbacks,
        vec![target("openai_personal", "gpt-5.5")]
    );
}

#[test]
fn the_operators_order_survives_inside_a_health_class() {
    let decision = router()
        .route(&ask("请给出证明"), &[], &candidates())
        .expect("routable");

    assert_eq!(decision.chosen, target("openai_personal", "gpt-5.5"));
    assert_eq!(
        decision.fallbacks,
        vec![target("anthropic_personal", "claude-opus-4-8")]
    );
}

#[test]
fn an_all_ejected_pool_degrades_to_last_resort_and_an_incapable_one_hard_fails() {
    let mut removed = candidates();
    removed[0].health = Health::Unavailable; // the only member of `cheap`

    // Every capable candidate in `cheap` is ejected. Rather than a hard 503 for
    // the length of a cooldown, the pool degrades to a last-resort probe: the
    // ejected candidate is offered so a single-candidate pool keeps making
    // progress (a successful probe clears the ejection).
    let last_resort = router()
        .route(&ask("hi"), &[], &removed)
        .expect("an all-ejected pool degrades to a last-resort probe, not a 503");
    assert_eq!(last_resort.chosen, removed[0].target);

    // `cheap` holds a model that reports no vision support.
    let mut request = ask("what is in this image");
    request.messages[0].content = Some(token_station_protocol::Content::Parts(vec![
        token_station_protocol::ContentPart::ImageUrl {
            image_url: token_station_protocol::ImageUrl {
                url: "https://example/cat.png".to_owned(),
                detail: None,
            },
        },
    ]));
    let mut cheap_only = config();
    cheap_only.heuristic = None;
    cheap_only.rules.clear();
    let router = Router::new(cheap_only).expect("valid");

    let incapable = router
        .route(&request, &[], &candidates())
        .expect_err("llama3.3 reports no vision support");
    assert_eq!(
        incapable,
        NoRoute::Unsatisfiable {
            pool: "cheap".to_owned(),
            reason: UnmetRequirement::Vision,
        }
    );
    assert_eq!(incapable.error_code(), ErrorCode::Capability);
    assert!(
        !incapable.error_code().is_retriable_elsewhere(),
        "another upstream would refuse for the same reason"
    );
}

#[test]
fn an_over_context_request_is_forwarded_rather_than_refused() {
    let mut candidates = candidates();
    // A self-hosted upstream whose adapter could not enumerate its models: its
    // window is unknown (0), so it is assumed small (8192) — never unlimited.
    candidates[0].capability.context_window = 0;

    let long = "x".repeat(40_000); // ~10k estimated tokens, over the 8192 assumption
    let mut config = config();
    config.heuristic = None; // send everything to `cheap` via default_pool
    config.rules.clear();
    let router = Router::new(config).expect("valid");

    // Context length is a soft preference, not a gate. With nothing in the pool
    // large enough, the request is forwarded to the last-resort candidate rather
    // than refused: the upstream's real context error — or the client's own
    // `/compact` — resolves the overflow, whereas a 400 dead-ends the session.
    let decision = router
        .route(&ask(&long), &[], &candidates)
        .expect("an over-context request is forwarded, not refused");

    assert_eq!(decision.pool, "cheap");
    assert_eq!(decision.chosen, target("ollama_local", "llama3.3"));
}

#[test]
fn context_window_prefers_a_fitting_candidate_and_falls_back_to_the_largest() {
    // One pool, three windows: small (8k), medium (64k), large (256k).
    let mut pools = BTreeMap::new();
    pools.insert(
        "cheap".to_owned(),
        vec![
            target("small", "m"),
            target("medium", "m"),
            target("large", "m"),
        ],
    );
    let config = RouterConfig {
        pools,
        heuristic: None,
        rules: Vec::new(),
        hint_routes: Vec::new(),
        ..config()
    };
    let router = Router::new(config).expect("valid");
    let candidates = vec![
        Candidate::new(target("small", "m"), capable(8_192), Health::Healthy),
        Candidate::new(target("medium", "m"), capable(64_000), Health::Healthy),
        Candidate::new(target("large", "m"), capable(256_000), Health::Healthy),
    ];

    // ~10k tokens fit medium and large, not small. The too-small candidate must
    // not be chosen, and operator order keeps the first fitting one (medium).
    let fits = router
        .route(&ask(&"x".repeat(40_000)), &[], &candidates)
        .expect("routable");
    assert_eq!(fits.chosen, target("medium", "m"));
    assert!(
        !fits.fallbacks.contains(&target("small", "m")),
        "a candidate too small to fit is not a fitting fallback"
    );

    // ~275k tokens exceed every window: forward to the LARGEST one, not refuse.
    let over = router
        .route(&ask(&"x".repeat(1_100_000)), &[], &candidates)
        .expect("an over-context request is forwarded, not refused");
    assert_eq!(over.chosen, target("large", "m"));
}

#[test]
fn a_pool_naming_an_upstream_nobody_installed_says_so() {
    let refused = router()
        .route(&ask("请给出证明"), &[], &[])
        .expect_err("no candidates at all");

    assert_eq!(
        refused,
        NoRoute::Unsatisfiable {
            pool: "sota".to_owned(),
            reason: UnmetRequirement::Unknown,
        }
    );
}

// -- B-1: exact-model Agents pin the caller's model, never substitute ---------

fn exact_router() -> Router {
    let mut config = config();
    config.honor_exact_model = true;
    Router::new(config).expect("honor_exact config validates")
}

fn ask_model(model: &str, text: &str) -> ChatRequest {
    ChatRequest::new(model, vec![Message::text(Role::User, text)])
}

#[test]
fn honor_exact_serves_the_named_model_instead_of_tier_routing_it() {
    // A prompt a tier router would score into `sota` and could answer with any
    // capable model. With honor_exact on, naming the local model pins it.
    let decision = exact_router()
        .route(
            &ask_model("llama3.3", "请给出这个引理的证明"),
            &[],
            &candidates(),
        )
        .expect("the pinned model is installed");

    assert_eq!(decision.chosen.model, "llama3.3");
    assert_eq!(
        decision.decided_by,
        DecidedBy::ExactModel {
            model: "llama3.3".to_owned()
        }
    );
    assert_eq!(decision.pool, "llama3.3");
}

#[test]
fn honor_exact_refuses_rather_than_substitute_a_different_model() {
    let refused = exact_router()
        .route(
            &ask_model("gpt-6-that-nobody-has", "hi"),
            &[],
            &candidates(),
        )
        .expect_err("no candidate carries the pinned model");

    assert_eq!(
        refused,
        NoRoute::Unsatisfiable {
            pool: "gpt-6-that-nobody-has".to_owned(),
            reason: UnmetRequirement::ExactModelUnavailable,
        }
    );
    // Capability, not Capacity: another upstream would refuse for the same
    // reason, so the host must not retry it as if the model might appear.
    assert_eq!(refused.error_code(), ErrorCode::Capability);
}

#[test]
fn honor_exact_fails_over_within_the_same_model_across_providers() {
    // The same wire model carried by two providers: pinning it still allows
    // failover, but only among providers of that one model.
    let mut pool = candidates();
    pool.push(Candidate::new(
        target("openai_backup", "gpt-5.5"),
        capable(400_000),
        Health::Healthy,
    ));

    let decision = exact_router()
        .route(&ask_model("gpt-5.5", "hi"), &[], &pool)
        .expect("pinned model is installed on two providers");

    assert_eq!(decision.chosen.model, "gpt-5.5");
    // The fallback is the other provider of the SAME model, never a different one.
    assert_eq!(decision.fallbacks.len(), 1);
    assert_eq!(decision.fallbacks[0].model, "gpt-5.5");
    assert!(
        decision.fallbacks[0].upstream.as_str() != decision.chosen.upstream.as_str(),
        "failover crosses providers, not models"
    );
}

// -- P1-4: RecoveryPolicy separates tier selection from failover --------------

fn ordered_router(backup: &[&str]) -> Router {
    let mut config = config();
    config.recovery = RecoveryPolicy::Ordered {
        pools: backup.iter().map(|pool| (*pool).to_owned()).collect(),
    };
    Router::new(config).expect("ordered recovery config validates")
}

#[test]
fn strict_recovery_never_leaves_the_selected_tier() {
    // A summarize hint routes to `cheap`, whose only member is llama3.3. Under
    // the default Strict policy there is no cross-tier backup.
    let hints = [AgentHint::new(HintKind::StepType, "summarize")];
    let decision = router()
        .route(&ask("hi"), &hints, &candidates())
        .expect("routable");

    assert_eq!(decision.pool, "cheap");
    assert_eq!(decision.chosen.model, "llama3.3");
    assert!(
        decision.fallbacks.is_empty(),
        "Strict stays in its tier — no silent downgrade to another"
    );
}

#[test]
fn ordered_recovery_appends_authorized_backup_tiers_behind_the_selected_one() {
    let hints = [AgentHint::new(HintKind::StepType, "summarize")];
    let decision = ordered_router(&["sota"])
        .route(&ask("hi"), &hints, &candidates())
        .expect("routable");

    // Quality still chose the tier: the decision resolved into `cheap`.
    assert_eq!(decision.pool, "cheap");
    assert_eq!(decision.chosen.model, "llama3.3");

    // Reliability, decided separately, authorized `sota` as the backup — its
    // members trail the selected tier's as fallbacks, never as the chosen tier.
    let fallback_models: Vec<&str> = decision
        .fallbacks
        .iter()
        .map(|target| target.model.as_str())
        .collect();
    assert!(fallback_models.contains(&"gpt-5.5"));
    assert!(fallback_models.contains(&"claude-opus-4-8"));
}

#[test]
fn ordered_recovery_naming_a_missing_tier_is_refused_at_build() {
    let mut config = config();
    config.recovery = RecoveryPolicy::Ordered {
        pools: vec!["ghost".to_owned()],
    };
    assert!(
        Router::new(config).is_err(),
        "a backup tier that does not exist is a config error, like any other pool reference"
    );
}

// -- T7: local-only routing keeps the data on the machine ---------------------

#[test]
fn local_only_serves_a_local_pool() {
    // A summarize hint routes to `cheap`, whose member is the local model.
    let hints = [AgentHint::new(HintKind::StepType, "summarize")];
    let decision = Router::new(local_only_config(false))
        .expect("validates")
        .route(&ask("tl;dr this"), &hints, &candidates())
        .expect("the local pool serves a local-only route");

    assert_eq!(decision.chosen, target("ollama_local", "llama3.3"));
}

#[test]
fn local_only_without_fallback_refuses_a_cloud_only_pool() {
    // The Chinese keyword for "prove" routes to `sota`, which carries only cloud upstreams.
    // Local-only with no fallback must refuse rather than leave the machine.
    let error = Router::new(local_only_config(false))
        .expect("validates")
        .route(&ask("请给出这个命题的证明"), &[], &candidates())
        .expect_err("a cloud-only pool cannot serve a local-only route");

    assert_eq!(
        error,
        NoRoute::Unsatisfiable {
            pool: "sota".to_owned(),
            reason: UnmetRequirement::LocalOnly,
        }
    );
    assert_eq!(error.error_code(), ErrorCode::Capability);
}

#[test]
fn local_only_with_fallback_authorizes_a_cloud_pool() {
    // Same route, but the operator authorized cloud fallback: the cloud pool now
    // serves, because no local candidate could.
    let decision = Router::new(local_only_config(true))
        .expect("validates")
        .route(&ask("请给出这个命题的证明"), &[], &candidates())
        .expect("authorized fallback lets the cloud pool serve");

    assert_eq!(decision.pool, "sota");
    assert!(
        matches!(
            decision.chosen.upstream.as_str(),
            "openai_personal" | "anthropic_personal"
        ),
        "fallback served a cloud upstream: {}",
        decision.chosen.upstream.as_str()
    );
}

#[test]
fn local_only_prefers_local_then_falls_back_within_a_pool() {
    // A pool carrying both a local and a cloud model. The local one is ejected;
    // with fallback off the route is refused, and with it authorized the pool's
    // cloud member serves — the escape hatch is explicit, never silent.
    let mut base = local_only_config(false);
    base.pools.insert(
        "cheap".to_owned(),
        vec![
            target("ollama_local", "llama3.3"),
            target("openai_personal", "gpt-5.5"),
        ],
    );

    let ejected_local = vec![
        Candidate::new(
            target("ollama_local", "llama3.3"),
            capable(128_000),
            Health::Unavailable,
        )
        .local(true),
        Candidate::new(
            target("openai_personal", "gpt-5.5"),
            capable(400_000),
            Health::Healthy,
        ),
    ];
    let hints = [AgentHint::new(HintKind::StepType, "summarize")];

    // Off: the local model is ejected and the cloud one must not be used silently.
    let refused = Router::new(base.clone())
        .expect("validates")
        .route(&ask("tl;dr"), &hints, &ejected_local)
        .expect_err("local ejected and fallback off → refuse");
    assert_eq!(
        refused,
        NoRoute::Unavailable {
            pool: "cheap".to_owned(),
        }
    );

    // On: once local cannot serve, the same pool's cloud member does.
    base.allow_cloud_fallback = true;
    let decision = Router::new(base)
        .expect("validates")
        .route(&ask("tl;dr"), &hints, &ejected_local)
        .expect("local ejected and fallback on → cloud serves");
    assert_eq!(decision.chosen, target("openai_personal", "gpt-5.5"));
}

// ── Quota-first mode ──────────────────────────────────────────────────────
// The supply-side router: difficulty is ignored, accounts are ranked so the
// allowance closing soonest is spent first, a conversation stays on the account
// it warmed, and spent/ejected accounts drop out.

fn quota_router() -> Router {
    Router::new(RouterConfig {
        routing_mode: RoutingMode::QuotaFirst,
        quota: QuotaConfig::default(),
        ..config()
    })
    .expect("a quota-first config needs no pools")
}

fn quota_router_with_accounts(accounts: Vec<UpstreamModel>) -> Router {
    Router::new(RouterConfig {
        routing_mode: RoutingMode::QuotaFirst,
        quota: QuotaConfig::default(),
        quota_accounts: accounts,
        ..config()
    })
    .expect("a quota-first config needs no pools")
}

const FIVE_H_MS: u64 = 5 * 60 * 60 * 1000;

fn quota_candidate(upstream: &str, quota: QuotaState) -> Candidate {
    Candidate::new(
        target(upstream, "shared-model"),
        capable(200_000),
        Health::Healthy,
    )
    .quota(quota)
}

#[test]
fn quota_first_serves_the_soonest_resetting_account_first() {
    let accounts = vec![
        quota_candidate("plan_slow", QuotaState::open(FIVE_H_MS)),
        quota_candidate("plan_closing", QuotaState::open(20 * 60 * 1000)),
    ];
    let decision = quota_router()
        .route_quota_first(&ask("anything"), &accounts, None)
        .expect("routable");

    assert_eq!(decision.chosen, target("plan_closing", "shared-model"));
    assert_eq!(decision.decided_by, DecidedBy::Quota);
    assert_eq!(decision.pool, "quota");
    // The rest is the failover order: the slower-resetting account follows.
    assert_eq!(
        decision.fallbacks,
        vec![target("plan_slow", "shared-model")]
    );
}

#[test]
fn quota_first_restricts_to_selected_accounts_and_honors_their_order() {
    // The host supplies three candidates, but the operator selected only two, in
    // an explicit order. The unselected one never routes, and exact ties break
    // by that selection order — not by which the host happened to list first.
    let accounts = vec![
        quota_candidate("plan_a", QuotaState::open(FIVE_H_MS / 2)),
        quota_candidate("plan_b", QuotaState::open(FIVE_H_MS / 2)),
        // plan_c resets soonest — it would win if it were in the selection.
        quota_candidate("plan_c", QuotaState::open(20 * 60 * 1000)),
    ];
    let selected = vec![
        target("plan_b", "shared-model"),
        target("plan_a", "shared-model"),
    ];
    let decision = quota_router_with_accounts(selected)
        .route_quota_first(&ask("anything"), &accounts, None)
        .expect("routable");
    // Same reset, no affinity ⇒ selection order decides: plan_b (listed first).
    assert_eq!(decision.chosen, target("plan_b", "shared-model"));
    // Only the two selected accounts route; the soonest-resetting plan_c is out.
    assert_eq!(decision.fallbacks, vec![target("plan_a", "shared-model")]);
}

#[test]
fn quota_first_ignores_a_selected_account_the_host_did_not_supply() {
    // A selected (upstream, model) with no matching supplied candidate is simply
    // absent from rotation — the selection can't conjure a candidate.
    let accounts = vec![quota_candidate("plan_a", QuotaState::open(FIVE_H_MS / 2))];
    let selected = vec![
        target("plan_missing", "shared-model"),
        target("plan_a", "shared-model"),
    ];
    let decision = quota_router_with_accounts(selected)
        .route_quota_first(&ask("hi"), &accounts, None)
        .expect("routable");
    assert_eq!(decision.chosen, target("plan_a", "shared-model"));
    assert!(decision.fallbacks.is_empty());
}

#[test]
fn quota_first_keeps_a_conversation_on_the_account_it_warmed() {
    // Same reset ⇒ affinity decides: the warmed account wins even though it is
    // listed second.
    let accounts = vec![
        quota_candidate("plan_a", QuotaState::open(FIVE_H_MS / 2)),
        quota_candidate("plan_b", QuotaState::open(FIVE_H_MS / 2)),
    ];
    let warmed = target("plan_b", "shared-model");
    let decision = quota_router()
        .route_quota_first(&ask("follow-up"), &accounts, Some(&warmed))
        .expect("routable");
    assert_eq!(decision.chosen, warmed);
}

#[test]
fn quota_first_spills_off_a_warmed_but_rate_pressured_account() {
    let pressured = QuotaState {
        reset: Some(ResetWindow {
            ms_until_reset: FIVE_H_MS / 2,
            remaining_permille: 1000,
        }),
        rate_headroom_permille: 0,
        rate_pressured: true,
        exhausted: false,
    };
    let accounts = vec![
        quota_candidate("plan_warm", pressured),
        quota_candidate("plan_free", QuotaState::open(FIVE_H_MS / 2)),
    ];
    let warmed = target("plan_warm", "shared-model");
    let decision = quota_router()
        .route_quota_first(&ask("hi"), &accounts, Some(&warmed))
        .expect("routable");
    assert_eq!(
        decision.chosen,
        target("plan_free", "shared-model"),
        "a throttled warmed account yields to a free one"
    );
}

#[test]
fn quota_first_skips_a_spent_allowance() {
    let spent = QuotaState {
        reset: Some(ResetWindow {
            ms_until_reset: 60_000,
            remaining_permille: 0,
        }),
        rate_headroom_permille: 1000,
        rate_pressured: false,
        exhausted: true,
    };
    let accounts = vec![
        quota_candidate("plan_spent", spent),
        quota_candidate("plan_left", QuotaState::open(FIVE_H_MS)),
    ];
    let decision = quota_router()
        .route_quota_first(&ask("hi"), &accounts, None)
        .expect("routable");
    assert_eq!(decision.chosen, target("plan_left", "shared-model"));
    assert!(
        decision.fallbacks.is_empty(),
        "the spent account is dropped, not a fallback"
    );
}

#[test]
fn quota_first_prefers_a_windowed_account_over_pay_as_you_go() {
    let accounts = vec![
        quota_candidate("metered", QuotaState::non_windowed()),
        quota_candidate("windowed", QuotaState::open(FIVE_H_MS)),
    ];
    let decision = quota_router()
        .route_quota_first(&ask("hi"), &accounts, None)
        .expect("routable");
    assert_eq!(
        decision.chosen,
        target("windowed", "shared-model"),
        "spend the perishable windowed allowance before the always-metered one"
    );
}

#[test]
fn quota_first_reports_unavailable_when_every_account_is_ejected() {
    let accounts = vec![
        Candidate::new(
            target("a", "shared-model"),
            capable(200_000),
            Health::Unavailable,
        )
        .quota(QuotaState::open(FIVE_H_MS)),
        Candidate::new(
            target("b", "shared-model"),
            capable(200_000),
            Health::Unavailable,
        )
        .quota(QuotaState::open(FIVE_H_MS)),
    ];
    let error = quota_router()
        .route_quota_first(&ask("hi"), &accounts, None)
        .expect_err("all ejected");
    assert!(matches!(error, NoRoute::Unavailable { .. }));
}

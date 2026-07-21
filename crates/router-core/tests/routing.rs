//! The four layers, in the order they are promised to fire, and the one
//! property that outlives every request: a decision cannot carry content.

use std::collections::BTreeMap;

use token_station_protocol::{
    AgentHint, ChatRequest, ErrorCode, HintKind, Message, ModelCapability, ResponseFormat, Role,
    ToolDef,
};
use token_station_router_core::{
    Candidate, DecidedBy, Health, Heuristic, HintRoute, Match, NoRoute, Router, RouterConfig, Rule,
    UnmetRequirement, UpstreamModel, UpstreamRef, Weights,
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
                context_window: 128_000,
                ..ModelCapability::default()
            },
            Health::Healthy,
        ),
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
    // 12,000 ASCII characters ≈ 3,000 estimated tokens → 30 points, plus one
    // tool at 20 → 50, over the threshold of 40.
    let mut request = ask(&"x".repeat(12_000));
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
fn an_unavailable_pool_is_retriable_and_an_incapable_one_is_not() {
    let mut removed = candidates();
    removed[0].health = Health::Unavailable; // the only member of `cheap`

    let all_down = router()
        .route(&ask("hi"), &[], &removed)
        .expect_err("nothing left in `cheap`");
    assert_eq!(
        all_down,
        NoRoute::Unavailable {
            pool: "cheap".to_owned()
        }
    );
    assert!(all_down.error_code().is_retriable_elsewhere());

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
fn an_unreported_context_window_is_assumed_small_rather_than_unlimited() {
    let mut candidates = candidates();
    // A self-hosted upstream whose adapter could not enumerate its models.
    candidates[0].capability.context_window = 0;

    let long = "x".repeat(40_000); // ~10k estimated tokens, over the 8192 assumption
    let mut config = config();
    config.heuristic = None; // send everything to `cheap` via default_pool
    config.rules.clear();
    let router = Router::new(config).expect("valid");

    let refused = router
        .route(&ask(&long), &[], &candidates)
        .expect_err("an unknown window must not be treated as unlimited");

    assert!(matches!(
        refused,
        NoRoute::Unsatisfiable {
            reason: UnmetRequirement::ContextWindow { .. },
            ..
        }
    ));
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

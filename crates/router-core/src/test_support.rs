//! One configuration, shared by the unit tests, so a change to the routing
//! table shows up everywhere it would in a real deployment.

use std::collections::BTreeMap;

use token_station_protocol::HintKind;

use crate::config::{Heuristic, HintRoute, Match, RouterConfig, Rule, Weights};
use crate::decision::{UpstreamModel, UpstreamRef};

pub(crate) fn upstream_model(upstream: &str, model: &str) -> UpstreamModel {
    UpstreamModel::new(
        UpstreamRef::new(upstream).expect("valid upstream reference name"),
        model,
    )
}

/// A local client's routing table: a local model for the cheap work, two BYOK
/// upstreams for the rest.
pub(crate) fn config() -> RouterConfig {
    let mut pools = BTreeMap::new();
    pools.insert(
        "cheap".to_owned(),
        vec![upstream_model("ollama_local", "llama3.3")],
    );
    pools.insert(
        "sota".to_owned(),
        vec![
            upstream_model("openai_personal", "gpt-5.5"),
            upstream_model("anthropic_personal", "claude-opus-4-8"),
        ],
    );

    RouterConfig {
        version: 1,
        pools,
        rules: vec![
            Rule {
                id: "long-context".to_owned(),
                matcher: Match {
                    estimated_input_tokens_at_least: Some(32_000),
                    ..Match::default()
                },
                route_to: "sota".to_owned(),
            },
            Rule {
                id: "tools".to_owned(),
                matcher: Match {
                    requires_tools: Some(true),
                    ..Match::default()
                },
                route_to: "sota".to_owned(),
            },
        ],
        hint_routes: vec![
            HintRoute {
                kind: HintKind::StepType,
                value: "planning".to_owned(),
                route_to: "sota".to_owned(),
            },
            HintRoute {
                kind: HintKind::StepType,
                value: "summarize".to_owned(),
                route_to: "cheap".to_owned(),
            },
        ],
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
        recovery: crate::config::RecoveryPolicy::Strict,
        local_only: false,
        allow_cloud_fallback: false,
        routing_mode: crate::config::RoutingMode::Tiered,
        quota: crate::quota::QuotaConfig::default(),
        quota_accounts: Vec::new(),
    }
}

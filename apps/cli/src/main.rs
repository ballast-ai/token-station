//! Bootstrap entry point: prove the routing kernel is callable, and print why it
//! decided what it decided.
//!
//! `C1#6` builds the real management surface. What exists here is the smallest
//! thing that exercises `router-core` end to end — load a configuration through
//! a `ConfigSource`, route a few requests, explain each answer — so that the
//! crate's public API is shaped by a caller rather than only by its tests.
//!
//! Candidates are hard-coded. Until `C1#2` wires real upstreams there is no
//! health checker to ask and no provider adapter to enumerate models, and
//! inventing a config field for them now would mean deleting it then.

mod config;

use std::process::ExitCode;

use config::FileConfigSource;
use token_station_protocol::{AgentHint, ChatRequest, HintKind, Message, ModelCapability, Role};
use token_station_router_core::{
    CacheError, Candidate, ConfigCache, ConfigSource, DecidedBy, Health, StaticConfigSource,
    UpstreamModel, UpstreamRef,
};

pub(crate) const EXAMPLE_CONFIG: &str = include_str!("../example-router.json");

fn main() -> ExitCode {
    let outcome = match std::env::args().nth(1) {
        Some(path) => explain(ConfigCache::load(FileConfigSource::new(path))),
        None => match serde_json::from_str(EXAMPLE_CONFIG) {
            Ok(config) => explain(ConfigCache::load(StaticConfigSource::new(config))),
            Err(error) => Err(format!(
                "the bundled example config does not parse: {error}"
            )),
        },
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn explain<S: ConfigSource>(
    cache: Result<ConfigCache<S>, CacheError<S::Error>>,
) -> Result<(), String> {
    let cache = cache.map_err(|error| error.to_string())?;
    let router = cache.current();

    println!("pools");
    for (pool, members) in &router.config().pools {
        let listed: Vec<String> = members.iter().map(ToString::to_string).collect();
        println!("  {pool:<8} {}", listed.join(", "));
    }

    println!("\ndecisions");
    for (label, request, hints) in demo_requests() {
        match router.route(&request, &hints, &candidates()) {
            Ok(decision) => println!(
                "  {label:<22} -> {:<36} {}",
                decision.chosen.to_string(),
                because(&decision.decided_by)
            ),
            Err(refusal) => println!("  {label:<22} -> refused: {refusal}"),
        }
    }

    Ok(())
}

/// The transparency requirement, in one line: for any request, why here.
fn because(decided_by: &DecidedBy) -> String {
    match decided_by {
        DecidedBy::Rule { rule } => format!("rule `{rule}`"),
        DecidedBy::Hint { kind, value } => format!("hint {kind:?} = `{value}`"),
        DecidedBy::Heuristic { score, threshold } => {
            format!("heuristic score {score} against threshold {threshold}")
        }
        DecidedBy::Default => "the default pool".to_owned(),
    }
}

fn demo_requests() -> Vec<(&'static str, ChatRequest, Vec<AgentHint>)> {
    vec![
        ("a proof, unhinted", ask("请给出这个引理的证明"), Vec::new()),
        (
            "a summary, hinted",
            ask("summarise this changelog"),
            vec![AgentHint::new(HintKind::StepType, "summarize")],
        ),
        (
            "a short question",
            ask("what is the capital of Peru"),
            Vec::new(),
        ),
    ]
}

fn ask(text: &str) -> ChatRequest {
    ChatRequest::new("auto", vec![Message::text(Role::User, text)])
}

/// Placeholder for the upstreams `C1#2` will install and health-check.
fn candidates() -> Vec<Candidate> {
    let upstream = |name: &str| UpstreamRef::new(name).expect("valid reference name");
    let frontier = ModelCapability {
        tool: true,
        vision: true,
        json_schema: true,
        context_window: 200_000,
        ..ModelCapability::default()
    };

    vec![
        Candidate::new(
            UpstreamModel::new(upstream("ollama_local"), "llama3.3"),
            ModelCapability {
                tool: true,
                context_window: 128_000,
                ..ModelCapability::default()
            },
            Health::Healthy,
        ),
        Candidate::new(
            UpstreamModel::new(upstream("openai_personal"), "gpt-5.5"),
            ModelCapability {
                context_window: 400_000,
                ..frontier.clone()
            },
            Health::Healthy,
        ),
        Candidate::new(
            UpstreamModel::new(upstream("anthropic_personal"), "claude-opus-4-8"),
            frontier,
            Health::Healthy,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::{EXAMPLE_CONFIG, candidates, demo_requests};
    use token_station_router_core::{Router, RouterConfig};

    fn router() -> Router {
        let config: RouterConfig =
            serde_json::from_str(EXAMPLE_CONFIG).expect("the shipped example parses");
        Router::new(config).expect("the shipped example is routable")
    }

    #[test]
    fn the_shipped_example_config_routes_every_demo_request() {
        let router = router();

        for (label, request, hints) in demo_requests() {
            assert!(
                router.route(&request, &hints, &candidates()).is_ok(),
                "`{label}` could not be routed by the config we ship"
            );
        }
    }

    #[test]
    fn a_keyword_rule_beats_the_heuristic_that_would_have_gone_cheap() {
        let router = router();
        let (_, proof, hints) = demo_requests().remove(0);

        let decision = router
            .route(&proof, &hints, &candidates())
            .expect("routable");

        assert_eq!(decision.pool, "sota");
        assert!(matches!(
            decision.decided_by,
            token_station_router_core::DecidedBy::Rule { .. }
        ));
    }
}

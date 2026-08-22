//! S4 acceptance: the shadow lane observes zero divergence between the v1
//! provider plugin and the v2 south component across the entire v1 fixture
//! pack — request building, response parsing, error mapping, capabilities,
//! and streaming, chunk for chunk.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};

use token_station_cli::south_component::{
    ShadowReport, ShadowedProviderAdapter, SouthComponentAdapter, south_component_runtime,
};
use token_station_conformance::{FixturePack, ProviderAdapter, ProviderFamily};
use token_station_plugin_runtime::{NoSecrets, PluginRuntime, ProviderPlugin, RuntimeLimits};
use token_station_protocol::{ChatRequest, HttpResponseParts, ProviderConfig, StreamChunk};

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
}

fn build_wasm(plugin: &str, wasm_file_stem: &str) -> PathBuf {
    let dir = repo_root().join("plugins/official").join(plugin);
    let status = Command::new("cargo")
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        .current_dir(&dir)
        .status()
        .expect("cargo is on PATH");
    assert!(
        status.success(),
        "guest `{plugin}` must build; run `rustup target add wasm32-wasip2` if missing"
    );
    dir.join(format!(
        "target/wasm32-wasip2/release/{wasm_file_stem}.wasm"
    ))
}

fn v1_plugin() -> Arc<ProviderPlugin> {
    static PLUGIN: OnceLock<Arc<ProviderPlugin>> = OnceLock::new();
    Arc::clone(PLUGIN.get_or_init(|| {
        let wasm = std::fs::read(build_wasm(
            "provider-openai-compatible",
            "provider_openai_compatible",
        ))
        .expect("v1 wasm reads");
        let manifest = std::fs::read_to_string(
            repo_root().join("plugins/official/provider-openai-compatible/manifest.json"),
        )
        .expect("v1 manifest reads");
        let runtime = PluginRuntime::new(RuntimeLimits::default()).expect("v1 engine builds");
        Arc::new(
            ProviderPlugin::load_embedded(&runtime, &manifest, &wasm, NoSecrets)
                .expect("the official v1 package loads"),
        )
    }))
}

fn v2_component() -> Arc<SouthComponentAdapter> {
    static COMPONENT: OnceLock<Arc<SouthComponentAdapter>> = OnceLock::new();
    Arc::clone(COMPONENT.get_or_init(|| {
        let wasm = std::fs::read(build_wasm(
            "provider-openai-compatible-v2",
            "provider_openai_compatible_v2",
        ))
        .expect("v2 wasm reads");
        let manifest = std::fs::read_to_string(
            repo_root().join("plugins/official/provider-openai-compatible-v2/manifest.json"),
        )
        .expect("v2 manifest reads");
        let runtime = south_component_runtime().expect("south engine builds");
        Arc::new(
            SouthComponentAdapter::load_embedded(&runtime, &manifest, &wasm)
                .expect("the official v2 package passes every south gate"),
        )
    }))
}

fn shadowed() -> (ShadowedProviderAdapter, Arc<ShadowReport>) {
    let report = Arc::new(ShadowReport::default());
    let adapter = ShadowedProviderAdapter::new(
        v1_plugin(),
        v2_component(),
        Arc::clone(&report),
        "openai-compatible",
    );
    (adapter, report)
}

fn fixture_pack() -> FixturePack<ProviderFamily> {
    FixturePack::load(&repo_root().join("plugins/official/provider-openai-compatible/fixtures"))
        .expect("the v1 fixture pack loads")
}

#[derive(serde::Deserialize)]
struct RequestInput {
    provider_config: ProviderConfig,
    chat_request: ChatRequest,
}

#[derive(serde::Deserialize)]
struct StreamInput {
    chunks: Vec<String>,
}

/// Every fixture case through both lanes; the report must show zero
/// divergence and every comparison actually happened.
#[test]
fn the_shadow_lane_observes_zero_divergence_across_the_v1_fixture_pack() {
    let (adapter, report) = shadowed();

    for case in fixture_pack().cases() {
        match case.family {
            ProviderFamily::Capabilities => {
                let config: ProviderConfig =
                    serde_json::from_value(case.input.clone()).expect("fixture config");
                let _ = adapter.model_capabilities(&config);
            }
            ProviderFamily::Request => {
                let input: RequestInput =
                    serde_json::from_value(case.input.clone()).expect("fixture request");
                let _ = adapter.build_http_request(&input.chat_request, &input.provider_config);
            }
            ProviderFamily::Response => {
                let parts: HttpResponseParts =
                    serde_json::from_value(case.input.clone()).expect("fixture parts");
                let _ = adapter.parse_response(&parts);
            }
            ProviderFamily::Error => {
                let parts: HttpResponseParts =
                    serde_json::from_value(case.input.clone()).expect("fixture parts");
                let _ = adapter.map_provider_error(&parts);
            }
            ProviderFamily::Stream => {
                let input: StreamInput =
                    serde_json::from_value(case.input.clone()).expect("fixture chunks");
                let mut parser = adapter.stream_parser();
                for chunk in &input.chunks {
                    let _ = parser.parse_chunk(&StreamChunk {
                        data: chunk.clone(),
                    });
                }
                // EOF flush, compared like any chunk.
                let _ = parser.parse_chunk(&StreamChunk {
                    data: String::new(),
                });
            }
        }
    }

    let comparisons = report.comparisons.load(Ordering::Relaxed);
    let divergences = report.divergences.load(Ordering::Relaxed);
    assert!(
        comparisons >= 9,
        "every fixture family must have been compared: {comparisons}"
    );
    assert_eq!(
        divergences, 0,
        "the shadow lane observed {divergences} divergences"
    );
}

/// The serving lane's answers are the v1 plugin's, byte for byte — shadowing
/// must never change what the caller sees.
#[test]
fn the_shadow_lane_serves_the_v1_result() {
    let (adapter, _) = shadowed();
    let v1 = v1_plugin();

    for case in fixture_pack().cases() {
        if case.family != ProviderFamily::Request {
            continue;
        }
        let input: RequestInput =
            serde_json::from_value(case.input.clone()).expect("fixture request");
        let served = adapter
            .build_http_request(&input.chat_request, &input.provider_config)
            .expect("fixture request builds");
        let direct = v1
            .build_http_request(&input.chat_request, &input.provider_config)
            .expect("fixture request builds");
        assert_eq!(
            serde_json::to_value(&served).expect("serializable"),
            serde_json::to_value(&direct).expect("serializable"),
        );
    }
}

/// The config surface: shadow is the default, the other modes round-trip,
/// and an unknown mode is refused rather than coerced.
#[test]
fn south_component_mode_defaults_to_shadow_and_round_trips() {
    use token_station_cli::config::SouthComponentMode;

    let default: SouthComponentMode =
        serde_json::from_value(serde_json::json!("shadow")).expect("shadow parses");
    assert_eq!(default, SouthComponentMode::default());

    for (text, mode) in [
        ("off", SouthComponentMode::Off),
        ("shadow", SouthComponentMode::Shadow),
        ("primary", SouthComponentMode::Primary),
    ] {
        let parsed: SouthComponentMode =
            serde_json::from_value(serde_json::json!(text)).expect("mode parses");
        assert_eq!(parsed, mode);
        assert_eq!(
            serde_json::to_value(mode).expect("serializes"),
            serde_json::json!(text)
        );
    }

    let unknown: Result<SouthComponentMode, _> =
        serde_json::from_value(serde_json::json!("future_mode"));
    assert!(unknown.is_err(), "an unknown mode must be refused");
}

/// Stage C: the Anthropic component loads through the same gates and answers
/// for its own dialect.
///
/// The point is not that a second package exists — it is that adding one was a
/// packaging change rather than a wiring one. The host resolves a component by
/// the dialect its manifest declares, the rule the v1 registry has always used,
/// so `provider-anthropic` claims `anthropic` and nothing else moved.
#[test]
fn the_anthropic_component_loads_and_claims_its_own_dialect() {
    let dir = repo_root().join("plugins/official/provider-anthropic-v2");
    let wasm = std::fs::read(build_wasm("provider-anthropic-v2", "provider_anthropic_v2"))
        .expect("the anthropic component builds");
    let manifest = std::fs::read_to_string(dir.join("manifest.json")).expect("its manifest reads");
    let runtime = south_component_runtime().expect("the south runtime builds");
    let adapter = SouthComponentAdapter::load_embedded(&runtime, &manifest, &wasm)
        .expect("the anthropic component passes every load gate");

    assert_eq!(adapter.package_name(), "provider-anthropic");
    assert_eq!(adapter.dialects(), vec!["anthropic".to_owned()]);
    assert!(
        !adapter.dialects().iter().any(|d| d == "openai-compatible"),
        "the two official components must not both answer for one dialect"
    );
}

/// And it translates: an Anthropic upstream now has a renderer, which is the
/// whole point of stage C. Before this package existed, the only southbound
/// renderer was the OpenAI-compatible one, which is why the northbound adapter
/// had to refuse shapes it could not lower and why the verbatim passthrough
/// existed to carry them.
#[test]
fn the_anthropic_component_renders_a_messages_request() {
    let wasm = std::fs::read(build_wasm("provider-anthropic-v2", "provider_anthropic_v2"))
        .expect("the anthropic component builds");
    let manifest = std::fs::read_to_string(
        repo_root().join("plugins/official/provider-anthropic-v2/manifest.json"),
    )
    .expect("its manifest reads");
    let runtime = south_component_runtime().expect("the south runtime builds");
    let adapter = SouthComponentAdapter::load_embedded(&runtime, &manifest, &wasm)
        .expect("the anthropic component loads");

    let request: ChatRequest = serde_json::from_value(serde_json::json!({
        "model": "claude-sonnet-4-5",
        "messages": [{"role": "user", "content": "hello"}],
        "sampling": {"max_output_tokens": 1024}
    }))
    .expect("a minimal IR request");
    let config: ProviderConfig = serde_json::from_value(serde_json::json!({
        "provider": "anthropic",
        "base_url": "https://api.anthropic.com/v1"
    }))
    .expect("an anthropic provider config");

    let descriptor = adapter
        .build_http_request(&request, &config)
        .expect("the component renders the request");
    let body = descriptor.body.expect("the descriptor carries a body");
    assert_eq!(body["model"], "claude-sonnet-4-5");
    assert_eq!(body["messages"][0]["role"], "user");
    assert!(
        descriptor.url.ends_with("/messages"),
        "an Anthropic request addresses /messages: {}",
        descriptor.url
    );
}

/// Stage C's shadow classifies without serving. The three outcomes answer
/// different questions and are counted apart on purpose: a divergence asks
/// whether the round trip is faithful, a refusal asks whether the translated
/// route can carry the request at all — and it is the refusal count that tells
/// stage A′ which rejections are worth opening.
#[test]
fn the_passthrough_shadow_separates_refusals_from_divergences() {
    use std::sync::atomic::Ordering as AtomicOrdering;
    use token_station_cli::south_component::{PassthroughShadow, PassthroughShadowReport};

    let wasm = std::fs::read(build_wasm("provider-anthropic-v2", "provider_anthropic_v2"))
        .expect("the anthropic component builds");
    let manifest = std::fs::read_to_string(
        repo_root().join("plugins/official/provider-anthropic-v2/manifest.json"),
    )
    .expect("its manifest reads");
    let runtime = south_component_runtime().expect("the south runtime builds");
    let component = Arc::new(
        SouthComponentAdapter::load_embedded(&runtime, &manifest, &wasm)
            .expect("the anthropic component loads"),
    );
    let report = Arc::new(PassthroughShadowReport::default());
    let shadow = PassthroughShadow::new(Arc::clone(&component), Arc::clone(&report));

    let config: ProviderConfig = serde_json::from_value(serde_json::json!({
        "provider": "anthropic",
        "base_url": "https://api.anthropic.com/v1"
    }))
    .expect("an anthropic provider config");
    let request: ChatRequest = serde_json::from_value(serde_json::json!({
        "model": "claude-sonnet-4-5",
        "messages": [{"role": "user", "content": "hello"}],
        "sampling": {"max_output_tokens": 1024}
    }))
    .expect("a minimal IR request");

    // A refusal is not a diff. The translated route declining a shape the
    // passthrough serves today is exactly what stage A′ needs counted.
    let refusing = |_: &serde_json::Value| {
        Err(token_station_protocol::ErrorEnvelope::new(
            token_station_protocol::ErrorCode::Capability,
            400,
            "server-tool history cannot be replayed",
        ))
    };
    shadow.observe(
        &refusing,
        &serde_json::json!({}),
        &config,
        "claude-sonnet-4-5",
    );
    assert_eq!(report.refused.load(AtomicOrdering::Relaxed), 1);
    assert_eq!(report.divergent.load(AtomicOrdering::Relaxed), 0);

    // Feeding back the component's own rendering is the one input guaranteed to
    // round-trip, so it pins the equality arm rather than the differing one.
    let rendered = component
        .build_http_request(&request, &config)
        .expect("the component renders")
        .body
        .expect("with a body");
    let accepting = |_: &serde_json::Value| Ok(request.clone());
    shadow.observe(&accepting, &rendered, &config, "claude-sonnet-4-5");
    assert_eq!(report.equivalent.load(AtomicOrdering::Relaxed), 1);

    // And a body the round trip does not reproduce lands as a divergence.
    shadow.observe(
        &accepting,
        &serde_json::json!({"model": "claude-sonnet-4-5", "messages": []}),
        &config,
        "claude-sonnet-4-5",
    );
    assert_eq!(report.divergent.load(AtomicOrdering::Relaxed), 1);
    assert_eq!(report.refused.load(AtomicOrdering::Relaxed), 1);
}

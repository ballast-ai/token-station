//! The South provider components through this host's seam: each official
//! component loads through every South gate, claims exactly the dialect its
//! manifest declares, and renders a Canonical IR request onto its wire.

use std::path::{Path, PathBuf};
use std::process::Command;

use token_station_cli::south_component::{
    ProviderAdapter, SouthComponentAdapter, south_component_runtime,
};
use token_station_protocol::{ChatRequest, ProviderConfig};

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

// -- compatibility admission (P1) ---------------------------------------------
//
// South 0.16.0 made the tuple handshake a required loader input, and this host
// declares the one expectation every component is judged against. Until that
// release the tuple was declared in every manifest and compared nowhere: these
// tests exist so "a stale package is refused" stops being a claim about a field
// and becomes a claim about behaviour.

/// Each of the four host-known fields, tampered one at a time on a package that
/// is otherwise exactly the one that ships. All four must be refused.
#[test]
fn a_component_disagreeing_on_any_tuple_field_is_refused() {
    let wasm = std::fs::read(build_wasm("provider-anthropic-v2", "provider_anthropic_v2"))
        .expect("the anthropic component builds");
    let shipped: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            repo_root().join("plugins/official/provider-anthropic-v2/manifest.json"),
        )
        .expect("its manifest reads"),
    )
    .expect("the manifest is JSON");
    let runtime = south_component_runtime().expect("the south runtime builds");

    // Shape-valid but different. A malformed value would be refused earlier, by
    // the manifest schema gate, and would prove nothing about the handshake.
    for (field, other) in [
        ("ir_schema_id", "token-station-protocol@0.4.0/v0.2.0"),
        ("kernel_version", "0.3.0"),
        (
            "kernel_revision",
            "0123456789abcdef0123456789abcdef01234567",
        ),
        ("south_runtime", "0.15.0"),
    ] {
        let mut tampered = shipped.clone();
        tampered["compatibility"][field] = serde_json::json!(other);
        let refused = SouthComponentAdapter::load_embedded(&runtime, &tampered.to_string(), &wasm);
        let Err(message) = refused else {
            panic!("a component disagreeing on `{field}` must not be admitted");
        };
        assert!(
            message.contains("not compatible"),
            "the refusal must name the handshake, not something downstream: {message}"
        );
    }
}

/// The shipped package is admitted. Without this the test above would pass on a
/// host that refuses everything.
#[test]
fn the_shipped_component_satisfies_this_hosts_expectations() {
    let wasm = std::fs::read(build_wasm("provider-anthropic-v2", "provider_anthropic_v2"))
        .expect("the anthropic component builds");
    let manifest = std::fs::read_to_string(
        repo_root().join("plugins/official/provider-anthropic-v2/manifest.json"),
    )
    .expect("its manifest reads");
    let runtime = south_component_runtime().expect("the south runtime builds");
    SouthComponentAdapter::load_embedded(&runtime, &manifest, &wasm)
        .expect("the package this host ships must satisfy the host's own expectations");
}

/// A package built for the previous South release is refused by name. This is
/// the case the tuple exists for.
#[test]
fn a_component_from_the_previous_south_release_is_refused() {
    let wasm = std::fs::read(build_wasm("provider-anthropic-v2", "provider_anthropic_v2"))
        .expect("the anthropic component builds");
    let mut stale: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            repo_root().join("plugins/official/provider-anthropic-v2/manifest.json"),
        )
        .expect("its manifest reads"),
    )
    .expect("the manifest is JSON");
    stale["compatibility"]["south_runtime"] = serde_json::json!("0.15.0");
    let runtime = south_component_runtime().expect("the south runtime builds");

    let Err(message) = SouthComponentAdapter::load_embedded(&runtime, &stale.to_string(), &wasm)
    else {
        panic!("a component verified against South 0.15.0 must not load here");
    };
    assert!(
        message.contains("0.15.0"),
        "the refusal must name the stale version so an operator can act on it: {message}"
    );
}

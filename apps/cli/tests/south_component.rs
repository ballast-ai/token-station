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

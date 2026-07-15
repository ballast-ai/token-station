//! A2's exit criterion, executed: the official OpenAI adapters, compiled to
//! real `wasm32-wasip2` components, loaded through every gate, and held to the
//! full conformance suite — the same suite, through the same trait, that judges
//! native adapters.
//!
//! This is also the whole install pipeline in miniature: manifest gate, import
//! scan, identity gate, then the fixture suite. A package that failed any of
//! these would be a draft in the registry, not a loadable plugin.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use token_station_conformance::{
    AgentAdapter, AgentFamily, FixturePack, ProviderFamily, run_agent_suite, run_provider_suite,
};
use token_station_plugin_runtime::{
    AgentPlugin, LoadError, NoSecrets, PluginRuntime, ProviderPlugin, RuntimeLimits,
};
use token_station_protocol::{ErrorCode, ErrorEnvelope, FinishReason, StreamEvent};

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/plugin-runtime sits two levels below the root")
}

/// Builds one official plugin and assembles its package directory next to a
/// copy of its real manifest.
fn build_package(plugin: &str) -> PathBuf {
    let source_dir = repo_root().join("plugins/official").join(plugin);

    let status = Command::new("cargo")
        .args(["build", "--target", "wasm32-wasip2"])
        .current_dir(&source_dir)
        .status()
        .expect("cargo is on PATH");
    assert!(
        status.success(),
        "{plugin} must build; run `rustup target add wasm32-wasip2` if the target is missing"
    );

    let wasm = source_dir
        .join("target/wasm32-wasip2/debug")
        .join(format!("{}.wasm", plugin.replace('-', "_")));

    let package = std::env::temp_dir().join(format!("ts-official-{}-{plugin}", std::process::id()));
    std::fs::create_dir_all(&package).expect("temp dir is writable");
    std::fs::copy(
        source_dir.join("manifest.json"),
        package.join("manifest.json"),
    )
    .expect("manifest copies");
    std::fs::copy(&wasm, package.join("adapter.wasm")).expect("wasm copies");
    package
}

fn provider_package() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| build_package("provider-openai-compatible"))
}

fn agent_package() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| build_package("agent-openai"))
}

fn anthropic_agent_package() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| build_package("agent-anthropic"))
}

fn runtime() -> PluginRuntime {
    PluginRuntime::new(RuntimeLimits {
        memory_bytes: 64 * 1024 * 1024,
        // Generous: the incrementality check replays the stream fixture at
        // every byte boundary, and CI machines are slow.
        call_timeout: Duration::from_secs(5),
    })
    .expect("engine builds")
}

#[test]
fn the_official_provider_adapter_passes_the_full_suite_as_wasm() {
    let plugin =
        ProviderPlugin::load(&runtime(), provider_package(), NoSecrets).expect("loads clean");

    let fixtures = repo_root().join("plugins/official/provider-openai-compatible/fixtures");
    let pack: FixturePack<ProviderFamily> =
        FixturePack::load(&fixtures).expect("the shipped pack loads");

    let report = run_provider_suite(&plugin, &pack);

    assert!(report.is_passing(), "{report}");
    assert_eq!(report.suite(), plugin.manifest().conformance.required_suite);
}

#[test]
fn the_official_agent_adapter_passes_the_full_suite_as_wasm() {
    let plugin = AgentPlugin::load(&runtime(), agent_package()).expect("loads clean");

    let fixtures = repo_root().join("plugins/official/agent-openai/fixtures");
    let pack: FixturePack<AgentFamily> =
        FixturePack::load(&fixtures).expect("the shipped pack loads");

    let report = run_agent_suite(&plugin, &pack);

    assert!(report.is_passing(), "{report}");
    assert_eq!(report.suite(), plugin.manifest().conformance.required_suite);
}

#[test]
fn the_official_anthropic_agent_adapter_passes_the_full_suite_as_wasm() {
    let plugin = AgentPlugin::load(&runtime(), anthropic_agent_package()).expect("loads clean");

    let fixtures = repo_root().join("plugins/official/agent-anthropic/fixtures");
    let pack: FixturePack<AgentFamily> =
        FixturePack::load(&fixtures).expect("the shipped pack loads");

    let report = run_agent_suite(&plugin, &pack);

    assert!(report.is_passing(), "{report}");
    assert_eq!(report.suite(), plugin.manifest().conformance.required_suite);
}

#[test]
fn anthropic_stream_state_is_isolated_and_cleaned_by_stream_id() {
    let plugin = AgentPlugin::load(&runtime(), anthropic_agent_package()).expect("loads clean");
    let context = |stream_id: &str, response_id: &str| {
        serde_json::json!({
            "protocol": "anthropic-messages",
            "stream_id": stream_id,
            "response_id": response_id,
            "model": "routed-model"
        })
    };
    let delta = |content: &str| StreamEvent::Delta {
        index: 0,
        content: content.to_owned(),
    };

    let a_first = plugin
        .render_stream_event(&delta("a1"), &context("stream-a", "msg-a"))
        .expect("stream A starts");
    let b_first = plugin
        .render_stream_event(&delta("b1"), &context("stream-b", "msg-b"))
        .expect("stream B starts");
    let a_second = plugin
        .render_stream_event(&delta("a2"), &context("stream-a", "msg-a"))
        .expect("stream A resumes");

    assert!(a_first["data"].as_str().unwrap().contains("message_start"));
    assert!(b_first["data"].as_str().unwrap().contains("message_start"));
    assert!(!a_second["data"].as_str().unwrap().contains("message_start"));
    assert!(a_second["data"].as_str().unwrap().contains("a2"));

    plugin
        .render_stream_event(
            &StreamEvent::Done {
                finish_reason: Some(FinishReason::Stop),
            },
            &context("stream-a", "msg-a"),
        )
        .expect("done cleans stream A");
    plugin
        .render_stream_event(
            &StreamEvent::Error {
                error: ErrorEnvelope::new(ErrorCode::UpstreamUnavailable, 502, "unavailable"),
            },
            &context("stream-b", "msg-b"),
        )
        .expect("error cleans stream B");

    let a_restarted = plugin
        .render_stream_event(&delta("a3"), &context("stream-a", "msg-a-2"))
        .expect("stream A can restart after done");
    let b_restarted = plugin
        .render_stream_event(&delta("b2"), &context("stream-b", "msg-b-2"))
        .expect("stream B can restart after error");
    assert!(
        a_restarted["data"]
            .as_str()
            .unwrap()
            .contains("message_start")
    );
    assert!(
        b_restarted["data"]
            .as_str()
            .unwrap()
            .contains("message_start")
    );
}

#[test]
fn the_two_kinds_do_not_load_through_each_other() {
    // An agent package through the provider loader and vice versa: refused at
    // the kind check, before any wasm is instantiated.
    let wrong = ProviderPlugin::load(&runtime(), agent_package(), NoSecrets);
    assert!(matches!(wrong, Err(LoadError::WrongKind(_))), "{wrong:?}");

    let wrong = AgentPlugin::load(&runtime(), provider_package());
    assert!(matches!(wrong, Err(LoadError::WrongKind(_))), "{wrong:?}");
}

#[test]
fn an_agent_component_that_asks_to_name_credentials_is_refused() {
    // The provider world may import `token-station:adapter/host`; the agent
    // world may not — an agent adapter cannot even *name* a credential. The
    // WIT declares that; this proves the loader enforces it on a compiled
    // artifact that ignored the declaration.
    let wat = r#"(component
        (import "token-station:adapter/host@1.0.0" (instance))
    )"#;
    let package =
        std::env::temp_dir().join(format!("ts-official-{}-host-grab", std::process::id()));
    std::fs::create_dir_all(&package).expect("temp dir is writable");
    std::fs::copy(
        repo_root().join("plugins/official/agent-openai/manifest.json"),
        package.join("manifest.json"),
    )
    .expect("manifest copies");
    std::fs::write(package.join("adapter.wasm"), wat).expect("wat writes");

    let refused = AgentPlugin::load(&runtime(), &package);

    match refused {
        Err(LoadError::ForbiddenImport(name)) => {
            assert!(name.starts_with("token-station:adapter/host"), "{name}");
        }
        other => panic!("expected a forbidden import, got {other:?}"),
    }
}

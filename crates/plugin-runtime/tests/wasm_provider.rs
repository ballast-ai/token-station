//! The load gates and the sandbox, exercised against a real `wasm32-wasip2`
//! component.
//!
//! The guest (`tests/guests/test-provider`) is an honest small provider
//! adapter that turns hostile on demand: magic keys in its inputs make it
//! hang, allocate without bound, panic, or ask the host to sign with names it
//! did not declare. Every limit test here drives a *real* misbehaviour through
//! the *real* runtime — no mock trap, no simulated deadline.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::json;
use token_station_conformance::ProviderAdapter;
use token_station_plugin_runtime::{
    LoadError, PluginRuntime, ProviderPlugin, RuntimeLimits, SecretSigner,
};
use token_station_protocol::{ErrorCode, ProviderConfig, StreamChunk, StreamEvent};

/// Builds the guest once per test process and returns the component's path.
fn guest_wasm() -> &'static Path {
    static WASM: OnceLock<PathBuf> = OnceLock::new();
    WASM.get_or_init(|| {
        let guest_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guests/test-provider");
        let status = Command::new("cargo")
            .args(["build", "--target", "wasm32-wasip2"])
            .current_dir(&guest_dir)
            .status()
            .expect("cargo is on PATH");
        assert!(
            status.success(),
            "the guest must build; run `rustup target add wasm32-wasip2` if the target is missing"
        );
        guest_dir.join("target/wasm32-wasip2/debug/test_provider.wasm")
    })
}

fn manifest_json(version: &str) -> String {
    json!({
        "name": "test-provider",
        "version": version,
        "kind": "provider-adapter",
        "api_version": "provider-adapter-v1",
        "providers": ["test"],
        "capabilities": ["chat", "stream"],
        "permissions": { "network": false, "filesystem": false, "secrets": ["provider_api_key"] },
        "conformance": { "required_suite": "provider-protocol-v1", "fixtures": "fixtures/" }
    })
    .to_string()
}

/// Assembles a plugin package directory: `manifest.json` next to `adapter.wasm`.
///
/// Unique per call, not per name: tests run in parallel, and two tests
/// assembling the same directory would read each other's half-copied wasm.
fn package(name: &str, manifest: &str, wasm: Option<&Path>) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ts-plugin-{}-{seq}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir is writable");
    std::fs::write(dir.join("manifest.json"), manifest).expect("manifest writes");
    if let Some(wasm) = wasm {
        std::fs::copy(wasm, dir.join("adapter.wasm")).expect("wasm copies");
    }
    dir
}

fn runtime() -> PluginRuntime {
    PluginRuntime::new(RuntimeLimits {
        memory_bytes: 64 * 1024 * 1024,
        call_timeout: Duration::from_millis(500),
    })
    .expect("engine builds")
}

struct FixedSigner;

impl SecretSigner for FixedSigner {
    fn sign(&self, _: &str, _: &[u8], _: &str) -> Result<Vec<u8>, String> {
        Ok(vec![0xAB; 32])
    }
}

fn config(extra: &serde_json::Value) -> ProviderConfig {
    let mut base = json!({
        "provider": "test",
        "base_url": "https://api.test.example/v1",
        "auth": "provider_api_key",
        "models": [{ "model": "test-1", "tool": true, "context_window": 8192 }],
    });
    base.as_object_mut()
        .expect("object")
        .extend(extra.as_object().cloned().unwrap_or_default());
    serde_json::from_value(base).expect("valid config")
}

fn load() -> ProviderPlugin {
    let dir = package("ok", &manifest_json("1.0.0"), Some(guest_wasm()));
    ProviderPlugin::load(&runtime(), &dir, FixedSigner).expect("the honest package loads")
}

// -- gates ---------------------------------------------------------------------

#[test]
fn a_faithful_package_passes_every_gate_and_translates() {
    let plugin = load();

    assert_eq!(plugin.metadata().name, "test-provider");

    let capabilities = plugin
        .model_capabilities(&config(&json!({})))
        .expect("capabilities");
    assert_eq!(capabilities.len(), 1);
    assert_eq!(capabilities[0].model, "test-1");
}

#[test]
fn the_manifest_gate_runs_before_any_code_is_read() {
    let mut manifest: serde_json::Value =
        serde_json::from_str(&manifest_json("1.0.0")).expect("valid json");
    manifest["permissions"]["network"] = json!(true);

    // No adapter.wasm in the package at all: if the manifest gate is really
    // first, the loader never notices.
    let dir = package("network", &manifest.to_string(), None);
    let refused = ProviderPlugin::load(&runtime(), &dir, FixedSigner);

    assert!(
        matches!(refused, Err(LoadError::Manifest(_))),
        "got {refused:?}"
    );
}

#[test]
fn an_agent_manifest_cannot_load_as_a_provider() {
    let manifest = json!({
        "name": "test-agent",
        "version": "1.0.0",
        "kind": "agent-adapter",
        "api_version": "agent-adapter-v1",
        "agent_protocols": ["openai-chat-completions"],
        "capabilities": ["chat"],
        "permissions": { "network": false, "filesystem": false, "secrets": [] },
        "conformance": { "required_suite": "agent-protocol-v1", "fixtures": "fixtures/" }
    });
    let dir = package("wrong-kind", &manifest.to_string(), Some(guest_wasm()));

    let refused = ProviderPlugin::load(&runtime(), &dir, FixedSigner);

    assert!(
        matches!(refused, Err(LoadError::WrongKind(_))),
        "got {refused:?}"
    );
}

#[test]
fn the_identity_gate_refuses_a_package_that_lies_about_its_version() {
    // Same wasm, same name, but the manifest claims 9.9.9. This is the
    // repackaged-around-a-signature case.
    let dir = package("liar", &manifest_json("9.9.9"), Some(guest_wasm()));

    let refused = ProviderPlugin::load(&runtime(), &dir, FixedSigner);

    match refused {
        Err(LoadError::IdentityMismatch { declared, reported }) => {
            assert_eq!(declared.version, "9.9.9");
            assert_eq!(reported.version, "1.0.0");
        }
        other => panic!("expected an identity mismatch, got {other:?}"),
    }
}

#[test]
fn a_component_that_asks_for_the_network_is_refused_by_name() {
    // A component that imports wasi:sockets. Hand-written, because no honest
    // build of an adapter produces one — which is the point.
    let wat = r#"(component
        (import "wasi:sockets/instance-network@0.2.0" (instance))
    )"#;
    let dir = package("sockets", &manifest_json("1.0.0"), None);
    std::fs::write(dir.join("adapter.wasm"), wat).expect("wat writes");

    let refused = ProviderPlugin::load(&runtime(), &dir, FixedSigner);

    match refused {
        Err(LoadError::ForbiddenImport(name)) => {
            assert!(name.starts_with("wasi:sockets/"), "{name}");
        }
        other => panic!("expected a forbidden import, got {other:?}"),
    }
}

// -- sandbox -------------------------------------------------------------------

#[test]
fn a_hung_guest_is_cut_off_at_the_deadline_not_at_infinity() {
    let plugin = load();

    let started = Instant::now();
    let refused = plugin
        .model_capabilities(&config(&json!({ "__hang": true })))
        .expect_err("an infinite loop must not return");
    let elapsed = started.elapsed();

    assert_eq!(refused.code, ErrorCode::Internal);
    assert!(
        refused.message.contains("did not answer"),
        "{}",
        refused.message
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "the deadline is 500ms; {elapsed:?} means the epoch never fired"
    );

    // The instance trapped, but the *plugin* must survive: the next call gets
    // a fresh deadline and, if the store is poisoned, a fresh error — not a
    // panic in the host.
    let _ = plugin.model_capabilities(&config(&json!({})));
}

#[test]
fn a_guest_that_allocates_past_the_limit_traps_instead_of_pressuring_the_host() {
    let plugin = load();

    let refused = plugin
        .model_capabilities(&config(&json!({ "__grow_mb": 256 })))
        .expect_err("256MB against a 64MB limit must fail");

    assert_eq!(refused.code, ErrorCode::Internal);
    assert!(
        refused.message.contains("did not answer"),
        "{}",
        refused.message
    );
}

#[test]
fn a_panicking_guest_becomes_an_error_envelope_not_a_host_panic() {
    let plugin = load();

    let refused = plugin
        .model_capabilities(&config(&json!({ "__panic": true })))
        .expect_err("a guest panic is a trap");

    assert_eq!(refused.code, ErrorCode::Internal);
}

// -- credentials -----------------------------------------------------------------

#[test]
fn a_declared_secret_reaches_the_signer_and_an_undeclared_one_never_does() {
    struct Tattletale;
    impl SecretSigner for Tattletale {
        fn sign(&self, secret_ref: &str, _: &[u8], _: &str) -> Result<Vec<u8>, String> {
            // The runtime promises the signer never sees an undeclared name.
            assert_eq!(secret_ref, "provider_api_key", "manifest boundary breached");
            Ok(vec![1, 2, 3])
        }
    }

    let dir = package("signing", &manifest_json("1.0.0"), Some(guest_wasm()));
    let plugin =
        ProviderPlugin::load(&runtime(), &dir, Tattletale).expect("the honest package loads");

    let request = |secret: &str| {
        serde_json::from_value(json!({
            "model": "test-1",
            "messages": [{ "role": "user", "content": "hi" }],
            "__sign": { "secret": secret, "algorithm": "hmac-sha256" },
        }))
        .expect("valid request")
    };

    let signed = plugin
        .build_http_request(&request("provider_api_key"), &config(&json!({})))
        .expect("a declared secret signs");
    assert_eq!(signed.extensions["signature_bytes"], json!(3));

    let refused = plugin
        .build_http_request(&request("someone_elses_key"), &config(&json!({})))
        .expect_err("an undeclared secret is refused before the signer sees it");
    assert!(
        refused.message.contains("not declared"),
        "{}",
        refused.message
    );
}

// -- streams ---------------------------------------------------------------------

#[test]
fn each_stream_gets_its_own_instance_so_buffers_cannot_interleave() {
    let plugin = load();

    let mut left = plugin.stream_parser();
    let mut right = plugin.stream_parser();

    let half = |data: &str| StreamChunk {
        data: data.to_owned(),
    };

    // Each parser receives half a frame. If they shared the guest's buffer,
    // the halves would concatenate and one of them would emit a mangled event.
    let none_yet = left.parse_chunk(&half("data: from-left")).expect("parses");
    assert!(none_yet.is_empty(), "half a frame is not an event");
    let none_yet = right
        .parse_chunk(&half("data: from-right"))
        .expect("parses");
    assert!(none_yet.is_empty());

    let finished = left.parse_chunk(&half("\n\n")).expect("parses");
    assert_eq!(
        finished,
        vec![StreamEvent::Delta {
            index: 0,
            content: "from-left".to_owned()
        }],
        "the left stream must complete with only its own bytes"
    );

    let finished = right.parse_chunk(&half("\n\n")).expect("parses");
    assert_eq!(
        finished,
        vec![StreamEvent::Delta {
            index: 0,
            content: "from-right".to_owned()
        }]
    );
}

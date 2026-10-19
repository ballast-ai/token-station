use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use south_provider_runtime::{ComponentRuntimeV1, RuntimeLimitsV1};
use token_station_cli::south_component::{ProviderAdapter, SouthComponentAdapter};
use token_station_protocol::{
    ChatRequest, ErrorCode, HttpResponseParts, ProviderConfig, StreamChunk, StreamEvent,
};

fn package() -> &'static (String, Vec<u8>) {
    static PACKAGE: OnceLock<(String, Vec<u8>)> = OnceLock::new();
    PACKAGE.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("repository root")
            .join("plugins/official/provider-codeassist-v2");
        let status = Command::new("cargo")
            .args([
                "build",
                "--locked",
                "--offline",
                "--release",
                "--target",
                "wasm32-wasip2",
            ])
            .current_dir(&root)
            .status()
            .expect("cargo runs");
        assert!(status.success(), "Code Assist component builds");
        (
            std::fs::read_to_string(root.join("manifest.json")).expect("manifest reads"),
            std::fs::read(root.join("target/wasm32-wasip2/release/provider_codeassist_v2.wasm"))
                .expect("component reads"),
        )
    })
}

fn config() -> ProviderConfig {
    serde_json::from_value(json!({"provider":"gemini-cli-plan","base_url":"https://cloudcode-pa.googleapis.com","auth":"local_subscription_token","google_project":"fixture"})).unwrap()
}

fn recovery_runtime() -> ComponentRuntimeV1 {
    ComponentRuntimeV1::new(RuntimeLimitsV1 {
        memory_bytes: 16 * 1024 * 1024,
        ..RuntimeLimitsV1::default()
    })
    .unwrap()
}

fn small_request() -> ChatRequest {
    serde_json::from_value(json!({"model":"m","messages":[{"role":"user","content":"after"}]}))
        .unwrap()
}

fn capacity_request() -> ChatRequest {
    let mut messages = Vec::new();
    for index in 0..12_000 {
        let identity = format!(
            "ca1_{}",
            URL_SAFE_NO_PAD.encode(json!([format!("wire-{index:08}"), "", "m"]).to_string())
        );
        messages.push(json!({"role":"assistant","tool_calls":[{"id":identity,"name":"read","arguments":"{}"}]}));
        messages.push(json!({"role":"tool","tool_call_id":identity,"content":"result"}));
    }
    serde_json::from_value(json!({"model":"m","messages":messages})).unwrap()
}

fn trap_main(adapter: &SouthComponentAdapter) {
    let error = adapter
        .build_http_request(&capacity_request(), &config())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(
        error.message.contains("trap"),
        "The failed request must remain a failure: {error:?}"
    );
}

fn response() -> HttpResponseParts {
    serde_json::from_value(json!({"status":200,"headers":{},"body":json!({"response":{"modelVersion":"m","candidates":[{"content":{"parts":[{"text":"after"}]},"finishReason":"STOP"}]}}).to_string()})).unwrap()
}

#[test]
fn trapped_main_recovers_for_later_calls_without_replaying_the_failed_request() {
    let (manifest, wasm) = package();
    let runtime = recovery_runtime();
    let adapter = SouthComponentAdapter::load_embedded(&runtime, manifest, wasm).unwrap();
    assert!(
        adapter
            .build_http_request(&small_request(), &config())
            .is_ok()
    );
    let mut stream = adapter.stream_parser();
    let wire = format!(
        "data: {}\n\n",
        json!({"response":{"modelVersion":"m","candidates":[{"content":{"parts":[{"text":"stream-before"}]}}]}})
    );
    assert!(
        !stream
            .parse_chunk(&StreamChunk { data: wire })
            .unwrap()
            .is_empty()
    );
    trap_main(&adapter);
    assert!(adapter.model_capabilities(&config()).unwrap().is_empty());
    assert_eq!(
        adapter.parse_response(&response()).unwrap().choices.len(),
        1
    );
    let failure = HttpResponseParts {
        status: 429,
        body: "{}".into(),
        ..response()
    };
    assert_eq!(
        adapter.map_provider_error(&failure).unwrap().code,
        ErrorCode::RateLimit
    );
    assert!(
        adapter
            .build_http_request(&small_request(), &config())
            .is_ok()
    );
    let terminal = format!(
        "data: {}\n\n",
        json!({"response":{"candidates":[{"finishReason":"STOP"}]}})
    );
    let mut events = stream.parse_chunk(&StreamChunk { data: terminal }).unwrap();
    events.extend(stream.finish().unwrap());
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::Done { .. }))
    );

    trap_main(&adapter);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let adapter = &adapter;
            scope.spawn(move || {
                assert!(
                    adapter
                        .build_http_request(&small_request(), &config())
                        .is_ok()
                );
            });
        }
    });
}

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).unwrap();
        let path = std::env::temp_dir().join(format!(
            "token-station-south-recovery-{}-{}",
            std::process::id(),
            URL_SAFE_NO_PAD.encode(nonce)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn directory_recovery_uses_the_admitted_snapshot_after_files_change() {
    let directory = Directory::new();
    let (manifest, wasm) = package();
    std::fs::write(directory.0.join("manifest.json"), manifest).unwrap();
    std::fs::write(directory.0.join("component.wasm"), wasm).unwrap();
    let runtime = recovery_runtime();
    let adapter = SouthComponentAdapter::load_dir(&runtime, &directory.0).unwrap();
    assert!(
        adapter
            .build_http_request(&small_request(), &config())
            .is_ok()
    );
    std::fs::write(directory.0.join("manifest.json"), "{}").unwrap();
    std::fs::write(directory.0.join("component.wasm"), "replacement").unwrap();
    trap_main(&adapter);
    let request = adapter
        .build_http_request(&small_request(), &config())
        .unwrap();
    assert_eq!(
        request.url,
        "https://cloudcode-pa.googleapis.com/v1internal:generateContent"
    );
    assert_eq!(adapter.package_name(), "provider-codeassist");
    assert!(adapter.auth_arms().contains("bearer"));
}

#[test]
fn directory_admission_preserves_manifest_precedence_and_file_bounds() {
    let directory = Directory::new();
    let runtime = recovery_runtime();
    let (manifest, _) = package();
    let mut stale: Value = serde_json::from_str(manifest).unwrap();
    stale["compatibility"]["south_runtime"] = json!("0.15.0");
    std::fs::write(directory.0.join("manifest.json"), stale.to_string()).unwrap();
    let error = SouthComponentAdapter::load_dir(&runtime, &directory.0).unwrap_err();
    assert!(error.contains("not compatible"), "{error}");
    std::fs::write(directory.0.join("manifest.json"), "{").unwrap();
    assert!(
        SouthComponentAdapter::load_dir(&runtime, &directory.0)
            .unwrap_err()
            .contains("manifest")
    );
    std::fs::write(directory.0.join("manifest.json"), manifest).unwrap();
    assert!(
        SouthComponentAdapter::load_dir(&runtime, &directory.0)
            .unwrap_err()
            .contains("component.wasm")
    );
    std::fs::File::create(directory.0.join("component.wasm"))
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    assert!(
        SouthComponentAdapter::load_dir(&runtime, &directory.0)
            .unwrap_err()
            .contains("limit")
    );
    std::fs::write(directory.0.join("manifest.json"), stale.to_string()).unwrap();
    assert!(
        SouthComponentAdapter::load_dir(&runtime, &directory.0)
            .unwrap_err()
            .contains("not compatible")
    );
    std::fs::write(directory.0.join("manifest.json"), "{").unwrap();
    assert!(
        SouthComponentAdapter::load_dir(&runtime, &directory.0)
            .unwrap_err()
            .contains("manifest")
    );
    std::fs::write(
        directory.0.join("manifest.json"),
        vec![b' '; 256 * 1024 + 1],
    )
    .unwrap();
    assert!(
        SouthComponentAdapter::load_dir(&runtime, &directory.0)
            .unwrap_err()
            .contains("limit")
    );
}

//! `plugin install` end to end, on the real official provider package: the
//! conformance suite runs against the actual `wasm32-wasip2` component, the
//! receipt lands in the data dir, the registry serves the dialect afterwards
//! — and stops serving it the moment the installed bytes change.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use serde_json::json;
use token_station_cli::config::ClientConfig;
use token_station_cli::plugins::{self, PluginRegistry};

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("apps/cli sits two levels below the root")
}

/// Builds the official provider plugin once and assembles an installable
/// source package (manifest + wasm + fixtures) outside the plugins dir.
fn source_package() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let source = repo_root().join("plugins/official/provider-openai-compatible");
        let status = Command::new("cargo")
            .args(["build", "--target", "wasm32-wasip2"])
            .current_dir(&source)
            .status()
            .expect("cargo is on PATH");
        assert!(status.success(), "provider-openai-compatible must build");

        let dir = std::env::temp_dir().join(format!("ts-install-source-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("fixtures")).expect("temp dir writable");
        std::fs::copy(source.join("manifest.json"), dir.join("manifest.json"))
            .expect("manifest copies");
        std::fs::copy(
            source.join("target/wasm32-wasip2/debug/provider_openai_compatible.wasm"),
            dir.join("adapter.wasm"),
        )
        .expect("wasm copies");
        for entry in std::fs::read_dir(source.join("fixtures")).expect("fixtures exist") {
            let entry = entry.expect("fixtures readable");
            std::fs::copy(entry.path(), dir.join("fixtures").join(entry.file_name()))
                .expect("fixture copies");
        }
        dir
    })
}

/// A config whose plugins dir starts empty and whose trust default is the
/// shipped one: unsigned packages do not serve.
fn config(tag: &str) -> ClientConfig {
    let scratch = std::env::temp_dir().join(format!("ts-install-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(scratch.join("plugins")).expect("temp dir writable");
    let config = json!({
        "version": 1,
        "server": { "listen": "127.0.0.1:0" },
        "data": { "dir": scratch.join("data") },
        "plugins": { "dir": scratch.join("plugins"), "agent": "agent-openai" },
        "upstreams": {},
        "router": { "version": 1, "pools": { "main": [] }, "hint_routes": [], "default_pool": "main" }
    });
    serde_json::from_value(config).expect("test config parses")
}

#[test]
fn install_admits_serve_follows_and_tampering_revokes() {
    let config = config("roundtrip");

    // Before: the dialect resolves to nothing.
    let registry = PluginRegistry::for_config(&config).expect("empty registry builds");
    assert!(registry.provider_binding("openai-compatible").is_none());

    // Install: conformance runs on the real component, receipt recorded.
    let summary = plugins::install(&config, source_package()).expect("the official plugin passes");
    assert!(summary.contains("provider-protocol-v1"), "{summary}");

    // After: the dialect serves, and the package reports verified.
    let registry = PluginRegistry::for_config(&config).expect("registry builds");
    assert!(registry.provider_binding("openai-compatible").is_some());
    assert!(
        registry
            .package("provider-openai-compatible")
            .expect("catalogued")
            .verified
    );

    // A second install of the same package is refused, not overwritten.
    let error = plugins::install(&config, source_package()).expect_err("already installed");
    assert!(error.contains("plugin remove"), "{error}");

    // Tampering: changed bytes lose the approval, loudly in the listing.
    let installed_wasm = config
        .plugins
        .dir
        .join("provider-openai-compatible/adapter.wasm");
    std::fs::write(&installed_wasm, b"tampered").expect("temp dir writable");
    let registry = PluginRegistry::for_config(&config).expect("registry builds");
    assert!(
        registry.provider_binding("openai-compatible").is_none(),
        "approval must follow bytes"
    );
    assert!(registry.render_list().contains("no conformance receipt"));
}

#[test]
fn install_accepts_the_package_a_configured_entry_predeclares() {
    // The shipped example config pre-declares `openai-compatible ->
    // provider-openai-compatible` before the package exists on disk
    // (`plugins.providers` documents that as intent, not presence). Installing
    // the very package the entry names is that intent being fulfilled — it
    // must admit, not report a conflict with itself.
    let mut config = config("predeclared");
    config.plugins.providers.insert(
        "openai-compatible".to_owned(),
        "provider-openai-compatible".to_owned(),
    );

    let summary = plugins::install(&config, source_package()).expect("agreement is not a conflict");
    assert!(summary.contains("provider-protocol-v1"), "{summary}");
    let registry = PluginRegistry::for_config(&config).expect("registry builds");
    assert!(registry.provider_binding("openai-compatible").is_some());
}

#[test]
fn install_refuses_a_dialect_a_different_package_already_claims() {
    let mut config = config("claimed");
    config.plugins.providers.insert(
        "openai-compatible".to_owned(),
        "provider-somebody-else".to_owned(),
    );

    let error = plugins::install(&config, source_package())
        .expect_err("two providers for one dialect stays a conflict");
    assert!(error.contains("provider-somebody-else"), "{error}");
}

#[test]
fn remove_deletes_the_package_unless_an_upstream_depends_on_it() {
    let mut config = config("remove");
    plugins::install(&config, source_package()).expect("the official plugin passes");

    // An upstream speaking the dialect pins the package in place.
    config.upstreams.insert(
        "mock".to_owned(),
        serde_json::from_value(json!({
            "provider": "openai-compatible",
            "base_url": "http://127.0.0.1:1/v1",
            "models": [ { "model": "gpt-5.5" } ]
        }))
        .expect("upstream parses"),
    );
    let error =
        plugins::remove(&config, "provider-openai-compatible").expect_err("an upstream depends");
    assert!(error.contains("mock"), "{error}");

    config.upstreams.clear();
    let summary = plugins::remove(&config, "provider-openai-compatible").expect("now removable");
    assert!(summary.contains("removed"), "{summary}");
    let registry = PluginRegistry::for_config(&config).expect("registry builds");
    assert!(registry.package("provider-openai-compatible").is_none());
}

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

#[cfg(unix)]
fn cloned_source(tag: &str) -> PathBuf {
    let source = source_package();
    let dir = std::env::temp_dir().join(format!("ts-install-clone-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("fixtures")).expect("clone fixture dir");
    for file in ["manifest.json", "adapter.wasm"] {
        std::fs::copy(source.join(file), dir.join(file)).expect("package file clones");
    }
    for entry in std::fs::read_dir(source.join("fixtures")).expect("fixtures exist") {
        let entry = entry.expect("fixture readable");
        std::fs::copy(entry.path(), dir.join("fixtures").join(entry.file_name()))
            .expect("fixture clones");
    }
    dir
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
            .conformance_passed
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

#[test]
fn remove_rejects_relative_and_absolute_paths_before_touching_the_filesystem() {
    let config = config("remove-escape");
    let scratch = config.plugins.dir.parent().expect("plugins has a parent");
    let victim = scratch.join("victim");
    std::fs::create_dir_all(&victim).expect("victim dir");
    std::fs::write(
        victim.join("manifest.json"),
        b"not a plugin, only a deletion canary",
    )
    .expect("victim marker");

    for malicious_name in ["../victim", victim.to_str().expect("utf-8 temp path")] {
        let error = plugins::remove(&config, malicious_name)
            .expect_err("a plugin name is never a relative or absolute path");
        assert!(
            error.contains("plugin name"),
            "refusal should identify the invalid public input: {error}"
        );
        assert!(
            victim.join("manifest.json").exists(),
            "invalid input must not delete outside the plugin root"
        );
    }

    std::fs::remove_dir_all(scratch).ok();
}

#[cfg(unix)]
#[test]
fn install_rejects_a_fixture_symlink_without_copying_its_target() {
    use std::os::unix::fs::symlink;

    let config = config("fixture-symlink");
    let source = cloned_source("fixture-symlink");
    let outside = source
        .parent()
        .expect("source parent")
        .join(format!("ts-install-secret-{}.txt", std::process::id()));
    std::fs::write(&outside, b"must never be copied").expect("outside canary");
    symlink(&outside, source.join("fixtures/leak.txt")).expect("fixture symlink");

    let error = plugins::install(&config, &source)
        .expect_err("an installable package may not contain symbolic links");
    assert!(error.contains("symbolic link"), "{error}");
    assert!(
        !config
            .plugins
            .dir
            .join("provider-openai-compatible")
            .exists(),
        "a refused package must not leave a discoverable directory"
    );

    std::fs::remove_dir_all(source).ok();
    std::fs::remove_file(outside).ok();
}

#[cfg(unix)]
#[test]
fn remove_rejects_a_plugin_directory_symlink_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let config = config("remove-symlink");
    let scratch = config.plugins.dir.parent().expect("plugins has a parent");
    let victim = scratch.join("victim-plugin");
    std::fs::create_dir_all(&victim).expect("victim dir");
    std::fs::copy(
        source_package().join("manifest.json"),
        victim.join("manifest.json"),
    )
    .expect("valid victim manifest");
    symlink(
        &victim,
        config.plugins.dir.join("provider-openai-compatible"),
    )
    .expect("plugin directory symlink");

    let error = plugins::remove(&config, "provider-openai-compatible")
        .expect_err("removal may never follow an installed-directory symlink");
    assert!(error.contains("symbolic link"), "{error}");
    assert!(
        victim.join("manifest.json").exists(),
        "the symlink target must remain untouched"
    );

    std::fs::remove_file(config.plugins.dir.join("provider-openai-compatible")).ok();
    std::fs::remove_dir_all(scratch).ok();
}

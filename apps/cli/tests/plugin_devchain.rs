//! The whole third-party chain, end to end: `plugin new` scaffolds a
//! project, `plugin build` compiles the real `wasm32-wasip2` component,
//! `plugin test` runs the conformance suite on it, `plugin install` admits
//! it — and the registry then serves the new dialect. If the scaffold ever
//! stops passing its own suite, this is where it fails.

use std::path::Path;

use serde_json::json;
use token_station_cli::config::ClientConfig;
use token_station_cli::plugins::{self, PluginRegistry};
use token_station_cli::scaffold;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("apps/cli sits two levels below the root")
}

fn cargo_path_dependency(path: &Path) -> String {
    let path = path.to_string_lossy();
    let quoted = serde_json::to_string(path.as_ref()).expect("filesystem path is serializable");
    format!("{{ path = {quoted} }}")
}

#[test]
fn windows_checkout_paths_are_escaped_as_valid_toml_basic_strings() {
    assert_eq!(
        cargo_path_dependency(Path::new(r"D:\tokenstation\crates\protocol")),
        r#"{ path = "D:\\tokenstation\\crates\\protocol" }"#
    );
}

#[test]
fn scaffold_build_test_install_serves_the_new_dialect() {
    let scratch = std::env::temp_dir().join(format!("ts-devchain-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(scratch.join("plugins")).expect("temp dir writable");

    // plugin new
    let summary =
        scaffold::new_package(&scratch, "provider-acme", "acme").expect("scaffold writes");
    assert!(summary.contains("acme"), "{summary}");
    let project = scratch.join("provider-acme");

    // The scaffold ships a git dependency; the test must build against THIS
    // checkout, not the network's idea of main.
    let cargo_toml = std::fs::read_to_string(project.join("Cargo.toml")).expect("written");
    let pinned = cargo_toml.replace(
        r#"{ git = "https://github.com/ballast-ai/token-station.git" }"#,
        &cargo_path_dependency(&repo_root().join("crates/protocol")),
    );
    assert_ne!(
        pinned, cargo_toml,
        "the git dependency line must exist to be pinned"
    );
    std::fs::write(project.join("Cargo.toml"), pinned).expect("temp dir writable");

    // This behavior test is not allowed to turn a transient registry outage
    // into a product failure. CI explicitly fetches the official provider's
    // locked dependency set first; the generated project then resolves and
    // builds from that cache with networking disabled.
    std::fs::create_dir_all(project.join(".cargo")).expect("cargo config dir writable");
    std::fs::write(
        project.join(".cargo/config.toml"),
        "[net]\noffline = true\n",
    )
    .expect("offline cargo config writable");

    // plugin build → plugin test
    scaffold::build_package(&project).expect("the scaffold compiles");
    let verdict = scaffold::test_package(&project).expect("the scaffold passes its own suite");
    assert!(verdict.contains("passed"), "{verdict}");

    // plugin install, into a config with the shipped trust default.
    let config: ClientConfig = serde_json::from_value(json!({
        "version": 1,
        "server": { "listen": "127.0.0.1:0" },
        "data": { "dir": scratch.join("data") },
        "plugins": { "dir": scratch.join("plugins"), "agent": "agent-openai" },
        "upstreams": {},
        "router": { "version": 1, "pools": { "main": [] }, "hint_routes": [], "default_pool": "main" }
    }))
    .expect("test config parses");
    plugins::install(&config, &project).expect("a passing package installs");

    let registry = PluginRegistry::for_config(&config).expect("registry builds");
    assert!(
        registry.provider_binding("acme").is_some(),
        "the scaffolded dialect serves"
    );
    assert!(
        registry
            .package("provider-acme")
            .expect("catalogued")
            .conformance_passed
    );
    std::fs::remove_dir_all(scratch).ok();
}

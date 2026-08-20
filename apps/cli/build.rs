//! Wires the builtin plugin tier (architecture section 12.1). With `--features
//! builtin-plugins` the release pipeline points `TOKEN_STATION_PLUGINS_DIST`
//! at a staged plugins directory and the official packages are compiled into
//! the binary. A plain `cargo build` has no builtin tier and none of this
//! runs — the feature exists for artifact assembly, not product tiering.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=TOKEN_STATION_RELEASE_PUBKEY_HEX");
    println!("cargo:rerun-if-env-changed=TOKEN_STATION_PLUGINS_DIST");
    if std::env::var_os("CARGO_FEATURE_BUILTIN_PLUGINS").is_none() {
        return;
    }

    let dist = std::env::var("TOKEN_STATION_PLUGINS_DIST").expect(
        "the `builtin-plugins` feature needs TOKEN_STATION_PLUGINS_DIST pointing at a directory \
         holding all official agent/provider packages (manifest.json + adapter.wasm each) \
         — scripts/build-release.sh assembles one",
    );
    let dist = Path::new(&dist)
        .canonicalize()
        .expect("TOKEN_STATION_PLUGINS_DIST must name an existing directory");

    for (stem, package, wasm_file) in [
        ("AGENT_OPENAI", "agent-openai", "adapter.wasm"),
        ("AGENT_ANTHROPIC", "agent-anthropic", "adapter.wasm"),
        (
            "AGENT_OPENAI_RESPONSES",
            "agent-openai-responses",
            "adapter.wasm",
        ),
        ("AGENT_GEMINI", "agent-gemini", "adapter.wasm"),
        (
            "PROVIDER_OPENAI",
            "provider-openai-compatible",
            "adapter.wasm",
        ),
        // The v2 south component: staged by the same scripts, embedded by the
        // same mechanism, but consumed by the south loader, never the v1
        // registry (its manifest is the v2 schema). It lives under the
        // `south/` subtree so the v1 directory scan never reads its manifest.
        (
            "PROVIDER_OPENAI_V2",
            "south/provider-openai-compatible-v2",
            "component.wasm",
        ),
    ] {
        for (kind, file) in [("MANIFEST", "manifest.json"), ("WASM", wasm_file)] {
            let path = dist.join(package).join(file);
            assert!(
                path.is_file(),
                "builtin plugin file missing: {}",
                path.display()
            );
            println!(
                "cargo:rustc-env=TS_BUILTIN_{stem}_{kind}={}",
                path.display()
            );
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

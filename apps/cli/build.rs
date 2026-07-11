//! Wires the builtin plugin tier (architecture section 12.1). With `--features
//! builtin-plugins` the release pipeline points `TOKEN_STATION_PLUGINS_DIST`
//! at a staged plugins directory and the official packages are compiled into
//! the binary. A plain `cargo build` has no builtin tier and none of this
//! runs — the feature exists for artifact assembly, not product tiering.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=TOKEN_STATION_PLUGINS_DIST");
    if std::env::var_os("CARGO_FEATURE_BUILTIN_PLUGINS").is_none() {
        return;
    }

    let dist = std::env::var("TOKEN_STATION_PLUGINS_DIST").expect(
        "the `builtin-plugins` feature needs TOKEN_STATION_PLUGINS_DIST pointing at a directory \
         holding agent-openai/ and provider-openai-compatible/ (manifest.json + adapter.wasm \
         each) — scripts/build-release.sh assembles one",
    );
    let dist = Path::new(&dist)
        .canonicalize()
        .expect("TOKEN_STATION_PLUGINS_DIST must name an existing directory");

    for (stem, package) in [
        ("AGENT_OPENAI", "agent-openai"),
        ("PROVIDER_OPENAI", "provider-openai-compatible"),
    ] {
        for (kind, file) in [("MANIFEST", "manifest.json"), ("WASM", "adapter.wasm")] {
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

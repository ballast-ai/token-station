//! Generates the builtin package tier from the official package set.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Deserialize)]
struct PackageSet {
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    dir: String,
    id: String,
    kind: String,
    wasm: String,
}

fn package_entries(packages: &[Package], kind: &str, dist: Option<&Path>) -> String {
    let Some(dist) = dist else {
        return String::new();
    };
    let mut entries = String::new();
    for package in packages.iter().filter(|package| package.kind == kind) {
        let package_dir = dist.join(&package.dir);
        let manifest = package_dir.join("manifest.json");
        let wasm = package_dir.join(&package.wasm);
        assert!(
            manifest.is_file(),
            "builtin plugin file missing: {}",
            manifest.display()
        );
        assert!(
            wasm.is_file(),
            "builtin plugin file missing: {}",
            wasm.display()
        );
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rerun-if-changed={}", wasm.display());
        writeln!(
            entries,
            "    Package {{ manifest_source: include_str!({:?}), wasm: include_bytes!({:?}) }},",
            manifest.to_string_lossy(),
            wasm.to_string_lossy(),
        )
        .expect("generated package entry writes");
    }
    entries
}

fn main() {
    println!("cargo:rerun-if-env-changed=TOKEN_STATION_RELEASE_PUBKEY_HEX");
    println!("cargo:rerun-if-env-changed=TOKEN_STATION_PLUGINS_DIST");

    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let package_set_path = manifest_dir.join("../../plugins/official/packages.json");
    println!("cargo:rerun-if-changed={}", package_set_path.display());
    let package_set: PackageSet = serde_json::from_slice(
        &std::fs::read(&package_set_path).expect("official package set reads"),
    )
    .expect("official package set is valid JSON");
    for package in &package_set.packages {
        assert!(
            matches!(package.kind.as_str(), "agent" | "south-component"),
            "unsupported official package kind: {}",
            package.kind
        );
    }

    let dist = if std::env::var_os("CARGO_FEATURE_BUILTIN_PLUGINS").is_some() {
        let path = std::env::var("TOKEN_STATION_PLUGINS_DIST").expect(
            "the `builtin-plugins` feature needs TOKEN_STATION_PLUGINS_DIST pointing at a directory holding every official package",
        );
        Some(
            Path::new(&path)
                .canonicalize()
                .expect("TOKEN_STATION_PLUGINS_DIST must name an existing directory"),
        )
    } else {
        None
    };

    let mut generated = String::new();
    generated.push_str("pub(super) const IDS: &[&str] = &[\n");
    for package in &package_set.packages {
        writeln!(generated, "    {:?},", package.id).expect("generated package id writes");
    }
    generated.push_str("];\n");
    generated.push_str("pub(super) const AGENTS: &[Package] = &[\n");
    generated.push_str(&package_entries(
        &package_set.packages,
        "agent",
        dist.as_deref(),
    ));
    generated.push_str("];\n");
    generated.push_str("pub(super) const PROVIDERS: &[Package] = &[\n");
    generated.push_str(&package_entries(
        &package_set.packages,
        "south-component",
        dist.as_deref(),
    ));
    generated.push_str("];\n");

    let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("build output dir"))
        .join("builtin_official_packages.rs");
    std::fs::write(output, generated).expect("generated builtin package source writes");
}

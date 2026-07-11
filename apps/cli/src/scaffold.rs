//! `plugin new / build / test`: the third-party developer chain
//! (architecture section 12.4, stage B3).
//!
//! The scaffold is not a stub — it is the official OpenAI-compatible
//! provider adapter with its identity renamed. It compiles, and it passes
//! the conformance suite before its author changes a line, so development
//! is "make the working translation say something else", with `plugin test`
//! telling the truth after every step. The template files are embedded at
//! compile time from `plugins/official/provider-openai-compatible`, so the
//! scaffold can never drift from the code the suite actually admits.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::Command;

use token_station_plugin_api::AdapterManifest;

use crate::plugins;

const TEMPLATE_PACKAGE: &str = "provider-openai-compatible";
const TEMPLATE_DIALECT: &str = "openai-compatible";
/// The `wit_bindgen::generate!` path the template uses inside this repo;
/// the scaffold vendors the WIT and points at it instead.
const TEMPLATE_WIT_PATH: &str = "../../../crates/plugin-api/wit";

const TEMPLATE_LIB: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/official/provider-openai-compatible/src/lib.rs"
));
const TEMPLATE_MANIFEST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/official/provider-openai-compatible/manifest.json"
));
const WIT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/plugin-api/wit/adapter.wit"
));

macro_rules! fixture {
    ($name:literal) => {
        (
            $name,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../plugins/official/provider-openai-compatible/fixtures/",
                $name
            )),
        )
    };
}

const FIXTURES: &[(&str, &str)] = &[
    fixture!("provider.capabilities.declared.input.json"),
    fixture!("provider.capabilities.declared.expected.json"),
    fixture!("provider.error.rate-limit.input.json"),
    fixture!("provider.error.rate-limit.expected.json"),
    fixture!("provider.error.rejected-credential.input.json"),
    fixture!("provider.error.rejected-credential.expected.json"),
    fixture!("provider.request.chat.input.json"),
    fixture!("provider.request.chat.expected.json"),
    fixture!("provider.response.tool-call.input.json"),
    fixture!("provider.response.tool-call.expected.json"),
    fixture!("provider.stream.tool-call.input.json"),
    fixture!("provider.stream.tool-call.expected.json"),
];

/// `plugin new`: writes a compiling, conformance-passing provider-adapter
/// project at `<parent>/<name>`, speaking `<dialect>`.
///
/// # Errors
///
/// An invalid name or dialect, an already-existing destination, or the
/// filesystem.
pub fn new_package(parent: &Path, name: &str, dialect: &str) -> Result<String, String> {
    validate_ident("package name", name)?;
    validate_ident("dialect", dialect)?;

    let dest = parent.join(name);
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()));
    }

    // The package name contains the dialect as a substring, so the longer
    // string goes through a placeholder first.
    let rename = |text: &str| {
        text.replace(TEMPLATE_PACKAGE, "\u{1}")
            .replace(TEMPLATE_DIALECT, dialect)
            .replace('\u{1}', name)
    };

    let write = |relative: &str, contents: &str| -> Result<(), String> {
        let path = dest.join(relative);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        }
        fs::write(&path, contents).map_err(|error| format!("{}: {error}", path.display()))
    };

    write("Cargo.toml", &cargo_toml(name))?;
    write(
        "src/lib.rs",
        &rename(TEMPLATE_LIB).replace(TEMPLATE_WIT_PATH, "wit"),
    )?;
    write("manifest.json", &rename(TEMPLATE_MANIFEST))?;
    write("wit/adapter.wit", WIT)?;
    for (fixture_name, contents) in FIXTURES {
        write(&format!("fixtures/{fixture_name}"), &rename(contents))?;
    }
    write(".gitignore", "/target\n/adapter.wasm\n")?;

    Ok(format!(
        "scaffolded `{name}` (dialect `{dialect}`) at {}\n\
         it is the official OpenAI-compatible adapter, renamed — it passes conformance \
         before you change anything.\n\
         next:\n  \
         plugin build {0}    # cargo build --target wasm32-wasip2\n  \
         plugin test {0}     # the conformance suite, honestly\n  \
         plugin install {0}  # admit it for traffic",
        dest.display(),
    ))
}

/// `plugin build`: compiles the package and places `adapter.wasm` beside its
/// manifest, so the project directory itself becomes an installable package.
///
/// # Errors
///
/// A missing manifest, a failing `cargo build` (with the likeliest fix
/// named), or the filesystem.
pub fn build_package(path: &Path) -> Result<String, String> {
    let manifest = read_manifest(path)?;

    let status = Command::new("cargo")
        .args(["build", "--target", "wasm32-wasip2"])
        .current_dir(path)
        .status()
        .map_err(|error| format!("cargo: {error}"))?;
    if !status.success() {
        return Err(
            "cargo build failed (see its output above); if the target is missing: \
             rustup target add wasm32-wasip2"
                .to_owned(),
        );
    }

    let artifact = path.join(format!(
        "target/wasm32-wasip2/debug/{}.wasm",
        manifest.name.replace('-', "_")
    ));
    let dest = path.join("adapter.wasm");
    fs::copy(&artifact, &dest).map_err(|error| format!("{}: {error}", artifact.display()))?;

    Ok(format!(
        "built {} — next: plugin test {}",
        dest.display(),
        path.display(),
    ))
}

/// `plugin test`: the same suite `plugin install` will hold the package to,
/// runnable on every save. Exits nonzero on failure, so it scripts.
///
/// # Errors
///
/// A missing manifest, a package the runtime refuses to load, or the report
/// itself when any check fails — each failure named.
pub fn test_package(path: &Path) -> Result<String, String> {
    let manifest = read_manifest(path)?;
    let report = plugins::run_conformance(path, &manifest)?;
    if report.is_passing() {
        Ok(format!(
            "{}: all {} checks passed",
            report.suite(),
            report.outcomes().len(),
        ))
    } else {
        let mut rendered = String::new();
        let _ = write!(rendered, "{report}");
        Err(rendered)
    }
}

fn read_manifest(path: &Path) -> Result<AdapterManifest, String> {
    let manifest_path = path.join("manifest.json");
    let source = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    serde_json::from_str(&source).map_err(|error| format!("{}: {error}", manifest_path.display()))
}

/// Lowercase kebab, like every package and dialect this ecosystem names.
fn validate_ident(what: &str, value: &str) -> Result<(), String> {
    let shaped = value
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if shaped {
        Ok(())
    } else {
        Err(format!(
            "{what} `{value}` must be lowercase kebab-case (a-z, 0-9, `-`, starting with a letter)"
        ))
    }
}

fn cargo_toml(name: &str) -> String {
    format!(
        r#"# A token-station provider adapter, compiled to a `wasm32-wasip2`
# component. Scaffolded from the official OpenAI-compatible adapter by
# `token-station-cli plugin new`.
[package]
name = "{name}"
version = "1.0.0"
edition = "2021"
publish = false

[workspace]

[lib]
crate-type = ["cdylib"]

[dependencies]
serde = "1"
serde_json = "1"
# The Canonical IR, used as types rather than re-derived by hand. Pin a tag
# once you depend on one: {{ git = ..., tag = "..." }}.
token-station-protocol = {{ git = "https://github.com/ballast-ai/token-station.git" }}
wit-bindgen = "0.59"
"#
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::new_package;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "token-station-scaffold-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir is writable");
        dir
    }

    #[test]
    fn the_scaffold_renames_the_identity_everywhere() {
        let dir = scratch("rename");
        new_package(&dir, "provider-acme", "acme").expect("scaffold writes");

        let root = dir.join("provider-acme");
        let manifest = std::fs::read_to_string(root.join("manifest.json")).expect("written");
        assert!(manifest.contains("\"provider-acme\""), "{manifest}");
        assert!(manifest.contains("\"acme\""), "{manifest}");
        assert!(!manifest.contains("openai"), "{manifest}");

        let lib = std::fs::read_to_string(root.join("src/lib.rs")).expect("written");
        assert!(lib.contains("provider-acme"), "identity renamed");
        assert!(!lib.contains(super::TEMPLATE_WIT_PATH), "WIT path vendored");
        assert!(root.join("wit/adapter.wit").is_file());

        for (fixture, _) in super::FIXTURES {
            let content =
                std::fs::read_to_string(root.join("fixtures").join(fixture)).expect("written");
            assert!(!content.contains("openai-compatible"), "{fixture}");
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_name_that_is_not_kebab_case_is_refused() {
        let dir = scratch("shape");
        for bad in ["Acme", "1acme", "ac me", "ACME", ""] {
            assert!(new_package(&dir, bad, "acme").is_err(), "{bad:?}");
        }
        std::fs::remove_dir_all(dir).ok();
    }
}

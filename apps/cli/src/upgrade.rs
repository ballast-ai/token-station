//! `upgrade`: anonymous version check, explicit-confirmation download,
//! signature verification. Never automatic.
//!
//! This is the **only** outbound destination that is not a user-configured
//! upstream (the exit criterion's explainable-egress rule), and it is reached exclusively
//! when the operator runs this command — `serve` never phones home.
//!
//! The check asks the GitHub Releases API; trust does not: a downloaded
//! artifact counts only if the release's Ed25519-signed manifest verifies
//! against [`OFFICIAL_RELEASE_PUBKEY_HEX`], compiled into this binary. The
//! distribution channel is a courier, not an authority — swapping the
//! endpoint for a self-hosted one later changes one constant here.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use token_station_release::{ReleaseManifest, parse_public_key, sha256_file, verify_bytes};

/// The official release public key, lowercase hex, stamped in by the release
/// pipeline. Empty in a source build: version *checking* still works, and
/// [`download_and_verify`] refuses — someone who builds from source verifies
/// by rebuilding, not by trusting our key.
pub const OFFICIAL_RELEASE_PUBKEY_HEX: &str = "";

/// Where the anonymous check goes (July 2026 decision: GitHub Releases first).
pub const DEFAULT_ENDPOINT: &str = "https://api.github.com/repos/ballast-ai/token-station";

/// This build's version, from the crate that compiled it.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// One published release, as much of it as the upgrade path needs.
#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    /// e.g. `v0.2.0`.
    pub tag_name: String,
    /// The human page to read the notes on.
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
}

/// Asks `endpoint` for the latest release. One anonymous GET; nothing about
/// this machine goes out but the request itself.
///
/// # Errors
///
/// Transport failure, a non-200, or a body that is not a release.
pub fn check(endpoint: &str) -> Result<Release, String> {
    let url = format!("{endpoint}/releases/latest");
    let body = http_get_string(&url)?;
    serde_json::from_str(&body).map_err(|error| format!("release listing: {error}"))
}

/// Whether `tag` (e.g. `v0.2.0`) is newer than `current` (e.g. `0.1.0`).
/// Numeric per-component comparison; a tag that does not parse is treated as
/// not newer — an unparseable tag must not become an upgrade prompt.
#[must_use]
pub fn is_newer(current: &str, tag: &str) -> bool {
    match (parse_version(current), parse_version(tag)) {
        (Some(current), Some(tag)) => tag > current,
        _ => false,
    }
}

fn parse_version(raw: &str) -> Option<[u64; 3]> {
    let mut parts = raw.trim().trim_start_matches('v').split('.');
    let mut version = [0u64; 3];
    for slot in &mut version {
        *slot = parts.next()?.parse().ok()?;
    }
    parts.next().is_none().then_some(version)
}

/// The target triple this binary was built for — the four we publish, spelled
/// exactly as the release artifacts spell them.
#[must_use]
pub fn target_triple() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("aarch64-unknown-linux-gnu")
    } else {
        None
    }
}

/// Downloads this platform's artifact from `release` into `target_dir` and
/// verifies it: manifest signature first (against `pubkey_hex`), then the
/// artifact's SHA-256 against the verified manifest. Returns the verified
/// file's path.
///
/// Nothing is executed and nothing is replaced — installing the verified
/// archive stays a human decision.
///
/// # Errors
///
/// A missing asset for this platform, a transport failure, or — the ones
/// that matter — a manifest that does not verify or an artifact whose hash
/// does not match. The partial download is removed on hash failure.
pub fn download_and_verify(
    release: &Release,
    target_dir: &Path,
    pubkey_hex: &str,
) -> Result<PathBuf, String> {
    if pubkey_hex.is_empty() {
        return Err(format!(
            "this build carries no release public key (built from source?); download from\n  {}\nand verify per docs/release/可复现构建与发布验证.md",
            release.html_url
        ));
    }
    let key = parse_public_key(pubkey_hex)?;
    let triple = target_triple().ok_or("no published artifact for this platform")?;

    let asset = |name: &str| {
        release
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .ok_or_else(|| format!("release has no `{name}` asset"))
    };
    let version = release.tag_name.trim_start_matches('v');
    let artifact_name = format!("token-station-cli-{version}-{triple}.tar.gz");

    // Manifest and signature are small; verify them before the big download.
    let manifest_bytes = http_get_bytes(&asset("manifest.json")?.browser_download_url)?;
    let signature = http_get_string(&asset("manifest.json.sig")?.browser_download_url)?;
    verify_bytes(&key, &manifest_bytes, &signature)
        .map_err(|error| format!("release manifest does not verify: {error}"))?;
    let manifest = ReleaseManifest::parse(&manifest_bytes)?;
    let expected = manifest
        .sha256_of(&artifact_name)
        .ok_or_else(|| format!("the signed manifest does not list `{artifact_name}`"))?;

    let artifact = http_get_bytes(&asset(&artifact_name)?.browser_download_url)?;
    let path = target_dir.join(&artifact_name);
    std::fs::write(&path, &artifact)
        .map_err(|error| format!("write `{}`: {error}", path.display()))?;

    let actual = sha256_file(&path)?;
    if actual != expected {
        std::fs::remove_file(&path).ok();
        return Err(format!(
            "`{artifact_name}` does not match the signed manifest (expected {expected}, got \
             {actual}); the download was discarded"
        ));
    }
    Ok(path)
}

/// Downloads are capped: a release artifact is megabytes, and an endpoint
/// that answers with more is answering something else.
const MAX_DOWNLOAD: u64 = 256 * 1024 * 1024;

fn http_get_bytes(url: &str) -> Result<Vec<u8>, String> {
    let response = ureq::get(url)
        // The GitHub API requires a User-Agent; ours says only what asked.
        .header(
            "user-agent",
            concat!("token-station-cli/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|error| format!("GET {url}: {error}"))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("GET {url}: HTTP {status}"));
    }
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_with_config()
        .limit(MAX_DOWNLOAD)
        .reader()
        .read_to_end(&mut bytes)
        .map_err(|error| format!("GET {url}: {error}"))?;
    Ok(bytes)
}

fn http_get_string(url: &str) -> Result<String, String> {
    String::from_utf8(http_get_bytes(url)?).map_err(|_| format!("GET {url}: not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::{is_newer, parse_version};

    #[test]
    fn version_comparison_is_numeric_not_lexicographic() {
        assert!(is_newer("0.1.0", "v0.2.0"));
        assert!(is_newer("0.9.0", "v0.10.0"), "lexicographic would say no");
        assert!(!is_newer("0.1.0", "v0.1.0"));
        assert!(!is_newer("0.2.0", "v0.1.9"));
        assert!(is_newer("0.1.0", "1.0.0"), "the v prefix is optional");
    }

    #[test]
    fn an_unparseable_tag_never_becomes_an_upgrade_prompt() {
        assert!(!is_newer("0.1.0", "nightly"));
        assert!(
            !is_newer("0.1.0", "v1.2"),
            "two components is not our shape"
        );
        assert!(!is_newer("0.1.0", "v1.2.3.4"));
        assert_eq!(parse_version("1.2.3"), Some([1, 2, 3]));
    }
}

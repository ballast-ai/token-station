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

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use token_station_release::{ReleaseManifest, parse_public_key, verify_bytes};

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
    let body = http_get_string(&url, MAX_METADATA_DOWNLOAD)?;
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
    let manifest_bytes = http_get_bytes(
        &asset("manifest.json")?.browser_download_url,
        MAX_METADATA_DOWNLOAD,
    )?;
    let signature = http_get_string(
        &asset("manifest.json.sig")?.browser_download_url,
        MAX_SIGNATURE_DOWNLOAD,
    )?;
    verify_bytes(&key, &manifest_bytes, &signature)
        .map_err(|error| format!("release manifest does not verify: {error}"))?;
    let manifest = ReleaseManifest::parse(&manifest_bytes)?;
    let expected = manifest
        .sha256_of(&artifact_name)
        .ok_or_else(|| format!("the signed manifest does not list `{artifact_name}`"))?;

    let path = target_dir.join(&artifact_name);
    if path.exists() {
        return Err(format!(
            "`{}` already exists; refusing to overwrite it",
            path.display()
        ));
    }
    download_verified_artifact(
        &asset(&artifact_name)?.browser_download_url,
        &path,
        expected,
    )?;
    Ok(path)
}

/// Downloads are capped: a release artifact is megabytes, and an endpoint
/// that answers with more is answering something else.
const MAX_DOWNLOAD: u64 = 256 * 1024 * 1024;
const MAX_METADATA_DOWNLOAD: u64 = 1024 * 1024;
const MAX_SIGNATURE_DOWNLOAD: u64 = 64 * 1024;

fn http_response(url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    let http = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(None)
            .build(),
    );
    let response = http
        .get(url)
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
    Ok(response)
}

fn http_get_bytes(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let response = http_response(url)?;
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_with_config()
        .limit(limit + 1)
        .reader()
        .read_to_end(&mut bytes)
        .map_err(|error| format!("GET {url}: {error}"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(format!(
            "GET {url}: response exceeds the {limit} byte limit"
        ));
    }
    Ok(bytes)
}

fn http_get_string(url: &str, limit: u64) -> Result<String, String> {
    String::from_utf8(http_get_bytes(url, limit)?).map_err(|_| format!("GET {url}: not UTF-8"))
}

fn download_verified_artifact(url: &str, path: &Path, expected: &str) -> Result<(), String> {
    std::fs::create_dir_all(
        path.parent()
            .ok_or_else(|| format!("download target `{}` has no parent", path.display()))?,
    )
    .map_err(|error| format!("create download directory: {error}"))?;
    let temporary = temporary_download_path(path)?;
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| format!("write `{}`: {error}", temporary.display()))?;
        let mut reader = http_response(url)?
            .into_body()
            .into_with_config()
            .limit(MAX_DOWNLOAD + 1)
            .reader();
        let mut hasher = Sha256::new();
        let mut total = 0_u64;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|error| format!("GET {url}: {error}"))?;
            if read == 0 {
                break;
            }
            total = total
                .checked_add(read as u64)
                .ok_or_else(|| "download size overflow".to_owned())?;
            if total > MAX_DOWNLOAD {
                return Err(format!(
                    "GET {url}: artifact exceeds the {MAX_DOWNLOAD} byte limit"
                ));
            }
            hasher.update(&buffer[..read]);
            file.write_all(&buffer[..read])
                .map_err(|error| format!("write `{}`: {error}", temporary.display()))?;
        }
        file.flush()
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("sync `{}`: {error}", temporary.display()))?;
        let actual = format!("{:x}", hasher.finalize());
        if actual != expected {
            return Err(format!(
                "`{}` does not match the signed manifest (expected {expected}, got {actual}); the download was discarded",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
        std::fs::hard_link(&temporary, path).map_err(|error| {
            format!(
                "publish verified artifact `{}` without overwrite: {error}",
                path.display()
            )
        })?;
        std::fs::remove_file(&temporary)
            .map_err(|error| format!("remove `{}`: {error}", temporary.display()))
    })();
    if result.is_err() {
        std::fs::remove_file(&temporary).ok();
    }
    result
}

fn temporary_download_path(path: &Path) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| format!("invalid download target `{}`", path.display()))?;
    for _ in 0..16 {
        let mut random = [0_u8; 12];
        getrandom::fill(&mut random).map_err(|error| format!("randomness: {error}"))?;
        let suffix = random.iter().fold(String::new(), |mut output, byte| {
            use std::fmt::Write as _;
            let _ = write!(output, "{byte:02x}");
            output
        });
        let temporary = path.with_file_name(format!(".{name}.{suffix}.partial"));
        if !temporary.exists() {
            return Ok(temporary);
        }
    }
    Err("could not allocate a unique download staging file".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        OFFICIAL_RELEASE_PUBKEY_HEX, Release, download_and_verify, is_newer, parse_version,
    };

    #[test]
    fn an_empty_release_key_fails_closed_before_any_download() {
        let release = Release {
            tag_name: "v9.9.9".to_owned(),
            html_url: "https://example.invalid/release".to_owned(),
            assets: Vec::new(),
        };
        let error = download_and_verify(&release, &std::env::temp_dir(), "")
            .expect_err("an empty key must refuse, never accept an unsigned binary");
        assert!(error.contains("no release public key"), "{error}");
    }

    #[test]
    fn this_build_ships_no_release_key_so_upgrades_are_fail_closed() {
        // Tripwire for the honesty fix: while the embedded key is empty, the
        // README says so and `upgrade` refuses. If a reviewed release build ever
        // injects a real key, this test fails — a deliberate reminder to restore
        // the README claim that the public key is in the source tree.
        assert!(
            OFFICIAL_RELEASE_PUBKEY_HEX.is_empty(),
            "a real key is embedded now — update README/README.zh-CN to match"
        );
    }

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

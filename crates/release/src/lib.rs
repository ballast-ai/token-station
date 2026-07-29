//! The release trust chain (C1#7): a versioned manifest of artifact hashes,
//! signed with Ed25519.
//!
//! Two parties share this crate and therefore cannot fork the format:
//!
//! - the **signing side** — the `ts-release` binary, run where the private
//!   key lives (the closed release pipeline; the key never appears in this
//!   repository or its CI);
//! - the **verifying side** — `token-station-cli upgrade`, with the public
//!   key compiled in, and anyone auditing a download by hand.
//!
//! # What a signature means
//!
//! The signature covers the manifest file's **exact bytes** — not a
//! re-serialization — so verification is `verify(bytes, sig)` with no
//! canonicalization step to get subtly wrong. The manifest lists artifact
//! hashes; together they say "the publisher who holds the key produced these
//! bytes". What they deliberately do *not* say is "these bytes are good":
//! that is the reproducible build's half of the trust chain — the signature
//! proves the publisher, the rebuild proves the source.

use std::fmt::Write as _;
use std::io::Read;
use std::path::Path;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The manifest shape's version. A verifier that meets a number it does not
/// know must refuse, not guess.
pub const FORMAT_VERSION: u32 = 1;

const MAX_PLUGIN_FILES: usize = 10_000;
const MAX_PLUGIN_DEPTH: usize = 16;
const MAX_PLUGIN_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PLUGIN_PACKAGE_BYTES: u64 = 128 * 1024 * 1024;

/// One release: what was published, hashed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    pub format_version: u32,
    /// The released version, e.g. `0.1.0` (no `v` prefix).
    pub version: String,
    /// Unix seconds; from `SOURCE_DATE_EPOCH` in the release pipeline, so the
    /// manifest itself is reproducible.
    pub created_unix: u64,
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    /// File name only — no paths; a manifest naming `../` is malformed.
    pub name: String,
    /// Lowercase hex SHA-256 of the file's bytes.
    pub sha256: String,
}

impl ReleaseManifest {
    /// Hashes `files` into a manifest, ordered by file name so two runs over
    /// the same set produce identical bytes.
    ///
    /// # Errors
    ///
    /// An unreadable file, or a path without a UTF-8 file name.
    pub fn for_files(version: &str, created_unix: u64, files: &[&Path]) -> Result<Self, String> {
        let mut artifacts = files
            .iter()
            .map(|path| {
                let name = path
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .ok_or_else(|| format!("`{}` has no UTF-8 file name", path.display()))?;
                Ok(Artifact {
                    name: name.to_owned(),
                    sha256: sha256_file(path)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        artifacts.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(Self {
            format_version: FORMAT_VERSION,
            version: version.to_owned(),
            created_unix,
            artifacts,
        })
    }

    /// Parses manifest bytes, refusing unknown format versions.
    ///
    /// # Errors
    ///
    /// Malformed JSON, unknown fields, or a format version this build does
    /// not know.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let manifest: Self =
            serde_json::from_slice(bytes).map_err(|error| format!("manifest: {error}"))?;
        if manifest.format_version != FORMAT_VERSION {
            return Err(format!(
                "manifest format version {} is not {FORMAT_VERSION}; upgrade the verifier",
                manifest.format_version
            ));
        }
        Ok(manifest)
    }

    /// The expected hash for `name`, if the manifest lists it.
    #[must_use]
    pub fn sha256_of(&self, name: &str) -> Option<&str> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.name == name)
            .map(|artifact| artifact.sha256.as_str())
    }
}

/// Lowercase hex SHA-256 of a file.
///
/// # Errors
///
/// The file could not be read.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        std::fs::File::open(path).map_err(|error| format!("read `{}`: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("read `{}`: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

/// A fresh Ed25519 signing key from the OS entropy source.
///
/// # Errors
///
/// The entropy source failed.
pub fn keygen() -> Result<SigningKey, String> {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|error| format!("entropy: {error}"))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Loads a signing key from its hex seed file (as `keygen` wrote it).
///
/// # Errors
///
/// An unreadable file or a seed that is not 32 hex-encoded bytes.
pub fn load_signing_key(path: &Path) -> Result<SigningKey, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("key `{}`: {error}", path.display()))?;
    let seed: [u8; 32] = unhex(text.trim())?
        .try_into()
        .map_err(|_| format!("key `{}` is not 32 bytes", path.display()))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Parses a lowercase-hex Ed25519 public key.
///
/// # Errors
///
/// Not 32 hex-encoded bytes, or not a valid curve point.
pub fn parse_public_key(hex_key: &str) -> Result<VerifyingKey, String> {
    let bytes: [u8; 32] = unhex(hex_key.trim())?
        .try_into()
        .map_err(|_| "public key is not 32 bytes".to_owned())?;
    VerifyingKey::from_bytes(&bytes).map_err(|error| format!("public key: {error}"))
}

/// Signs raw bytes; returns the signature as lowercase hex.
#[must_use]
pub fn sign_bytes(key: &SigningKey, bytes: &[u8]) -> String {
    hex(&key.sign(bytes).to_bytes())
}

/// Verifies a hex signature over raw bytes.
///
/// # Errors
///
/// A malformed signature, or one that does not match — the two cases are
/// deliberately not distinguished for the caller.
pub fn verify_bytes(key: &VerifyingKey, bytes: &[u8], signature_hex: &str) -> Result<(), String> {
    let raw: [u8; 64] = unhex(signature_hex.trim())?
        .try_into()
        .map_err(|_| "signature is not 64 bytes".to_owned())?;
    key.verify(bytes, &Signature::from_bytes(&raw))
        .map_err(|_| "signature does not verify".to_owned())
}

/// The canonical digest of a plugin package: every file's name and SHA-256,
/// sorted by name, one `name\nsha256\n` pair per file. `signature.sig` itself
/// is excluded — it cannot cover itself.
///
/// Signing this digest (rather than an archive) keeps the signature valid
/// however the package directory reached the user's disk.
///
/// # Errors
///
/// An unreadable package directory or file.
pub fn plugin_package_digest(package_dir: &Path) -> Result<String, String> {
    let root_metadata = std::fs::symlink_metadata(package_dir)
        .map_err(|error| format!("package `{}`: {error}", package_dir.display()))?;
    if !root_metadata.file_type().is_dir() {
        return Err(format!(
            "package `{}` is not a directory",
            package_dir.display()
        ));
    }

    let mut files = Vec::new();
    collect_plugin_files(package_dir, package_dir, 0, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Err(format!("package `{}` is empty", package_dir.display()));
    }

    let mut total_bytes = 0_u64;
    let mut digest = String::new();
    for relative in files {
        let path = package_dir.join(&relative);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("package entry `{}`: {error}", path.display()))?;
        if !metadata.file_type().is_file() {
            return Err(format!(
                "package entry `{}` is not a regular file",
                path.display()
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(format!(
                    "package entry `{}` has multiple hard links",
                    path.display()
                ));
            }
        }
        if metadata.len() > MAX_PLUGIN_FILE_BYTES {
            return Err(format!(
                "package entry `{}` exceeds the {} byte file limit",
                path.display(),
                MAX_PLUGIN_FILE_BYTES
            ));
        }
        total_bytes = total_bytes.saturating_add(metadata.len());
        if total_bytes > MAX_PLUGIN_PACKAGE_BYTES {
            return Err(format!(
                "package exceeds the {MAX_PLUGIN_PACKAGE_BYTES} byte total limit"
            ));
        }
        let relative = relative
            .to_str()
            .ok_or_else(|| "package file name is not UTF-8".to_owned())?
            .replace(std::path::MAIN_SEPARATOR, "/");
        let _ = writeln!(digest, "{relative}\n{}", sha256_file(&path)?);
    }
    Ok(digest)
}

fn collect_plugin_files(
    root: &Path,
    directory: &Path,
    depth: usize,
    files: &mut Vec<std::path::PathBuf>,
) -> Result<(), String> {
    if depth > MAX_PLUGIN_DEPTH {
        return Err(format!(
            "package directory depth exceeds the {MAX_PLUGIN_DEPTH} level limit"
        ));
    }
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("package directory `{}`: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("package entry: {error}"))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| format!("package entry `{}` escaped its root", path.display()))?;
        if relative == Path::new(PLUGIN_SIGNATURE_FILE) {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("package entry `{}`: {error}", path.display()))?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return Err(format!(
                "package entry `{}` is a symbolic link; symbolic links are forbidden",
                path.display()
            ));
        }
        if file_type.is_dir() {
            collect_plugin_files(root, &path, depth + 1, files)?;
        } else if file_type.is_file() {
            files.push(relative.to_path_buf());
            if files.len() > MAX_PLUGIN_FILES {
                return Err(format!(
                    "package contains more than {MAX_PLUGIN_FILES} files"
                ));
            }
        } else {
            return Err(format!(
                "package entry `{}` is not a regular file or directory",
                path.display()
            ));
        }
    }
    Ok(())
}

/// The signature file inside a plugin package.
pub const PLUGIN_SIGNATURE_FILE: &str = "signature.sig";

/// What `signature.sig` holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSignature {
    pub format_version: u32,
    pub algorithm: String,
    /// Hex Ed25519 signature over [`plugin_package_digest`].
    pub signature: String,
}

/// Signs a plugin package directory; returns the `signature.sig` contents.
///
/// # Errors
///
/// See [`plugin_package_digest`].
pub fn sign_plugin_package(key: &SigningKey, package_dir: &Path) -> Result<String, String> {
    let digest = plugin_package_digest(package_dir)?;
    let signature = PluginSignature {
        format_version: FORMAT_VERSION,
        algorithm: "ed25519".to_owned(),
        signature: sign_bytes(key, digest.as_bytes()),
    };
    serde_json::to_string_pretty(&signature).map_err(|error| error.to_string())
}

/// Verifies a plugin package against its `signature.sig`.
///
/// # Errors
///
/// A missing or malformed signature file, an unknown format or algorithm, or
/// a package whose current bytes do not match the signed digest.
pub fn verify_plugin_package(key: &VerifyingKey, package_dir: &Path) -> Result<(), String> {
    let raw = std::fs::read_to_string(package_dir.join(PLUGIN_SIGNATURE_FILE))
        .map_err(|error| format!("{PLUGIN_SIGNATURE_FILE}: {error}"))?;
    let parsed: PluginSignature =
        serde_json::from_str(&raw).map_err(|error| format!("{PLUGIN_SIGNATURE_FILE}: {error}"))?;
    if parsed.format_version != FORMAT_VERSION {
        return Err(format!(
            "{PLUGIN_SIGNATURE_FILE} format version {} is not {FORMAT_VERSION}",
            parsed.format_version
        ));
    }
    if parsed.algorithm != "ed25519" {
        return Err(format!("unknown algorithm `{}`", parsed.algorithm));
    }
    let digest = plugin_package_digest(package_dir)?;
    verify_bytes(key, digest.as_bytes(), &parsed.signature)
}

/// Lowercase hex. Small enough to own; a dependency would be another link in
/// exactly the chain this crate exists to keep short.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Hex to bytes; rejects odd lengths and non-hex characters.
///
/// # Errors
///
/// Names what disqualified the input, never echoing it in full (a mistyped
/// paste may be a secret).
pub fn unhex(text: &str) -> Result<Vec<u8>, String> {
    // `% 2` rather than `is_multiple_of`: the latter postdates the 1.85 MSRV.
    if text.len() % 2 != 0 {
        return Err("hex input has odd length".to_owned());
    }
    (0..text.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&text[index..index + 2], 16)
                .map_err(|_| "hex input has non-hex characters".to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        FORMAT_VERSION, ReleaseManifest, keygen, parse_public_key, plugin_package_digest,
        sign_bytes, sign_plugin_package, verify_bytes, verify_plugin_package,
    };
    use std::path::PathBuf;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ts-release-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_manifest_signs_and_verifies_round_trip() {
        let dir = scratch_dir("roundtrip");
        std::fs::write(dir.join("b.tar.gz"), b"artifact-b").expect("writes");
        std::fs::write(dir.join("a.tar.gz"), b"artifact-a").expect("writes");

        let manifest = ReleaseManifest::for_files(
            "0.1.0",
            1_752_000_000,
            &[&dir.join("b.tar.gz"), &dir.join("a.tar.gz")],
        )
        .expect("hashes");
        assert_eq!(
            manifest.artifacts[0].name, "a.tar.gz",
            "sorted by name, not argument order"
        );

        let bytes = serde_json::to_vec_pretty(&manifest).expect("serializes");
        let key = keygen().expect("entropy");
        let signature = sign_bytes(&key, &bytes);

        let parsed = ReleaseManifest::parse(&bytes).expect("parses");
        assert_eq!(parsed, manifest);
        verify_bytes(&key.verifying_key(), &bytes, &signature).expect("verifies");
    }

    #[test]
    fn one_flipped_byte_breaks_the_signature() {
        let key = keygen().expect("entropy");
        let bytes = b"the exact bytes are what is signed".to_vec();
        let signature = sign_bytes(&key, &bytes);

        let mut tampered = bytes.clone();
        tampered[0] ^= 1;
        assert!(verify_bytes(&key.verifying_key(), &tampered, &signature).is_err());
        // And an honest verifier with the wrong key learns nothing either.
        let other = keygen().expect("entropy");
        assert!(verify_bytes(&other.verifying_key(), &bytes, &signature).is_err());
    }

    #[test]
    fn an_unknown_format_version_is_refused_not_guessed() {
        let manifest = serde_json::json!({
            "format_version": FORMAT_VERSION + 1,
            "version": "9.9.9",
            "created_unix": 0,
            "artifacts": []
        });
        let error = ReleaseManifest::parse(manifest.to_string().as_bytes())
            .expect_err("a future format is not ours to interpret");
        assert!(error.contains("format version"), "{error}");
    }

    #[test]
    fn a_public_key_round_trips_through_hex() {
        let key = keygen().expect("entropy");
        let hex_key = super::hex(key.verifying_key().as_bytes());

        let parsed = parse_public_key(&hex_key).expect("parses");
        assert_eq!(parsed, key.verifying_key());

        assert!(parse_public_key("zz").is_err());
        assert!(parse_public_key("abc").is_err(), "odd length");
    }

    #[test]
    fn a_plugin_package_signs_and_a_swapped_wasm_is_caught() {
        let dir = scratch_dir("plugin");
        std::fs::write(dir.join("manifest.json"), b"{\"name\":\"p\"}").expect("writes");
        std::fs::write(dir.join("adapter.wasm"), b"\0asm-original").expect("writes");

        let key = keygen().expect("entropy");
        let signature = sign_plugin_package(&key, &dir).expect("signs");
        std::fs::write(dir.join(super::PLUGIN_SIGNATURE_FILE), &signature).expect("writes");

        verify_plugin_package(&key.verifying_key(), &dir).expect("verifies");

        // The attack this exists for: same manifest, different code.
        std::fs::write(dir.join("adapter.wasm"), b"\0asm-swapped").expect("writes");
        assert!(verify_plugin_package(&key.verifying_key(), &dir).is_err());
    }

    #[test]
    fn the_package_digest_excludes_the_signature_file_itself() {
        let dir = scratch_dir("digest");
        std::fs::write(dir.join("manifest.json"), b"{}").expect("writes");

        let before = plugin_package_digest(&dir).expect("digests");
        std::fs::write(dir.join(super::PLUGIN_SIGNATURE_FILE), b"anything").expect("writes");
        let after = plugin_package_digest(&dir).expect("digests");

        assert_eq!(before, after, "signing must not invalidate itself");
    }

    #[test]
    fn package_digest_recursively_binds_fixture_paths_and_bytes() {
        let dir = scratch_dir("recursive-digest");
        std::fs::create_dir_all(dir.join("fixtures/nested")).expect("fixture tree");
        std::fs::write(dir.join("manifest.json"), b"{}").expect("manifest");
        std::fs::write(dir.join("adapter.wasm"), b"wasm").expect("wasm");
        std::fs::write(dir.join("fixtures/nested/case.json"), b"{\"version\":1}").expect("fixture");

        let before = plugin_package_digest(&dir).expect("a real plugin tree digests");
        std::fs::write(dir.join("fixtures/nested/case.json"), b"{\"version\":2}")
            .expect("fixture changes");
        let after = plugin_package_digest(&dir).expect("changed tree digests");

        assert_ne!(
            before, after,
            "a receipt or signature must bind every recursive fixture byte"
        );
    }

    #[cfg(unix)]
    #[test]
    fn package_digest_rejects_symlinks_instead_of_following_them() {
        use std::os::unix::fs::symlink;

        let dir = scratch_dir("special-entry");
        std::fs::create_dir_all(dir.join("fixtures")).expect("fixture tree");
        std::fs::write(dir.join("outside.json"), b"outside").expect("target");
        symlink("../outside.json", dir.join("fixtures/case.json")).expect("symlink");

        let error =
            plugin_package_digest(&dir).expect_err("package symlinks are not trusted bytes");
        assert!(error.contains("symbolic links are forbidden"), "{error}");
    }
}

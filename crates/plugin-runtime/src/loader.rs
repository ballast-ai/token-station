//! The load gates both adapter kinds share, in the order both run them.
//!
//! Kind-specific work — which world to instantiate, which host interfaces the
//! linker offers — stays in `provider.rs` / `agent.rs`. What lives here is the
//! part where the two kinds must not be allowed to drift: how a package is
//! read, which gate runs first, and what the sandbox refuses by name.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use token_station_conformance::accepts_manifest;
use token_station_plugin_api::{AdapterKind, AdapterManifest, AdapterMetadata, ManifestError};
use token_station_protocol::{ErrorCode, ErrorEnvelope};
use wasmtime::component::{Component, ResourceTable};
use wasmtime::{StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::provider::SecretSigner;
use crate::runtime::PluginRuntime;

const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
const MAX_COMPONENT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ABI_JSON_BYTES: usize = 16 * 1024 * 1024;

/// Interface prefixes no adapter of any kind may import.
///
/// The rest of WASI gets a locked-down implementation because a std-compiled
/// guest imports it whether its author wanted to or not. The network is
/// different: no protocol adapter has any business even *asking*, and an empty
/// implementation would leave "did we actually cut this off" to be re-audited
/// on every wasmtime upgrade. Refusing the import makes the question moot.
const FORBIDDEN_EVERYWHERE: &[&str] = &["wasi:sockets/", "wasi:http/"];

/// Additionally forbidden for `agent-adapter` components.
///
/// Only the provider world imports `host`, and only to name credentials. The
/// WIT declares that; this enforces it on the compiled artifact, so an agent
/// component that somehow acquired the import is refused by name rather than
/// failing instantiation with a generic link error.
pub(crate) const FORBIDDEN_FOR_AGENTS: &[&str] = &["token-station:adapter/host"];

/// Everything one store carries: the locked-down WASI, the resource limits,
/// and the credential boundary for `host.sign`.
///
/// Shared by both kinds. For an agent adapter the credential fields are inert
/// by construction: its linker never registers the `host` interface, so
/// nothing in the instance can reach them.
pub(crate) struct Ctx {
    wasi: WasiCtx,
    table: ResourceTable,
    pub(crate) limits: StoreLimits,
    /// The manifest's `permissions.secrets`, the only names `sign` may see.
    pub(crate) declared_secrets: BTreeSet<String>,
    pub(crate) signer: Arc<dyn SecretSigner + Sync>,
}

impl Ctx {
    pub(crate) fn new(
        memory_bytes: usize,
        declared_secrets: BTreeSet<String>,
        signer: Arc<dyn SecretSigner + Sync>,
    ) -> Self {
        // No preopened directories, no environment, no arguments, no inherited
        // stdio: WASI is provided so a std-compiled guest can instantiate, and
        // it opens onto nothing.
        Self {
            wasi: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new().memory_size(memory_bytes).build(),
            declared_secrets,
            signer,
        }
    }
}

impl WasiView for Ctx {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// Why a plugin package was refused at load.
///
/// Ordered like the gates: a package that fails early is reported for the
/// early failure, so a broken manifest is not reported as a sandbox violation.
#[derive(Debug)]
pub enum LoadError {
    /// The package directory, `manifest.json` or `adapter.wasm` could not be
    /// read.
    Unreadable { path: PathBuf, detail: String },
    /// `manifest.json` is not a manifest.
    ManifestSyntax(serde_json::Error),
    /// The manifest gate refused it.
    Manifest(ManifestError),
    /// The manifest declares a different adapter kind than this loader loads.
    WrongKind(AdapterKind),
    /// The bytes are not a WASM component, or do not export the expected
    /// world.
    NotAnAdapter(wasmtime::Error),
    /// The component imports an interface the sandbox will never provide.
    ForbiddenImport(String),
    /// The identity gate refused it: `metadata()` disagrees with the manifest.
    IdentityMismatch {
        declared: Box<AdapterMetadata>,
        reported: Box<AdapterMetadata>,
    },
    /// The adapter trapped or failed while being probed at load.
    Probe(wasmtime::Error),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { path, detail } => {
                write!(f, "cannot read `{}`: {detail}", path.display())
            }
            Self::ManifestSyntax(detail) => write!(f, "manifest.json is not a manifest: {detail}"),
            Self::Manifest(error) => write!(f, "manifest refused: {error}"),
            Self::WrongKind(kind) => {
                write!(
                    f,
                    "the manifest declares {kind:?}, which this loader does not load"
                )
            }
            Self::NotAnAdapter(error) => write!(f, "not an adapter component: {error}"),
            Self::ForbiddenImport(name) => write!(
                f,
                "component imports `{name}`, which the sandbox will never provide"
            ),
            Self::IdentityMismatch { declared, reported } => write!(
                f,
                "adapter reports {}/{} but its manifest declares {}/{}; \
                 the package is not what it claims to be",
                reported.name, reported.version, declared.name, declared.version
            ),
            Self::Probe(error) => write!(f, "adapter failed while being probed at load: {error}"),
        }
    }
}

impl Error for LoadError {}

/// Gates 1 and 2: the manifest, then the compiled component and its imports.
///
/// Gate order is observable: the manifest is judged before `adapter.wasm` is
/// even opened, so a package with a violating manifest and no wasm at all is
/// refused for the manifest.
pub(crate) fn read_package(
    runtime: &PluginRuntime,
    dir: &Path,
    expected_kind: AdapterKind,
    extra_forbidden: &[&str],
) -> Result<(AdapterManifest, Component), LoadError> {
    let manifest_path = dir.join("manifest.json");
    let manifest_bytes =
        read_file_limited(&manifest_path, MAX_MANIFEST_BYTES).map_err(|error| {
            LoadError::Unreadable {
                path: manifest_path.clone(),
                detail: error,
            }
        })?;
    let manifest_source =
        String::from_utf8(manifest_bytes).map_err(|error| LoadError::Unreadable {
            path: manifest_path,
            detail: error.to_string(),
        })?;
    let manifest = gate_manifest(&manifest_source, expected_kind)?;

    let wasm_path = dir.join("adapter.wasm");
    let wasm = read_file_limited(&wasm_path, MAX_COMPONENT_BYTES).map_err(|error| {
        LoadError::Unreadable {
            path: wasm_path,
            detail: error,
        }
    })?;
    let component = gate_component(runtime, &wasm, extra_forbidden)?;

    Ok((manifest, component))
}

fn read_file_limited(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if metadata.len() > limit {
        return Err(format!("file exceeds the {limit} byte limit"));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(format!("file exceeds the {limit} byte limit"));
    }
    Ok(bytes)
}

/// The filesystem-free twin of [`read_package`]: same gates, same order, for
/// package bytes that were compiled in (the builtin tier) rather than read
/// from a directory. Builtin packages earn no shortcut — an embedded component
/// that imports the network is refused exactly like a downloaded one.
pub(crate) fn parse_package(
    runtime: &PluginRuntime,
    manifest_source: &str,
    wasm: &[u8],
    expected_kind: AdapterKind,
    extra_forbidden: &[&str],
) -> Result<(AdapterManifest, Component), LoadError> {
    let manifest = gate_manifest(manifest_source, expected_kind)?;
    let component = gate_component(runtime, wasm, extra_forbidden)?;
    Ok((manifest, component))
}

/// Gate 1: the manifest, judged before any wasm is even opened.
fn gate_manifest(
    manifest_source: &str,
    expected_kind: AdapterKind,
) -> Result<AdapterManifest, LoadError> {
    let manifest: AdapterManifest =
        serde_json::from_str(manifest_source).map_err(LoadError::ManifestSyntax)?;
    accepts_manifest(&manifest).map_err(LoadError::Manifest)?;
    if manifest.kind != expected_kind {
        return Err(LoadError::WrongKind(manifest.kind));
    }
    Ok(manifest)
}

/// Gate 2: the compiled component and its imports.
fn gate_component(
    runtime: &PluginRuntime,
    wasm: &[u8],
    extra_forbidden: &[&str],
) -> Result<Component, LoadError> {
    let component = Component::new(runtime.engine(), wasm).map_err(LoadError::NotAnAdapter)?;

    for (name, _) in component.component_type().imports(runtime.engine()) {
        let refused = FORBIDDEN_EVERYWHERE
            .iter()
            .chain(extra_forbidden)
            .any(|prefix| name.starts_with(prefix));
        if refused {
            return Err(LoadError::ForbiddenImport(name.to_owned()));
        }
    }

    Ok(component)
}

/// The guest's error side is a `protocol::ErrorEnvelope` as JSON. A guest that
/// returns something else on its error channel gets an `internal` envelope
/// quoting it, so the failure is still attributed to the adapter.
pub(crate) fn parse_error_envelope(error_json: &str) -> ErrorEnvelope {
    if error_json.len() > MAX_ABI_JSON_BYTES {
        return ErrorEnvelope::new(
            ErrorCode::Internal,
            500,
            "adapter error output exceeded the host limit",
        );
    }
    serde_json::from_str(error_json).unwrap_or_else(|_| {
        ErrorEnvelope::new(
            ErrorCode::Internal,
            500,
            "adapter returned a malformed error envelope",
        )
    })
}

pub(crate) fn trap_envelope(_: &wasmtime::Error) -> ErrorEnvelope {
    ErrorEnvelope::new(
        ErrorCode::Internal,
        500,
        "adapter trapped or exceeded a resource limit",
    )
}

pub(crate) fn to_json<T: serde::Serialize>(value: &T) -> Result<String, ErrorEnvelope> {
    serde_json::to_string(value).map_err(|error| {
        ErrorEnvelope::new(
            ErrorCode::Internal,
            500,
            format!("could not serialize the canonical form: {error}"),
        )
    })
}

pub(crate) fn from_json<T: for<'de> serde::Deserialize<'de>>(
    json: &str,
) -> Result<T, ErrorEnvelope> {
    if json.len() > MAX_ABI_JSON_BYTES {
        return Err(ErrorEnvelope::new(
            ErrorCode::Internal,
            500,
            "adapter JSON output exceeded the host limit",
        ));
    }
    serde_json::from_str(json).map_err(|error| {
        ErrorEnvelope::new(
            ErrorCode::Internal,
            500,
            format!("adapter returned JSON that is not the canonical form: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn adapter_json_output_is_bounded_before_parsing() {
        let oversized = serde_json::to_string(&"x".repeat(16 * 1024 * 1024 + 1))
            .expect("test string serializes");
        let error = super::from_json::<String>(&oversized)
            .expect_err("a guest may not force an unbounded host allocation");
        assert!(error.message.contains("limit"), "{}", error.message);
    }
}

//! The southbound seam: South's v2 provider components in this host's adapter
//! layer.
//!
//! Every southbound translation runs through a `provider-adapter-v2` component
//! loaded by `south-provider-runtime`. [`ProviderAdapter`] is the typed face
//! the gateway calls; [`SouthComponentAdapter`] implements it over the South
//! runtime's JSON face, serializing this host's own Canonical IR types — the
//! runtime itself never parses the IR, so the host side of the boundary lives
//! here. Nothing in this module opens a socket: translation and transport are
//! separate seams (see `south_provider_call`).

use std::io::Read;
use std::path::Path;
use std::sync::Mutex;

use std::collections::{BTreeMap, BTreeSet};

use south_provider_api::{ComponentManifestV1, HostRangeV1, RUNTIME_ABI};
use south_provider_runtime::{
    CallErrorV1, ComponentRuntimeV1, ComponentStreamV1, HostCompatibilityV1, LoadedComponentV1,
    NoSecretsV1, RuntimeLimitsV1,
};
use token_station_protocol::{
    ChatRequest, ChatResponse, ErrorCode, ErrorEnvelope, HttpRequestDescriptor, HttpResponseParts,
    ModelCapability, ProviderConfig, StreamChunk, StreamEvent,
};

/// What an adapter returns. The error is the adapter's own [`ErrorEnvelope`],
/// which is also what the runtime reports when a component traps.
pub type AdapterResult<T> = Result<T, ErrorEnvelope>;

/// One provider stream, mid-parse.
///
/// Streaming is the only stateful part of the ABI. A chunk off the socket is
/// not a whole SSE frame, so an adapter must hold the tail until the rest
/// arrives. The component runtime keeps that state in a per-stream instance,
/// which is why this hands out a fresh parser per stream rather than
/// pretending the call is pure.
pub trait StreamParser {
    /// Consumes one fragment and emits whatever complete events it completed.
    ///
    /// Zero events is a normal answer: the fragment ended mid-frame.
    ///
    /// # Errors
    ///
    /// Returns the envelope the caller should be answered with.
    fn parse_chunk(&mut self, chunk: &StreamChunk) -> AdapterResult<Vec<StreamEvent>>;

    /// Flushes a clean transport EOF, represented as an empty fragment, which a
    /// successful network read can never produce.
    ///
    /// # Errors
    ///
    /// Returns a typed protocol failure when buffered state cannot finish.
    fn finish(&mut self) -> AdapterResult<Vec<StreamEvent>> {
        self.parse_chunk(&StreamChunk {
            data: String::new(),
        })
    }
}

/// Southbound: the Canonical IR, in and out of one provider's HTTP dialect.
pub trait ProviderAdapter {
    /// # Errors
    ///
    /// Returns the envelope the caller should be answered with.
    fn model_capabilities(&self, config: &ProviderConfig) -> AdapterResult<Vec<ModelCapability>>;

    /// # Errors
    ///
    /// Returns the envelope the caller should be answered with.
    fn build_http_request(
        &self,
        request: &ChatRequest,
        config: &ProviderConfig,
    ) -> AdapterResult<HttpRequestDescriptor>;

    /// # Errors
    ///
    /// Returns the envelope the caller should be answered with.
    fn parse_response(&self, parts: &HttpResponseParts) -> AdapterResult<ChatResponse>;

    /// Maps a failed upstream response onto the stable error catalog.
    ///
    /// # Errors
    ///
    /// Returns the envelope the caller should be answered with. An adapter that
    /// cannot classify a failure returns `Ok(ErrorEnvelope { code: Internal, .. })`
    /// rather than `Err`; `Err` here means the mapping itself broke.
    fn map_provider_error(&self, parts: &HttpResponseParts) -> AdapterResult<ErrorEnvelope>;

    /// A parser for one stream. Called once per exchange.
    fn stream_parser(&self) -> Box<dyn StreamParser>;
}

/// One process-wide South engine for every v2 component instance.
///
/// # Errors
///
/// Returns the engine construction error message.
pub fn south_component_runtime() -> Result<ComponentRuntimeV1, String> {
    ComponentRuntimeV1::new(RuntimeLimitsV1::default())
        .map_err(|error| format!("south component runtime: {error:#}"))
}

/// What this host admits, declared in one place.
///
/// South judges every component against the host's declaration; the host owns
/// the declaration. These are constants rather than derivations because nothing
/// in the dependency graph carries them: South exposes no release constant. A
/// declaration that nobody checks is what let the old exact tuple rot unnoticed
/// for three releases, so `tests/south-dependency-policy.mjs` asserts these
/// constants against the pinned South release and every shipped manifest.
///
/// The handshake is South's range (B3, `host-zero-vendor-boundary` §8.4): a
/// package must declare this runtime ABI and exactly these kernel contracts,
/// and must have been verified with a South runtime from `SOUTH_RUNTIME_MIN` up
/// to the one this host links. A package built before the range handshake
/// declares no `runtime_abi` and is refused; the exact-tuple arm left with the
/// last such package.
pub(crate) const SOUTH_RUNTIME: &str = "0.85.1";
/// The oldest South runtime a package may declare: the oldest one among the
/// packages this host ships. Raising it is how a host enforces a release that
/// tightened a South gate.
pub(crate) const SOUTH_RUNTIME_MIN: &str = "0.77.0";
/// The kernel contracts of protocol 0.7.0 (kernel mirror `ce6894d`).
pub(crate) const KERNEL_CONTRACTS: [(&str, u32); 3] =
    [("canonical_ir", 5), ("error_catalog", 1), ("stream", 3)];

/// The admission every component is loaded through; see `SOUTH_RUNTIME`.
#[must_use]
pub fn host_admission() -> HostRangeV1 {
    HostRangeV1 {
        runtime_abi: RUNTIME_ABI,
        south_runtime_min: SOUTH_RUNTIME_MIN.to_owned(),
        south_runtime: SOUTH_RUNTIME.to_owned(),
        kernel_contracts: KERNEL_CONTRACTS
            .iter()
            .map(|(name, version)| ((*name).to_owned(), *version))
            .collect(),
        // Provider-world packages declare no South contracts; task, media,
        // image and speech worlds are not served by this host.
        contracts: BTreeMap::new(),
    }
}

/// A v2 component, loaded through the South gates.
pub struct SouthComponentAdapter {
    component: Mutex<Option<LoadedComponentV1>>,
    runtime: ComponentRuntimeV1,
    manifest_source: String,
    wasm: Vec<u8>,
    manifest: ComponentManifestV1,
}

impl SouthComponentAdapter {
    /// Loads embedded package bytes through every South load gate.
    ///
    /// # Errors
    ///
    /// The South loader's refusal, formatted.
    pub fn load_embedded(
        runtime: &ComponentRuntimeV1,
        manifest_source: &str,
        wasm: &[u8],
    ) -> Result<Self, String> {
        let component = LoadedComponentV1::load_embedded(
            runtime,
            manifest_source,
            wasm,
            &host_admission(),
            NoSecretsV1,
        )
        .map_err(|error| format!("south v2 component: {error}"))?;
        let manifest = component.manifest().clone();
        Ok(Self {
            component: Mutex::new(Some(component)),
            runtime: runtime.clone(),
            manifest_source: manifest_source.to_owned(),
            wasm: wasm.to_vec(),
            manifest,
        })
    }

    /// Loads `manifest.json` and `component.wasm` from a package directory
    /// through the same gates as [`Self::load_embedded`].
    ///
    /// # Errors
    ///
    /// The South loader's refusal, formatted with the directory.
    pub fn load_dir(runtime: &ComponentRuntimeV1, dir: &Path) -> Result<Self, String> {
        let snapshot = || {
            let bytes = read_snapshot_file(&dir.join("manifest.json"), 256 * 1024)?;
            let manifest_source = String::from_utf8(bytes)
                .map_err(|error| format!("manifest.json is not UTF-8: {error}"))?;
            let manifest: ComponentManifestV1 = serde_json::from_str(&manifest_source)
                .map_err(|error| format!("manifest.json is not a manifest: {error}"))?;
            manifest
                .validate()
                .map_err(|error| format!("manifest refused: {error}"))?;
            host_admission()
                .admit(&manifest)
                .map_err(|error| error.to_string())?;
            let wasm = read_snapshot_file(&dir.join("component.wasm"), 64 * 1024 * 1024)?;
            Self::load_embedded(runtime, &manifest_source, &wasm)
        };
        snapshot().map_err(|error| format!("south v2 component at {}: {error}", dir.display()))
    }

    fn current<'slot>(
        &self,
        slot: &'slot mut Option<LoadedComponentV1>,
    ) -> AdapterResult<&'slot LoadedComponentV1> {
        if slot.is_none() {
            *slot = Some(
                LoadedComponentV1::load_embedded(
                    &self.runtime,
                    &self.manifest_source,
                    &self.wasm,
                    &host_admission(),
                    NoSecretsV1,
                )
                .map_err(|error| {
                    internal(format_args!("cannot restore provider component: {error}"))
                })?,
            );
        }
        slot.as_ref()
            .ok_or_else(|| internal("provider component is unavailable"))
    }

    fn call(
        &self,
        operation: impl FnOnce(&LoadedComponentV1) -> Result<String, CallErrorV1>,
    ) -> AdapterResult<String> {
        let mut slot = self
            .component
            .lock()
            .map_err(|_| internal("provider component lock is poisoned"))?;
        let result = operation(self.current(&mut slot)?);
        if result.as_ref().is_err_and(invalidates_instance) {
            slot.take();
        }
        result.map_err(|error| seam_error(&error))
    }

    /// The provider dialect families the loaded component's manifest declares.
    #[must_use]
    pub fn dialects(&self) -> Vec<String> {
        self.manifest.providers.clone()
    }

    /// The auth shapes this component's manifest declares it can carry.
    ///
    /// Transport eligibility reads this instead of matching on provider names.
    /// A name-based allowlist has to be edited for every dialect the host ever
    /// admits — and it was, which is why the Anthropic component could translate
    /// a request the transport then refused to send. The manifest already states
    /// the answer, and admission has already verified the manifest.
    #[must_use]
    pub fn auth_arms(&self) -> BTreeSet<String> {
        self.manifest.auth_arms.clone()
    }

    /// The admitted manifest. The transport hands it to South's descriptor auth
    /// rule.
    #[must_use]
    pub fn manifest(&self) -> &ComponentManifestV1 {
        &self.manifest
    }

    /// The secret-bearing header names the manifest declares (`secret_headers`,
    /// auth contract version five), already checked by South's manifest gate.
    #[must_use]
    pub fn secret_headers(&self) -> &[String] {
        &self.manifest.secret_headers
    }

    /// Whether the manifest lets `family` models declare the dialect word `word`
    /// (`model_parameters`, South family-model-parameters record).
    #[must_use]
    pub fn declares_model_parameter(&self, family: &str, word: &str) -> bool {
        self.manifest
            .model_parameters
            .get(family)
            .is_some_and(|parameters| parameters.words.iter().any(|declared| declared == word))
    }

    /// The package name its manifest reports, for diagnostics that have to name
    /// which of several components is being talked about.
    #[must_use]
    pub fn package_name(&self) -> String {
        self.manifest.name.clone()
    }
}

fn read_snapshot_file(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let read = || -> Result<Vec<u8>, String> {
        let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("package entry is not a regular file".into());
        }
        if metadata.len() > limit {
            return Err(format!("file exceeds the {limit} byte limit"));
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path).map_err(|error| error.to_string())?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("package entry is not a regular file".into());
        }
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
    };
    read().map_err(|error| format!("cannot read `{}`: {error}", path.display()))
}

fn invalidates_instance(error: &CallErrorV1) -> bool {
    matches!(error, CallErrorV1::Trap(_) | CallErrorV1::Deadline)
}

fn internal(detail: impl std::fmt::Display) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::Internal, 500, detail.to_string())
}

/// The one place the runtime's opaque component-error payload becomes typed.
fn seam_error(error: &CallErrorV1) -> ErrorEnvelope {
    match error {
        CallErrorV1::Component(error_json) => serde_json::from_str(error_json)
            .unwrap_or_else(|_| internal("component returned a malformed error envelope")),
        other => internal(other),
    }
}

fn to_json<T: serde::Serialize>(value: &T) -> AdapterResult<String> {
    serde_json::to_string(value).map_err(|error| internal(format_args!("serialize: {error}")))
}

fn from_json<T: for<'de> serde::Deserialize<'de>>(json: &str) -> AdapterResult<T> {
    serde_json::from_str(json).map_err(|error| {
        internal(format_args!(
            "component returned JSON that is not the canonical form: {error}"
        ))
    })
}

impl ProviderAdapter for SouthComponentAdapter {
    fn model_capabilities(&self, config: &ProviderConfig) -> AdapterResult<Vec<ModelCapability>> {
        let config = to_json(config)?;
        let out = self.call(|component| component.call_model_capabilities(&config))?;
        from_json(&out)
    }

    fn build_http_request(
        &self,
        request: &ChatRequest,
        config: &ProviderConfig,
    ) -> AdapterResult<HttpRequestDescriptor> {
        let request = to_json(request)?;
        let config = to_json(config)?;
        let out = self.call(|component| component.call_build_http_request(&request, &config))?;
        from_json(&out)
    }

    fn parse_response(&self, parts: &HttpResponseParts) -> AdapterResult<ChatResponse> {
        let parts = to_json(parts)?;
        let out = self.call(|component| component.call_parse_response(&parts))?;
        from_json(&out)
    }

    fn map_provider_error(&self, parts: &HttpResponseParts) -> AdapterResult<ErrorEnvelope> {
        let parts = to_json(parts)?;
        let out = self.call(|component| component.call_map_provider_error(&parts))?;
        from_json(&out)
    }

    fn stream_parser(&self) -> Box<dyn StreamParser> {
        let stream = self
            .component
            .lock()
            .map_err(|_| internal("provider component lock is poisoned"))
            .and_then(|mut slot| {
                self.current(&mut slot)?
                    .open_stream()
                    .map_err(|error| seam_error(&error))
            });
        match stream {
            Ok(stream) => Box::new(SouthComponentStreamParser { stream }),
            Err(error) => Box::new(BrokenStreamParser { envelope: error }),
        }
    }
}

impl std::fmt::Debug for SouthComponentAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SouthComponentAdapter")
            .field("metadata", &self.manifest.metadata())
            .finish_non_exhaustive()
    }
}

struct SouthComponentStreamParser {
    stream: ComponentStreamV1,
}

impl StreamParser for SouthComponentStreamParser {
    fn parse_chunk(&mut self, chunk: &StreamChunk) -> AdapterResult<Vec<StreamEvent>> {
        // The host seam carries UTF-8 chunk text; the v2 ABI carries bytes.
        // Feeding the text's bytes is lossless, and the empty chunk keeps its
        // EOF-flush meaning on both sides.
        let out = self
            .stream
            .parse_chunk(chunk.data.as_bytes())
            .map_err(|error| seam_error(&error))?;
        from_json(&out)
    }
}

/// Stands in when a stream's instance could not be created: every chunk fails
/// with the open error instead of panicking mid-stream.
struct BrokenStreamParser {
    envelope: ErrorEnvelope,
}

impl StreamParser for BrokenStreamParser {
    fn parse_chunk(&mut self, _: &StreamChunk) -> AdapterResult<Vec<StreamEvent>> {
        Err(self.envelope.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::invalidates_instance;
    use south_provider_runtime::CallErrorV1;

    #[test]
    fn only_traps_and_deadlines_invalidate_a_regular_instance() {
        for (error, invalidated) in [
            (CallErrorV1::Trap("synthetic".into()), true),
            (CallErrorV1::Deadline, true),
            (CallErrorV1::Component("{}".into()), false),
            (
                CallErrorV1::PayloadTooLarge {
                    limit: 16 * 1024 * 1024,
                },
                false,
            ),
            (CallErrorV1::StreamLimit, false),
        ] {
            assert_eq!(invalidates_instance(&error), invalidated);
        }
    }

    fn packaged_manifest(dir: &str) -> serde_json::Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/official")
            .join(dir)
            .join("manifest.json");
        serde_json::from_str(&std::fs::read_to_string(path).expect("manifest reads"))
            .expect("manifest parses")
    }

    fn admits(manifest: &serde_json::Value) -> Result<(), String> {
        let manifest: super::ComponentManifestV1 =
            serde_json::from_value(manifest.clone()).expect("a component manifest");
        super::HostCompatibilityV1::admit(&super::host_admission(), &manifest)
            .map_err(|error| error.to_string())
    }

    #[test]
    fn every_shipped_component_is_admitted_through_the_arm_it_declares() {
        for dir in [
            "provider-anthropic-v2",
            "provider-openai-compatible-v2",
            "provider-chatgpt-plan-v2",
            "provider-claude-plan-v2",
            "provider-copilot-plan-v2",
            "provider-codeassist-v2",
        ] {
            admits(&packaged_manifest(dir)).unwrap_or_else(|error| panic!("{dir}: {error}"));
        }
    }

    #[test]
    fn the_range_arm_refuses_runtimes_and_contracts_outside_the_host() {
        let range = packaged_manifest("provider-anthropic-v2");
        for (field, value) in [
            ("south_runtime", serde_json::json!("0.76.9")),
            ("south_runtime", serde_json::json!("0.85.2")),
            ("runtime_abi", serde_json::json!(2)),
            (
                "kernel_contracts",
                serde_json::json!({"canonical_ir": 4, "error_catalog": 1, "stream": 3}),
            ),
        ] {
            let mut outside = range.clone();
            outside["compatibility"][field] = value.clone();
            assert!(admits(&outside).is_err(), "{field} = {value} was admitted");
        }
    }

    #[test]
    fn a_package_built_before_the_range_handshake_is_refused() {
        let mut stale = packaged_manifest("provider-copilot-plan-v2");
        let compatibility = stale["compatibility"]
            .as_object_mut()
            .expect("compatibility object");
        compatibility.remove("runtime_abi");
        compatibility.remove("kernel_contracts");
        compatibility.insert("south_runtime".into(), serde_json::json!("0.20.0"));
        assert!(
            admits(&stale)
                .unwrap_err()
                .contains("declares no runtime_abi")
        );
    }
}

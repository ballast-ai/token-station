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

use std::path::Path;

use std::collections::BTreeSet;

use south_provider_api::{AuthArmV1, HostExpectationsV1};
use south_provider_runtime::{
    CallErrorV1, ComponentRuntimeV1, ComponentStreamV1, LoadedComponentV1, NoSecretsV1,
    RuntimeLimitsV1,
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

/// What this host was built against, for the tuple handshake South's loader
/// now requires.
///
/// **The one place.** Every component this host admits is judged against these
/// four values, so a second copy would be a second opinion. South owns the
/// comparison; the host owns the expectation.
///
/// These are declarations rather than derivations because nothing in the
/// dependency graph carries them: South exposes no version constant, and the
/// kernel revision names an upstream `token-station` commit the mirror tracks,
/// which no crate in this build can look up. A declaration that nobody checks
/// is exactly what let the tuple rot unnoticed for three releases, so
/// `tests/south-dependency-policy.mjs` asserts `SOUTH_RUNTIME` against the
/// pinned South version and the shipped manifests: drift turns a gate red
/// rather than turning the handshake into a formality again.
pub(crate) const IR_SCHEMA_ID: &str = "token-station-protocol@0.3.0/v0.2.0";
pub(crate) const KERNEL_VERSION: &str = "0.2.0";
pub(crate) const KERNEL_REVISION: &str = "72458e3a11fe157f9ac04818c44b62a3dd2cb09c";
pub(crate) const SOUTH_RUNTIME: &str = "0.16.0";

/// The expectation every component is admitted against.
#[must_use]
pub fn host_expectations() -> HostExpectationsV1 {
    HostExpectationsV1 {
        ir_schema_id: IR_SCHEMA_ID.to_owned(),
        kernel_version: KERNEL_VERSION.to_owned(),
        kernel_revision: KERNEL_REVISION.to_owned(),
        south_runtime: SOUTH_RUNTIME.to_owned(),
    }
}

/// A v2 component, loaded through the South gates.
pub struct SouthComponentAdapter {
    component: LoadedComponentV1,
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
            &host_expectations(),
            NoSecretsV1,
        )
        .map_err(|error| format!("south v2 component: {error}"))?;
        Ok(Self { component })
    }

    /// Loads `manifest.json` and `component.wasm` from a package directory
    /// through the same gates as [`Self::load_embedded`].
    ///
    /// # Errors
    ///
    /// The South loader's refusal, formatted with the directory.
    pub fn load_dir(runtime: &ComponentRuntimeV1, dir: &Path) -> Result<Self, String> {
        let component = LoadedComponentV1::load(runtime, dir, &host_expectations(), NoSecretsV1)
            .map_err(|error| format!("south v2 component at {}: {error}", dir.display()))?;
        Ok(Self { component })
    }

    /// The provider dialect families the loaded component's manifest declares.
    #[must_use]
    pub fn dialects(&self) -> Vec<String> {
        self.component.manifest().providers.clone()
    }

    /// The auth shapes this component's manifest declares it can carry.
    ///
    /// Transport eligibility reads this instead of matching on provider names.
    /// A name-based allowlist has to be edited for every dialect the host ever
    /// admits — and it was, which is why the Anthropic component could translate
    /// a request the transport then refused to send. The manifest already states
    /// the answer, and admission has already verified the manifest.
    #[must_use]
    pub fn auth_arms(&self) -> BTreeSet<AuthArmV1> {
        self.component.manifest().auth_arms.clone()
    }

    /// The package name its manifest reports, for diagnostics that have to name
    /// which of several components is being talked about.
    #[must_use]
    pub fn package_name(&self) -> String {
        self.component.manifest().name.clone()
    }
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
        let out = self
            .component
            .call_model_capabilities(&to_json(config)?)
            .map_err(|error| seam_error(&error))?;
        from_json(&out)
    }

    fn build_http_request(
        &self,
        request: &ChatRequest,
        config: &ProviderConfig,
    ) -> AdapterResult<HttpRequestDescriptor> {
        let out = self
            .component
            .call_build_http_request(&to_json(request)?, &to_json(config)?)
            .map_err(|error| seam_error(&error))?;
        from_json(&out)
    }

    fn parse_response(&self, parts: &HttpResponseParts) -> AdapterResult<ChatResponse> {
        let out = self
            .component
            .call_parse_response(&to_json(parts)?)
            .map_err(|error| seam_error(&error))?;
        from_json(&out)
    }

    fn map_provider_error(&self, parts: &HttpResponseParts) -> AdapterResult<ErrorEnvelope> {
        let out = self
            .component
            .call_map_provider_error(&to_json(parts)?)
            .map_err(|error| seam_error(&error))?;
        from_json(&out)
    }

    fn stream_parser(&self) -> Box<dyn StreamParser> {
        match self.component.open_stream() {
            Ok(stream) => Box::new(SouthComponentStreamParser { stream }),
            Err(error) => Box::new(BrokenStreamParser {
                envelope: seam_error(&error),
            }),
        }
    }
}

impl std::fmt::Debug for SouthComponentAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SouthComponentAdapter")
            .field("metadata", &self.component.metadata())
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

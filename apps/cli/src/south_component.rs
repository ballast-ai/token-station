//! S4: the v2 South component in this host's adapter layer.
//!
//! Two pieces. [`SouthComponentAdapter`] is the typed seam: it implements the
//! conformance crate's `ProviderAdapter` over the South runtime's JSON face,
//! serializing this host's own Canonical IR types — the runtime itself never
//! parses the IR, so the host side of the boundary lives here.
//! [`ShadowedProviderAdapter`] is the migration's first stage: every eligible
//! translation runs through both the v1 provider plugin and the v2 component,
//! byte-compares the canonical JSON of both outcomes, logs and counts any
//! divergence, and always serves the v1 result. Pure translation — nothing in
//! either piece opens a socket, so the shadow can never double-send.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde_json::Value;
use south_provider_runtime::{
    CallErrorV1, ComponentRuntimeV1, ComponentStreamV1, LoadedComponentV1, NoSecretsV1,
    RuntimeLimitsV1,
};
use token_station_conformance::{AdapterResult, ProviderAdapter, StreamParser};
use token_station_plugin_api::{AdapterKind, AdapterMetadata};
use token_station_protocol::{
    ChatRequest, ChatResponse, ErrorCode, ErrorEnvelope, HttpRequestDescriptor, HttpResponseParts,
    ModelCapability, ProviderConfig, StreamChunk, StreamEvent,
};

/// One process-wide South engine for every v2 component instance.
///
/// # Errors
///
/// Returns the engine construction error message.
pub fn south_component_runtime() -> Result<ComponentRuntimeV1, String> {
    ComponentRuntimeV1::new(RuntimeLimitsV1::default())
        .map_err(|error| format!("south component runtime: {error:#}"))
}

/// The official v2 component, loaded through the South gates.
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
        let component =
            LoadedComponentV1::load_embedded(runtime, manifest_source, wasm, NoSecretsV1)
                .map_err(|error| format!("south v2 component: {error}"))?;
        Ok(Self { component })
    }

    /// The provider dialect families the loaded component's manifest declares.
    #[must_use]
    pub fn dialects(&self) -> Vec<String> {
        self.component.manifest().providers.clone()
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
    fn metadata(&self) -> AdapterMetadata {
        let reported = self.component.metadata();
        AdapterMetadata::new(
            reported.name,
            reported.version,
            AdapterKind::Provider,
            reported.api_version,
        )
    }

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
        // The v1 seam carries UTF-8 chunk text; the v2 ABI carries bytes.
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

/// Counts what the shadow lane observed. Shared so tests and diagnostics can
/// read it; divergences are also logged as they happen.
#[derive(Debug, Default)]
pub struct ShadowReport {
    pub comparisons: AtomicU64,
    pub divergences: AtomicU64,
}

/// The migration's first stage: v1 serves, v2 shadows, every output is
/// byte-compared after canonical serialization.
pub struct ShadowedProviderAdapter {
    primary: Arc<dyn ProviderAdapter + Send + Sync>,
    shadow: Arc<SouthComponentAdapter>,
    report: Arc<ShadowReport>,
    dialect: String,
}

impl ShadowedProviderAdapter {
    #[must_use]
    pub fn new(
        primary: Arc<dyn ProviderAdapter + Send + Sync>,
        shadow: Arc<SouthComponentAdapter>,
        report: Arc<ShadowReport>,
        dialect: impl Into<String>,
    ) -> Self {
        Self {
            primary,
            shadow,
            report,
            dialect: dialect.into(),
        }
    }

    fn compare<T: serde::Serialize>(
        &self,
        operation: &'static str,
        primary: &AdapterResult<T>,
        shadow: &AdapterResult<T>,
    ) {
        compare_outcomes(&self.report, &self.dialect, operation, primary, shadow);
    }
}

fn canonical<T: serde::Serialize>(outcome: &AdapterResult<T>) -> Value {
    match outcome {
        Ok(value) => serde_json::json!({ "ok": serde_json::to_value(value).ok() }),
        Err(envelope) => serde_json::json!({ "err": serde_json::to_value(envelope).ok() }),
    }
}

fn compare_outcomes<T: serde::Serialize>(
    report: &ShadowReport,
    dialect: &str,
    operation: &'static str,
    primary: &AdapterResult<T>,
    shadow: &AdapterResult<T>,
) {
    report.comparisons.fetch_add(1, Ordering::Relaxed);
    let left = canonical(primary);
    let right = canonical(shadow);
    if left != right {
        report.divergences.fetch_add(1, Ordering::Relaxed);
        let rendered_left = truncate(&left.to_string());
        let rendered_right = truncate(&right.to_string());
        eprintln!(
            "south shadow divergence [{dialect}] {operation}: v1 {rendered_left} vs v2 \
             {rendered_right}"
        );
    }
}

fn truncate(rendered: &str) -> String {
    if rendered.chars().count() <= 400 {
        return rendered.to_owned();
    }
    let head: String = rendered.chars().take(400).collect();
    format!("{head}…")
}

impl ProviderAdapter for ShadowedProviderAdapter {
    fn metadata(&self) -> AdapterMetadata {
        // Identity stays the serving adapter's; the shadow's differing
        // api_version is expected, not a divergence.
        self.primary.metadata()
    }

    fn model_capabilities(&self, config: &ProviderConfig) -> AdapterResult<Vec<ModelCapability>> {
        let primary = self.primary.model_capabilities(config);
        let shadow = self.shadow.model_capabilities(config);
        self.compare("model_capabilities", &primary, &shadow);
        primary
    }

    fn build_http_request(
        &self,
        request: &ChatRequest,
        config: &ProviderConfig,
    ) -> AdapterResult<HttpRequestDescriptor> {
        let primary = self.primary.build_http_request(request, config);
        let shadow = self.shadow.build_http_request(request, config);
        self.compare("build_http_request", &primary, &shadow);
        primary
    }

    fn parse_response(&self, parts: &HttpResponseParts) -> AdapterResult<ChatResponse> {
        let primary = self.primary.parse_response(parts);
        let shadow = self.shadow.parse_response(parts);
        self.compare("parse_response", &primary, &shadow);
        primary
    }

    fn map_provider_error(&self, parts: &HttpResponseParts) -> AdapterResult<ErrorEnvelope> {
        let primary = self.primary.map_provider_error(parts);
        let shadow = self.shadow.map_provider_error(parts);
        self.compare("map_provider_error", &primary, &shadow);
        primary
    }

    fn stream_parser(&self) -> Box<dyn StreamParser> {
        Box::new(ShadowedStreamParser {
            primary: self.primary.stream_parser(),
            shadow: self.shadow.stream_parser(),
            report: Arc::clone(&self.report),
            dialect: self.dialect.clone(),
            shadow_poisoned: AtomicBool::new(false),
        })
    }
}

impl std::fmt::Debug for ShadowedProviderAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShadowedProviderAdapter")
            .field("dialect", &self.dialect)
            .finish_non_exhaustive()
    }
}

/// One stream, both lanes. The primary's events are served; the shadow's are
/// compared chunk-for-chunk. A shadow that errors is poisoned and skipped for
/// the rest of the stream (counted once), never affecting the serving lane.
struct ShadowedStreamParser {
    primary: Box<dyn StreamParser>,
    shadow: Box<dyn StreamParser>,
    report: Arc<ShadowReport>,
    dialect: String,
    shadow_poisoned: AtomicBool,
}

impl StreamParser for ShadowedStreamParser {
    fn parse_chunk(&mut self, chunk: &StreamChunk) -> AdapterResult<Vec<StreamEvent>> {
        let primary = self.primary.parse_chunk(chunk);
        if !self.shadow_poisoned.load(Ordering::Relaxed) {
            let shadow = self.shadow.parse_chunk(chunk);
            if shadow.is_err() && primary.is_ok() {
                self.shadow_poisoned.store(true, Ordering::Relaxed);
            }
            compare_outcomes(
                &self.report,
                &self.dialect,
                "parse_stream_chunk",
                &primary,
                &shadow,
            );
        }
        primary
    }
}

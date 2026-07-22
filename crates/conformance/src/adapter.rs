//! The seam between the conformance suite and a loaded adapter.
//!
//! The suite is written against these traits rather than against a WASM runtime,
//! for two reasons. It lets the gates exist and be proven to bite before
//! `plugin-runtime` can instantiate a component; and it keeps this crate free of
//! a wasmtime dependency, so a plugin author's own CI can run the same suite
//! against a native build of their adapter before paying for a WASM toolchain.
//!
//! Each method mirrors one function of the corresponding WIT world, with the
//! `json` payloads already parsed into the Canonical IR. What the runtime does
//! with the boundary — serialize, call, deserialize, and turn a trap into an
//! [`ErrorEnvelope`] — is exactly what makes it an implementation of these
//! traits.
//!
//! `healthcheck`, `match_inbound` and `supported_agent_protocols` are absent.
//! They carry no fixture: the architecture's ABI row is satisfied by a component
//! exporting the world at all, which is a load-time check the runtime performs,
//! not something a fixture can express.

use serde_json::Value;
use token_station_plugin_api::AdapterMetadata;
use token_station_protocol::{
    AgentHint, AgentRequestEnvelope, ChatRequest, ChatResponse, ErrorEnvelope,
    HttpRequestDescriptor, HttpResponseParts, ModelCapability, ProviderConfig, StreamChunk,
    StreamEvent,
};

/// What an adapter returns. The error is the adapter's own [`ErrorEnvelope`],
/// which is also what the runtime reports when a component traps.
pub type AdapterResult<T> = Result<T, ErrorEnvelope>;

/// Northbound: an agent tool's protocol, in and out of the Canonical IR.
pub trait AgentAdapter {
    fn metadata(&self) -> AdapterMetadata;

    /// # Errors
    ///
    /// Returns the envelope the caller should be answered with.
    fn normalize_inbound(&self, envelope: &AgentRequestEnvelope) -> AdapterResult<ChatRequest>;

    /// # Errors
    ///
    /// Returns the envelope the caller should be answered with.
    fn extract_agent_hint(&self, envelope: &AgentRequestEnvelope) -> AdapterResult<Vec<AgentHint>>;

    /// `context` is the WIT `AgentRenderContext`, which the IR does not model.
    ///
    /// # Errors
    ///
    /// Returns the envelope the caller should be answered with.
    fn render_response(&self, response: &ChatResponse, context: &Value) -> AdapterResult<Value>;

    /// # Errors
    ///
    /// Returns the envelope the caller should be answered with.
    fn render_stream_event(&self, event: &StreamEvent, context: &Value) -> AdapterResult<Value>;

    /// # Errors
    ///
    /// Returns the envelope the caller should be answered with.
    fn map_inbound_error(&self, error: &ErrorEnvelope, context: &Value) -> AdapterResult<Value>;
}

/// One provider stream, mid-parse.
///
/// Streaming is the only stateful part of the ABI. A chunk off the socket is not
/// a whole SSE frame, so an adapter must hold the tail until the rest arrives.
/// The WIT expresses that as instance state behind a plain `parse-stream-chunk`
/// function, which means the host instantiates a component per stream — so this
/// trait hands out a fresh parser per stream rather than pretending the call is
/// pure.
pub trait StreamParser {
    /// Consumes one fragment and emits whatever complete events it completed.
    ///
    /// Zero events is a normal answer: the fragment ended mid-frame.
    ///
    /// # Errors
    ///
    /// Returns the envelope the caller should be answered with.
    fn parse_chunk(&mut self, chunk: &StreamChunk) -> AdapterResult<Vec<StreamEvent>>;

    /// Flushes a clean transport EOF. The v1 WIT has no separate finish
    /// export, so the runtime represents EOF as an empty fragment, which a
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
    fn metadata(&self) -> AdapterMetadata;

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

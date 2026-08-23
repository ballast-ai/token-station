//! The seam between the conformance suite and a loaded adapter.
//!
//! The suite is written against these traits rather than against a WASM runtime,
//! for two reasons. It lets the gates exist and be proven to bite before
//! `plugin-runtime` can instantiate a component; and it keeps this crate free of
//! a wasmtime dependency, so a plugin author's own CI can run the same suite
//! against a native build of their adapter before paying for a WASM toolchain.
//!
//! Each method mirrors one function of the agent WIT world, with the `json`
//! payloads already parsed into the Canonical IR. What the runtime does with
//! the boundary — serialize, call, deserialize, and turn a trap into an
//! [`ErrorEnvelope`] — is exactly what makes it an implementation of this
//! trait.
//!
//! Southbound adapters are not part of this seam: provider translation runs
//! through South's `provider-adapter-v2` components, whose conformance is
//! South's (`south-component-conformance`), not this crate's.
//!
//! `healthcheck`, `match_inbound` and `supported_agent_protocols` are absent.
//! They carry no fixture: the architecture's ABI row is satisfied by a component
//! exporting the world at all, which is a load-time check the runtime performs,
//! not something a fixture can express.

use serde_json::Value;
use token_station_plugin_api::AdapterMetadata;
use token_station_protocol::{
    AgentHint, AgentRequestEnvelope, ChatRequest, ChatResponse, ErrorEnvelope, StreamEvent,
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

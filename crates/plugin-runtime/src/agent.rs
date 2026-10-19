use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;
use token_station_conformance::{AdapterResult, AgentAdapter, reported_identity_matches};
use token_station_plugin_api::{AdapterKind, AdapterManifest, AdapterMetadata};
use token_station_protocol::{
    AgentHint, AgentRequestEnvelope, ChatRequest, ChatResponse, ErrorCode, ErrorEnvelope,
    StreamEvent,
};
use wasmtime::Store;
use wasmtime::component::{Component, Linker};

use crate::bindings::agent::AgentAdapterV1;
use crate::bindings::agent::token_station::adapter::common as wit_common;
use crate::instance::{AdapterWorld, InstanceHandle, call as call_guest, instantiate};
use crate::loader::{
    Ctx, FORBIDDEN_FOR_AGENTS, LoadError, from_json, parse_package, read_package, to_json,
};
use crate::runtime::PluginRuntime;

/// A loaded, gated agent adapter.
///
/// The agent world imports nothing of the host's — not even the ability to
/// *name* a credential. [`AgentPlugin::load`] enforces that on the compiled
/// artifact: a component that imports `token-station:adapter/host` is refused
/// by name, whatever its manifest says. The WIT declares the boundary; this is
/// where a binary that ignored the declaration meets it.
pub struct AgentPlugin {
    runtime: PluginRuntime,
    manifest: AdapterManifest,
    component: Component,
    linker: Linker<Ctx>,
    main: Mutex<AgentState>,
}

const MAX_ACTIVE_STREAMS: usize = 1024;
const MAX_STREAM_ID_BYTES: usize = 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamEpoch {
    Current,
    Stale,
}

struct AgentState {
    instance: Option<InstanceHandle<AgentAdapterV1>>,
    streams: BTreeMap<String, StreamEpoch>,
    unidentified_stale: bool,
}

impl AgentState {
    fn admit_stream(&mut self, id: Option<&str>, terminal: bool) -> AdapterResult<()> {
        let lost = || {
            ErrorEnvelope::new(
                ErrorCode::TransportTruncated,
                502,
                "adapter stream state was lost after a resource failure",
            )
        };
        let Some(id) = id else {
            return if self.unidentified_stale {
                Err(lost())
            } else {
                Ok(())
            };
        };
        if self.streams.get(id) == Some(&StreamEpoch::Stale) {
            if terminal {
                self.streams.remove(id);
            }
            return Err(lost());
        }
        if !self.streams.contains_key(id) {
            if self.streams.len() >= MAX_ACTIVE_STREAMS {
                return Err(ErrorEnvelope::new(
                    ErrorCode::Capability,
                    400,
                    "adapter reached the 1024-stream safety limit",
                ));
            }
            self.streams.insert(id.to_owned(), StreamEpoch::Current);
        }
        Ok(())
    }

    fn invalidate(&mut self) {
        self.instance = None;
        self.streams
            .values_mut()
            .for_each(|epoch| *epoch = StreamEpoch::Stale);
        self.unidentified_stale = true;
    }
}

/// The host-facing result of [`AgentPlugin::match_inbound`]: whether the
/// adapter claims the request, and — when it speaks several — which protocol
/// it matched as. `protocol` is `None` when the adapter did not name one; the
/// gateway then falls back to the adapter's manifest protocol.
#[derive(Debug, Clone)]
pub struct MatchOutcome {
    pub matched: bool,
    pub protocol: Option<String>,
}

impl AgentPlugin {
    /// Loads `manifest.json` and `adapter.wasm` from `dir` and runs the gates.
    ///
    /// # Errors
    ///
    /// The first [`LoadError`] encountered, in gate order.
    pub fn load(runtime: &PluginRuntime, dir: &Path) -> Result<Self, LoadError> {
        let (manifest, component) =
            read_package(runtime, dir, AdapterKind::Agent, FORBIDDEN_FOR_AGENTS)?;
        Self::admit(runtime, manifest, &component)
    }

    /// [`AgentPlugin::load`] for a package compiled into the host (the builtin
    /// tier): same gates, same order, no filesystem.
    ///
    /// # Errors
    ///
    /// The first [`LoadError`] encountered, in gate order.
    pub fn load_embedded(
        runtime: &PluginRuntime,
        manifest_source: &str,
        wasm: &[u8],
    ) -> Result<Self, LoadError> {
        let (manifest, component) = parse_package(
            runtime,
            manifest_source,
            wasm,
            AdapterKind::Agent,
            FORBIDDEN_FOR_AGENTS,
        )?;
        Self::admit(runtime, manifest, &component)
    }

    /// Gate 3 onward, shared by both load paths.
    fn admit(
        runtime: &PluginRuntime,
        manifest: AdapterManifest,
        component: &Component,
    ) -> Result<Self, LoadError> {
        // WASI only. The `host` interface is deliberately not linked: even a
        // component that slipped past the import scan would fail to
        // instantiate, because nothing offers it.
        let mut linker: Linker<Ctx> = Linker::new(runtime.engine());
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(LoadError::NotAnAdapter)?;

        let handle = instantiate_identity(runtime, &manifest, component, &linker)?;

        Ok(Self {
            runtime: runtime.clone(),
            manifest,
            component: component.clone(),
            linker,
            main: Mutex::new(AgentState {
                instance: Some(handle),
                streams: BTreeMap::new(),
                unidentified_stale: false,
            }),
        })
    }

    /// The manifest this plugin was admitted under.
    #[must_use]
    pub fn manifest(&self) -> &AdapterManifest {
        &self.manifest
    }

    /// `match_inbound`: does this adapter claim the given request? The host
    /// asks every loaded agent this — with a redacted `request-head`
    /// (`{ method, path, headers }`, headers already a `HeaderDigest`) — and
    /// dispatches to the first that says yes. It is *not* in the
    /// [`AgentAdapter`] trait: the conformance harness drills translation, not
    /// the host's multiplexing, so this stays an inherent method over the
    /// generated binding.
    ///
    /// # Errors
    ///
    /// A trap in the guest. The gateway treats an erroring adapter as "did not
    /// match" so one broken adapter cannot veto the others.
    ///
    /// # Panics
    ///
    /// If the adapter's instance lock is poisoned — a previous call panicked
    /// mid-invocation, so the instance state is unknown and every later call
    /// stays poisoned rather than running on it.
    pub fn match_inbound(&self, request_head: &Value) -> AdapterResult<MatchOutcome> {
        let head_json = to_json(request_head)?;
        let result = self.call(|handle| {
            handle
                .instance
                .token_station_adapter_agent_adapter()
                .call_match_inbound(&mut handle.store, &head_json)
                .map(Ok)
        })?;
        Ok(MatchOutcome {
            matched: result.matched,
            protocol: result.protocol,
        })
    }

    fn call<T>(
        &self,
        operation: impl FnOnce(
            &mut InstanceHandle<AgentAdapterV1>,
        ) -> wasmtime::Result<Result<T, String>>,
    ) -> AdapterResult<T> {
        let mut state = self.main.lock().expect("a poisoned adapter stays poisoned");
        self.call_locked(&mut state, operation)
    }

    fn call_locked<T>(
        &self,
        state: &mut AgentState,
        operation: impl FnOnce(
            &mut InstanceHandle<AgentAdapterV1>,
        ) -> wasmtime::Result<Result<T, String>>,
    ) -> AdapterResult<T> {
        if state.instance.is_none() {
            let instance =
                instantiate_identity(&self.runtime, &self.manifest, &self.component, &self.linker)
                    .map_err(|_| {
                        ErrorEnvelope::new(
                            ErrorCode::Internal,
                            500,
                            "adapter instance recovery failed",
                        )
                    })?;
            state.instance = Some(instance);
        }
        let handle = state.instance.as_mut().expect("instance initialized above");
        let result = call_guest(&self.runtime, handle, operation);
        if handle.trapped {
            state.invalidate();
        }
        result
    }
}

fn instantiate_identity(
    runtime: &PluginRuntime,
    manifest: &AdapterManifest,
    component: &Component,
    linker: &Linker<Ctx>,
) -> Result<InstanceHandle<AgentAdapterV1>, LoadError> {
    let mut handle: InstanceHandle<AgentAdapterV1> =
        instantiate(runtime, component, linker, agent_ctx(runtime)).map_err(LoadError::Probe)?;
    let reported = handle.call_metadata(runtime).map_err(LoadError::Probe)?;
    if !reported_identity_matches(&reported, manifest) {
        return Err(LoadError::IdentityMismatch {
            declared: Box::new(manifest.metadata()),
            reported: Box::new(reported),
        });
    }
    Ok(handle)
}

/// The sandbox context an agent-world instance runs under.
fn agent_ctx(runtime: &PluginRuntime) -> Ctx {
    Ctx::new(runtime.limits().memory_bytes)
}

impl AdapterWorld for AgentAdapterV1 {
    fn instantiate_world(
        store: &mut Store<Ctx>,
        component: &Component,
        linker: &Linker<Ctx>,
    ) -> wasmtime::Result<Self> {
        Self::instantiate(store, component, linker)
    }

    fn call_metadata(&self, store: &mut Store<Ctx>) -> wasmtime::Result<AdapterMetadata> {
        let reported = self
            .token_station_adapter_agent_adapter()
            .call_metadata(store)?;
        Ok(convert_metadata(reported))
    }
}

/// Decodes an agent adapter's normalized request and clears `host_values`.
///
/// Only the host mints host values. An agent adapter sees client input, so any
/// host value it returns is client-supplied and must not reach a provider. A
/// map that breaks the `ComponentValues` grammar fails the decode instead.
fn admit_normalized_request(out: &str) -> AdapterResult<ChatRequest> {
    let mut request: ChatRequest = from_json(out)?;
    request.host_values.clear();
    Ok(request)
}

fn convert_metadata(wit: wit_common::AdapterMetadata) -> AdapterMetadata {
    AdapterMetadata::new(
        wit.name,
        wit.version,
        match wit.kind {
            wit_common::AdapterKind::Agent => AdapterKind::Agent,
        },
        wit.api_version,
    )
}

impl AgentAdapter for AgentPlugin {
    fn metadata(&self) -> AdapterMetadata {
        self.manifest.metadata()
    }

    fn normalize_inbound(&self, envelope: &AgentRequestEnvelope) -> AdapterResult<ChatRequest> {
        let envelope_json = to_json(envelope)?;
        let out = self.call(|handle| {
            handle
                .instance
                .token_station_adapter_agent_adapter()
                .call_normalize_inbound(&mut handle.store, &envelope_json)
        })?;
        admit_normalized_request(&out)
    }

    fn extract_agent_hint(&self, envelope: &AgentRequestEnvelope) -> AdapterResult<Vec<AgentHint>> {
        let envelope_json = to_json(envelope)?;
        let out = self.call(|handle| {
            handle
                .instance
                .token_station_adapter_agent_adapter()
                .call_extract_agent_hint(&mut handle.store, &envelope_json)
        })?;
        from_json(&out)
    }

    fn render_response(&self, response: &ChatResponse, context: &Value) -> AdapterResult<Value> {
        let response_json = to_json(response)?;
        let context_json = to_json(context)?;
        let out = self.call(|handle| {
            handle
                .instance
                .token_station_adapter_agent_adapter()
                .call_render_response(&mut handle.store, &response_json, &context_json)
        })?;
        from_json(&out)
    }

    fn render_stream_event(&self, event: &StreamEvent, context: &Value) -> AdapterResult<Value> {
        let id = context
            .get("stream_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        if id.is_some_and(|id| id.len() > MAX_STREAM_ID_BYTES) {
            return Err(ErrorEnvelope::new(
                ErrorCode::InvalidRequest,
                400,
                "adapter stream identifier exceeded the host limit",
            ));
        }
        let terminal = matches!(event, StreamEvent::Done { .. } | StreamEvent::Error { .. });
        let event_json = to_json(event)?;
        let context_json = to_json(context)?;
        let mut state = self.main.lock().expect("a poisoned adapter stays poisoned");
        state.admit_stream(id, terminal)?;
        let result = self.call_locked(&mut state, |handle| {
            handle
                .instance
                .token_station_adapter_agent_adapter()
                .call_render_stream_event(&mut handle.store, &event_json, &context_json)
        });
        let output = result.and_then(|out| from_json(&out));
        if terminal
            && output.is_ok()
            && let Some(id) = id
        {
            state.streams.remove(id);
        }
        output
    }

    fn map_inbound_error(&self, error: &ErrorEnvelope, context: &Value) -> AdapterResult<Value> {
        let error_json = to_json(error)?;
        let context_json = to_json(context)?;
        let out = self.call(|handle| {
            handle
                .instance
                .token_station_adapter_agent_adapter()
                .call_map_inbound_error(&mut handle.store, &error_json, &context_json)
        })?;
        from_json(&out)
    }
}

impl fmt::Debug for AgentPlugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentPlugin")
            .field("manifest", &self.manifest.metadata())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::admit_normalized_request;

    #[test]
    fn an_agent_adapter_cannot_supply_host_values() {
        let request = admit_normalized_request(
            r#"{"model":"auto","messages":[],"host_values":{"attempt_id":"forged"},"seed":7}"#,
        )
        .expect("a valid normalized request");

        assert!(request.host_values.is_empty());
        assert_eq!(request.extensions["seed"], serde_json::json!(7));
    }

    #[test]
    fn an_agent_adapter_host_value_outside_the_grammar_fails_the_request() {
        let forged =
            r#"{"model":"auto","messages":[],"host_values":{"attempt_id":"a\r\nx-injected: 1"}}"#;

        assert!(admit_normalized_request(forged).is_err());
    }
}

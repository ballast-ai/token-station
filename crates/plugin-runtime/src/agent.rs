use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use token_station_conformance::{AdapterResult, AgentAdapter, reported_identity_matches};
use token_station_plugin_api::{AdapterKind, AdapterManifest, AdapterMetadata};
use token_station_protocol::{
    AgentHint, AgentRequestEnvelope, ChatRequest, ChatResponse, ErrorEnvelope,
};
use wasmtime::Store;
use wasmtime::component::{Component, Linker};

use crate::bindings::agent::AgentAdapterV1;
use crate::bindings::agent::token_station::adapter::common as wit_common;
use crate::loader::{
    Ctx, FORBIDDEN_FOR_AGENTS, LoadError, from_json, parse_error_envelope, parse_package,
    read_package, to_json, trap_envelope,
};
use crate::provider::NoSecrets;
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
    main: Mutex<InstanceHandle>,
}

struct InstanceHandle {
    store: Store<Ctx>,
    instance: AgentAdapterV1,
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

        let mut handle = instantiate(runtime, component, &linker).map_err(LoadError::Probe)?;
        let reported = handle.call_metadata(runtime).map_err(LoadError::Probe)?;
        if !reported_identity_matches(&reported, &manifest) {
            return Err(LoadError::IdentityMismatch {
                declared: Box::new(manifest.metadata()),
                reported: Box::new(reported),
            });
        }

        Ok(Self {
            runtime: runtime.clone(),
            manifest,
            main: Mutex::new(handle),
        })
    }

    /// The manifest this plugin was admitted under.
    #[must_use]
    pub fn manifest(&self) -> &AdapterManifest {
        &self.manifest
    }

    fn call<T>(
        &self,
        operation: impl FnOnce(&mut InstanceHandle) -> wasmtime::Result<Result<T, String>>,
    ) -> AdapterResult<T> {
        let mut handle = self.main.lock().expect("a poisoned adapter stays poisoned");
        handle
            .store
            .set_epoch_deadline(self.runtime.deadline_ticks());

        match operation(&mut handle) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error_json)) => Err(parse_error_envelope(&error_json)),
            Err(trap) => Err(trap_envelope(&trap)),
        }
    }
}

fn instantiate(
    runtime: &PluginRuntime,
    component: &Component,
    linker: &Linker<Ctx>,
) -> wasmtime::Result<InstanceHandle> {
    // An agent adapter has no secrets to declare and no signer to reach; the
    // credential fields of `Ctx` are inert because nothing links `host`.
    let ctx = Ctx::new(
        runtime.limits().memory_bytes,
        std::collections::BTreeSet::new(),
        Arc::new(NoSecrets),
    );
    let mut store = Store::new(runtime.engine(), ctx);
    store.limiter(|ctx| &mut ctx.limits);
    store.set_epoch_deadline(runtime.deadline_ticks());

    let instance = AgentAdapterV1::instantiate(&mut store, component, linker)?;
    Ok(InstanceHandle { store, instance })
}

impl InstanceHandle {
    fn call_metadata(&mut self, runtime: &PluginRuntime) -> wasmtime::Result<AdapterMetadata> {
        self.store.set_epoch_deadline(runtime.deadline_ticks());
        let reported = self
            .instance
            .token_station_adapter_agent_adapter()
            .call_metadata(&mut self.store)?;
        Ok(convert_metadata(reported))
    }
}

fn convert_metadata(wit: wit_common::AdapterMetadata) -> AdapterMetadata {
    AdapterMetadata::new(
        wit.name,
        wit.version,
        match wit.kind {
            wit_common::AdapterKind::Agent => AdapterKind::Agent,
            wit_common::AdapterKind::Provider => AdapterKind::Provider,
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
        from_json(&out)
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

    fn render_stream_event(
        &self,
        event: &token_station_protocol::StreamEvent,
        context: &Value,
    ) -> AdapterResult<Value> {
        let event_json = to_json(event)?;
        let context_json = to_json(context)?;
        let out = self.call(|handle| {
            handle
                .instance
                .token_station_adapter_agent_adapter()
                .call_render_stream_event(&mut handle.store, &event_json, &context_json)
        })?;
        from_json(&out)
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

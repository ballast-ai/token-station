use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use token_station_conformance::{
    AdapterResult, ProviderAdapter, StreamParser, reported_identity_matches,
};
use token_station_plugin_api::{AdapterKind, AdapterManifest, AdapterMetadata};
use token_station_protocol::{
    ChatRequest, ChatResponse, ErrorEnvelope, HttpRequestDescriptor, HttpResponseParts,
    ModelCapability, ProviderConfig, StreamChunk, StreamEvent,
};
use wasmtime::Store;
use wasmtime::component::{Component, Linker};

use crate::bindings::provider::ProviderAdapterV1;
use crate::bindings::provider::token_station::adapter::common as wit_common;
use crate::bindings::provider::token_station::adapter::host as wit_host;
use crate::loader::{
    Ctx, LoadError, from_json, parse_error_envelope, parse_package, read_package, to_json,
    trap_envelope,
};
use crate::runtime::PluginRuntime;

/// Resolves a *declared* credential name into a signature.
///
/// The runtime has already checked the name against the manifest before this is
/// called, so an implementation never learns that an undeclared name was asked
/// for. What it must still decide is whether the named credential exists and
/// whether the algorithm is supported.
pub trait SecretSigner: Send + 'static {
    /// # Errors
    ///
    /// A message safe to hand back to the guest: it must not contain key
    /// material, because the guest will see it verbatim.
    fn sign(&self, secret_ref: &str, payload: &[u8], algorithm: &str) -> Result<Vec<u8>, String>;
}

/// A signer for hosts with no signing credentials configured. Refuses politely.
pub struct NoSecrets;

impl SecretSigner for NoSecrets {
    fn sign(&self, _: &str, _: &[u8], _: &str) -> Result<Vec<u8>, String> {
        Err("this host has no signing credentials configured".to_owned())
    }
}

impl wit_host::Host for Ctx {
    fn sign(
        &mut self,
        secret_ref: String,
        payload: Vec<u8>,
        algorithm: String,
    ) -> Result<Vec<u8>, String> {
        // The manifest boundary, enforced before any signer sees the request.
        if !self.declared_secrets.contains(&secret_ref) {
            return Err(format!(
                "secret `{secret_ref}` is not declared in this adapter's manifest"
            ));
        }
        self.signer.sign(&secret_ref, &payload, &algorithm)
    }
}

/// A loaded, gated provider adapter.
///
/// Implements `token-station-conformance`'s `ProviderAdapter`, which is what
/// lets the same conformance suite that judged native reference adapters judge
/// a `.wasm` one — the official plugins pass through exactly this type on
/// their way into the registry.
pub struct ProviderPlugin {
    runtime: PluginRuntime,
    component: Component,
    linker: Arc<Linker<Ctx>>,
    manifest: AdapterManifest,
    signer: Arc<dyn SecretSigner + Sync>,
    /// The instance regular calls go through. Streams get their own; see
    /// [`ProviderPlugin::stream_parser`].
    main: Mutex<InstanceHandle>,
}

/// One instantiated component and its store.
struct InstanceHandle {
    store: Store<Ctx>,
    instance: ProviderAdapterV1,
}

impl ProviderPlugin {
    /// Loads `manifest.json` and `adapter.wasm` from `dir` and runs the gates.
    ///
    /// # Errors
    ///
    /// The first [`LoadError`] encountered, in gate order.
    pub fn load(
        runtime: &PluginRuntime,
        dir: &Path,
        signer: impl SecretSigner + Sync,
    ) -> Result<Self, LoadError> {
        let (manifest, component) = read_package(runtime, dir, AdapterKind::Provider, &[])?;
        Self::admit(runtime, manifest, component, Arc::new(signer))
    }

    /// [`ProviderPlugin::load`] for a package compiled into the host (the
    /// builtin tier): same gates, same order, no filesystem.
    ///
    /// # Errors
    ///
    /// The first [`LoadError`] encountered, in gate order.
    pub fn load_embedded(
        runtime: &PluginRuntime,
        manifest_source: &str,
        wasm: &[u8],
        signer: impl SecretSigner + Sync,
    ) -> Result<Self, LoadError> {
        let (manifest, component) =
            parse_package(runtime, manifest_source, wasm, AdapterKind::Provider, &[])?;
        Self::admit(runtime, manifest, component, Arc::new(signer))
    }

    /// Gate 3 onward, shared by both load paths: link, instantiate, and check
    /// the reported identity against the manifest.
    fn admit(
        runtime: &PluginRuntime,
        manifest: AdapterManifest,
        component: Component,
        signer: Arc<dyn SecretSigner + Sync>,
    ) -> Result<Self, LoadError> {
        let mut linker: Linker<Ctx> = Linker::new(runtime.engine());
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(LoadError::NotAnAdapter)?;
        wit_host::add_to_linker::<Ctx, wasmtime::component::HasSelf<Ctx>>(&mut linker, |ctx| ctx)
            .map_err(LoadError::NotAnAdapter)?;
        let linker = Arc::new(linker);

        // Gate 3: instantiate and check identity.
        let mut handle = instantiate(runtime, &component, &linker, &manifest, &signer)
            .map_err(LoadError::Probe)?;
        let reported = handle.call_metadata(runtime).map_err(LoadError::Probe)?;
        if !reported_identity_matches(&reported, &manifest) {
            return Err(LoadError::IdentityMismatch {
                declared: Box::new(manifest.metadata()),
                reported: Box::new(reported),
            });
        }

        Ok(Self {
            runtime: runtime.clone(),
            component,
            linker,
            manifest,
            signer,
            main: Mutex::new(handle),
        })
    }

    /// The manifest this plugin was admitted under.
    #[must_use]
    pub fn manifest(&self) -> &AdapterManifest {
        &self.manifest
    }

    /// Runs one guest call with a fresh deadline, mapping a trap to the
    /// adapter-shaped error the trait promises.
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
            // The adapter answered with its own ErrorEnvelope, as JSON.
            Ok(Err(error_json)) => Err(parse_error_envelope(&error_json)),
            // The adapter trapped: deadline, memory, panic. All the same to
            // the caller — the adapter did not answer.
            Err(trap) => Err(trap_envelope(&trap)),
        }
    }
}

/// Builds one instance with its own locked-down store.
fn instantiate(
    runtime: &PluginRuntime,
    component: &Component,
    linker: &Linker<Ctx>,
    manifest: &AdapterManifest,
    signer: &Arc<dyn SecretSigner + Sync>,
) -> wasmtime::Result<InstanceHandle> {
    let ctx = Ctx::new(
        runtime.limits().memory_bytes,
        manifest.permissions.secrets.iter().cloned().collect(),
        Arc::clone(signer),
    );
    let mut store = Store::new(runtime.engine(), ctx);
    store.limiter(|ctx| &mut ctx.limits);
    store.set_epoch_deadline(runtime.deadline_ticks());

    let instance = ProviderAdapterV1::instantiate(&mut store, component, linker)?;
    Ok(InstanceHandle { store, instance })
}

impl InstanceHandle {
    fn call_metadata(&mut self, runtime: &PluginRuntime) -> wasmtime::Result<AdapterMetadata> {
        self.store.set_epoch_deadline(runtime.deadline_ticks());
        let reported = self
            .instance
            .token_station_adapter_provider_adapter()
            .call_metadata(&mut self.store)?;
        Ok(convert_metadata(reported))
    }
}

pub(crate) fn convert_metadata(wit: wit_common::AdapterMetadata) -> AdapterMetadata {
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

impl ProviderAdapter for ProviderPlugin {
    fn metadata(&self) -> AdapterMetadata {
        // Identity was gated at load; report the manifest's, which is equal.
        self.manifest.metadata()
    }

    fn model_capabilities(&self, config: &ProviderConfig) -> AdapterResult<Vec<ModelCapability>> {
        let config_json = to_json(config)?;
        let out = self.call(|handle| {
            handle
                .instance
                .token_station_adapter_provider_adapter()
                .call_model_capabilities(&mut handle.store, &config_json)
        })?;
        from_json(&out)
    }

    fn build_http_request(
        &self,
        request: &ChatRequest,
        config: &ProviderConfig,
    ) -> AdapterResult<HttpRequestDescriptor> {
        let request_json = to_json(request)?;
        let config_json = to_json(config)?;
        let out = self.call(|handle| {
            handle
                .instance
                .token_station_adapter_provider_adapter()
                .call_build_http_request(&mut handle.store, &request_json, &config_json)
        })?;
        from_json(&out)
    }

    fn parse_response(&self, parts: &HttpResponseParts) -> AdapterResult<ChatResponse> {
        let parts_json = to_json(parts)?;
        let out = self.call(|handle| {
            handle
                .instance
                .token_station_adapter_provider_adapter()
                .call_parse_response(&mut handle.store, &parts_json)
        })?;
        from_json(&out)
    }

    fn map_provider_error(&self, parts: &HttpResponseParts) -> AdapterResult<ErrorEnvelope> {
        let parts_json = to_json(parts)?;
        let out = self.call(|handle| {
            handle
                .instance
                .token_station_adapter_provider_adapter()
                .call_map_provider_error(&mut handle.store, &parts_json)
        })?;
        from_json(&out)
    }

    fn stream_parser(&self) -> Box<dyn StreamParser> {
        // One instance per stream: `parse-stream-chunk` holds the unparsed
        // tail as instance state, so sharing an instance across streams would
        // interleave two providers' bodies.
        match instantiate(
            &self.runtime,
            &self.component,
            &self.linker,
            &self.manifest,
            &self.signer,
        ) {
            Ok(handle) => Box::new(WasmStreamParser {
                runtime: self.runtime.clone(),
                handle,
            }),
            Err(error) => Box::new(BrokenStreamParser {
                envelope: trap_envelope(&error),
            }),
        }
    }
}

impl fmt::Debug for ProviderPlugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderPlugin")
            .field("manifest", &self.manifest.metadata())
            .finish_non_exhaustive()
    }
}

/// One provider stream, backed by its own component instance.
struct WasmStreamParser {
    runtime: PluginRuntime,
    handle: InstanceHandle,
}

impl StreamParser for WasmStreamParser {
    fn parse_chunk(&mut self, chunk: &StreamChunk) -> AdapterResult<Vec<StreamEvent>> {
        let chunk_json = to_json(chunk)?;
        self.handle
            .store
            .set_epoch_deadline(self.runtime.deadline_ticks());

        match self
            .handle
            .instance
            .token_station_adapter_provider_adapter()
            .call_parse_stream_chunk(&mut self.handle.store, &chunk_json)
        {
            Ok(Ok(events_json)) => from_json(&events_json),
            Ok(Err(error_json)) => Err(parse_error_envelope(&error_json)),
            Err(trap) => Err(trap_envelope(&trap)),
        }
    }
}

/// Stands in when a stream's instance could not be created: every chunk fails
/// with the instantiation error instead of panicking mid-stream.
struct BrokenStreamParser {
    envelope: ErrorEnvelope,
}

impl StreamParser for BrokenStreamParser {
    fn parse_chunk(&mut self, _: &StreamChunk) -> AdapterResult<Vec<StreamEvent>> {
        Err(self.envelope.clone())
    }
}

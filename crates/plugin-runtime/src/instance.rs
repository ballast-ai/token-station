//! The instantiate/call/metadata skeleton both adapter worlds run, generic
//! over which world.
//!
//! `provider.rs` and `agent.rs` each `bindgen!` a distinct world (see
//! `bindings.rs`), so `ProviderAdapterV1` and `AgentAdapterV1` are unrelated
//! generated types with no shared trait of their own. [`AdapterWorld`] is that
//! trait, written by hand: one instantiate call and one metadata call per
//! world, everything else here runs once instead of twice.
//!
//! What stays out on purpose: `admit()`'s linker wiring (the provider world
//! links `host.sign`, the agent world deliberately does not — see
//! `agent.rs`'s doc comment) and every adapter-specific method (`build-http-
//! request`, `normalize-inbound`, ...). Those differ by more than a type
//! parameter and forcing them through this trait would hide the differences
//! that matter, not the ones that don't.

use std::sync::Mutex;

use token_station_conformance::AdapterResult;
use token_station_plugin_api::AdapterMetadata;
use wasmtime::Store;
use wasmtime::component::{Component, Linker};

use crate::loader::{Ctx, parse_error_envelope, trap_envelope};
use crate::runtime::PluginRuntime;

/// A `bindgen!`-generated adapter world: how to instantiate it and how to ask
/// it who it is.
pub(crate) trait AdapterWorld: Sized {
    /// Builds one instance of this world inside `store`.
    ///
    /// Named `instantiate_world`, not `instantiate`: the `bindgen!`-generated
    /// type this is implemented on already has an inherent `instantiate` with
    /// the same shape, and this delegates to it. A shared name would make
    /// every call inside the `impl` ambiguous between "call the trait method
    /// again" and "call the generated one".
    fn instantiate_world(
        store: &mut Store<Ctx>,
        component: &Component,
        linker: &Linker<Ctx>,
    ) -> wasmtime::Result<Self>;

    /// Calls the world's `metadata()` export and converts the result to the
    /// host's [`AdapterMetadata`].
    fn call_metadata(&self, store: &mut Store<Ctx>) -> wasmtime::Result<AdapterMetadata>;
}

/// One instantiated component and its store, generic over which world it
/// instantiates.
pub(crate) struct InstanceHandle<W> {
    pub(crate) store: Store<Ctx>,
    pub(crate) instance: W,
}

/// Builds one instance with its own locked-down store.
///
/// `ctx` is the caller's: the provider world seeds it with the manifest's
/// declared secrets and a real signer, the agent world seeds it with none —
/// that choice stays in `provider.rs` / `agent.rs`, this only wires the store
/// limits and deadline every world needs identically.
pub(crate) fn instantiate<W: AdapterWorld>(
    runtime: &PluginRuntime,
    component: &Component,
    linker: &Linker<Ctx>,
    ctx: Ctx,
) -> wasmtime::Result<InstanceHandle<W>> {
    let mut store = Store::new(runtime.engine(), ctx);
    store.limiter(|ctx| &mut ctx.limits);
    store.set_epoch_deadline(runtime.deadline_ticks());

    let instance = W::instantiate_world(&mut store, component, linker)?;
    Ok(InstanceHandle { store, instance })
}

impl<W: AdapterWorld> InstanceHandle<W> {
    pub(crate) fn call_metadata(
        &mut self,
        runtime: &PluginRuntime,
    ) -> wasmtime::Result<AdapterMetadata> {
        self.store.set_epoch_deadline(runtime.deadline_ticks());
        self.instance.call_metadata(&mut self.store)
    }
}

/// Runs one guest call with a fresh deadline, mapping a trap to the
/// adapter-shaped error the trait promises.
///
/// Shared by [`crate::ProviderPlugin`] and [`crate::AgentPlugin`], both of
/// which hold their live instance behind a `Mutex<InstanceHandle<W>>` — the
/// lock, not a type parameter on `Self`, is what this needs.
pub(crate) fn call<W, T>(
    runtime: &PluginRuntime,
    main: &Mutex<InstanceHandle<W>>,
    operation: impl FnOnce(&mut InstanceHandle<W>) -> wasmtime::Result<Result<T, String>>,
) -> AdapterResult<T> {
    let mut handle = main.lock().expect("a poisoned adapter stays poisoned");
    handle.store.set_epoch_deadline(runtime.deadline_ticks());

    match operation(&mut handle) {
        Ok(Ok(value)) => Ok(value),
        // The adapter answered with its own ErrorEnvelope, as JSON.
        Ok(Err(error_json)) => Err(parse_error_envelope(&error_json)),
        // The adapter trapped: deadline, memory, panic. All the same to the
        // caller — the adapter did not answer.
        Err(trap) => Err(trap_envelope(&trap)),
    }
}

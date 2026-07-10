//! Host-side bindings for both adapter worlds.
//!
//! Generated from the same `wit/adapter.wit` that `token-station-plugin-api`
//! embeds and tests, so the worlds the runtime instantiates are by construction
//! the worlds the manifest schema names. Guests are compiled against that file;
//! this is the other half of the contract.
//!
//! Two modules because `bindgen!` generates one namespace per world, and the
//! worlds share interface names (`common`) that would otherwise collide.

#![allow(
    clippy::pedantic,
    clippy::all,
    reason = "generated code is held to wasmtime's style, not ours"
)]

pub mod provider {
    wasmtime::component::bindgen!({
        path: "../plugin-api/wit",
        world: "provider-adapter-v1",
    });
}

pub mod agent {
    wasmtime::component::bindgen!({
        path: "../plugin-api/wit",
        world: "agent-adapter-v1",
    });
}

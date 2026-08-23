//! WASM host runtime: loads northbound agent adapters and holds them to the
//! sandbox the architecture promises.
//!
//! What this crate is, in one sentence: the thing that makes
//! `token-station-conformance`'s trait seam real for a `.wasm` file. The suite
//! and the request path are written against `AgentAdapter`; this crate
//! implements that trait over a WASM component, which is exactly the
//! "serialize, call, deserialize, turn a trap into an `ErrorEnvelope`" job the
//! seam's documentation assigns to it.
//!
//! Southbound provider components do not load here. They are South's
//! `provider-adapter-v2` components, loaded through `south-provider-runtime`
//! with its own gates; the host wires the two runtimes side by side.
//!
//! # The load path is the trust path
//!
//! [`AgentPlugin::load`] runs the gates in order, cheapest and most decidable
//! first:
//!
//! 1. **Manifest gate** — `accepts_manifest`, before any code is read.
//! 2. **Import scan** — the compiled component must not import `wasi:sockets`
//!    or `wasi:http`. Everything else WASI gets a locked-down implementation
//!    (no preopened directories, no environment, no inherited stdio), but the
//!    network is refused at the door rather than emptied: a plugin that even
//!    *asks* for sockets is not a protocol adapter.
//! 3. **Identity gate** — the instantiated adapter's `metadata()` must equal
//!    the manifest's declaration (`reported_identity_matches`).
//!
//! # Resource bounds
//!
//! Every store carries a memory limit ([`RuntimeLimits::memory_bytes`]) and
//! every guest call a deadline ([`RuntimeLimits::call_timeout`], enforced by
//! epoch interruption — a background ticker thread advances the engine epoch
//! and a hung guest traps at its deadline). A trap of either kind surfaces as
//! an `ErrorEnvelope` with code `internal`, because to the caller an adapter
//! that ran out of memory and an adapter that divided by zero are the same
//! thing: an adapter that did not answer.
//!
//! # Credentials
//!
//! The agent world imports nothing of the host's, so no instance built here can
//! so much as name a credential. The import scan enforces that on the compiled
//! artifact: a component importing `token-station:adapter/host` — the credential
//! interface the retired southbound world used to carry — is refused by name.

mod agent;
mod bindings;
mod instance;
mod loader;
mod runtime;

pub use agent::{AgentPlugin, MatchOutcome};
pub use loader::{LoadError, UnreadableReason};
pub use runtime::{PluginRuntime, RuntimeLimits};

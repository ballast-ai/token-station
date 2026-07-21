//! The routing kernel, shared by the local client and the server gateway.
//!
//! One crate rather than two, because a routing decision the two lines disagree
//! about is worse than either answer alone: the same request, the same config,
//! and a different model depending on which binary served it. Everything here is
//! arranged to make that disagreement impossible to introduce quietly.
//!
//! # Four layers, first to answer wins
//!
//! 1. **Rules** the operator wrote. Highest priority, always — this is the
//!    override the user is promised when the router guesses wrong.
//! 2. **Agent hints**: the calling tool knows it is planning rather than
//!    summarising, and the host cannot infer that.
//! 3. **The heuristic**: score the request, compare to a threshold. Stands in
//!    for the learned classifier until `C3#2` ships it.
//! 4. **The default pool.**
//!
//! The cascade layer — answer cheaply, then judge whether to escalate — is
//! deliberately absent. It doubles latency and cannot be done on a streaming
//! first response, and the local client is a streaming proxy.
//!
//! # Three properties, each load-bearing
//!
//! **[`Router::route`] is pure.** No clock, no randomness, no IO. The audit
//! command replays a logged decision and gets the same answer; the two lines,
//! given the same inputs, cannot diverge. Health is an *input*, supplied by the
//! host, precisely because deciding an upstream is sick needs a clock.
//!
//! **The score is integer arithmetic.** A float would let the client and the
//! gateway disagree in the last bit, and route the same request to different
//! models.
//!
//! **A [`Decision`] cannot carry content.** The router reads the prompt — a
//! keyword rule scans it, the heuristic counts code fences — and remembers only
//! [`RequestFeatures`], which derives `Copy` and therefore cannot own a `String`.
//! Every string in a `Decision` originates in the [`RouterConfig`]: a rule id,
//! a pool name, an upstream reference name, a hint key from the routing table.
//! None comes from the request.
//!
//! That last one is the client's central promise — request content never leaves
//! the device — expressed as a type rather than as a code-review convention. The
//! decision record is what the metrics store persists and what the cloud-sync
//! whitelist is later drawn from, so it is the one place the promise can break.
//!
//! # Configuration
//!
//! [`RouterConfig`] is a document; [`Router`] is a validated one. Only a
//! `Router` can route, which is why [`NoRoute`] has no "unknown pool" arm.
//! [`ConfigCache`] keeps the last configuration that validated, so an
//! unreachable config service or a malformed remote profile cannot stop the
//! request path.

mod config;
mod decision;
mod features;
mod health;
mod lexicon;
mod route;
mod source;

#[cfg(test)]
mod test_support;

pub use config::{
    Band, CONFIG_VERSION, ConfigError, Heuristic, HintRoute, Match, RecoveryPolicy, RouterConfig,
    Rule, Weights,
};
pub use decision::{
    DecidedBy, Decision, InvalidUpstreamRef, NoRoute, UnmetRequirement, UpstreamModel, UpstreamRef,
};
pub use features::{RequestFeatures, estimate_tokens};
pub use health::{HealthPolicy, HealthTracker};
pub use route::{Candidate, Health, Router};
pub use source::{CacheError, ConfigCache, ConfigSource, StaticConfigSource};

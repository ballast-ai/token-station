//! The community client: a loopback OpenAI-compatible proxy over the shared
//! routing kernel and the WASM adapter plugins.
//!
//! Layering, from the socket inward:
//!
//! - [`server`] — the async facade. axum frames HTTP; nothing below it awaits.
//! - [`gateway`] — the synchronous data plane: agent plugin → router →
//!   provider plugin → upstream, with the exfiltration gate
//!   (`ProviderConfig::authorize`) between "the plugin built a request" and
//!   "the host resolved a credential".
//! - [`config`] / [`secrets`] — one JSON file; credential *locations*, never
//!   values.
//! - [`manage`] / [`stats`] — the CLI management surface: pure edits of the
//!   config file, and read-only aggregation of the metrics store.
//!
//! A library crate so the integration tests can stand the whole server up
//! in-process against a mock upstream; `main.rs` is a thin argument parser.

pub mod admin;
pub mod admission;
pub mod backup;
pub mod budget;
pub mod cancel;
pub mod config;
pub mod filelog;
pub mod gateway;
pub mod manage;
pub mod plugins;
pub mod pricing;
mod private_fs;
pub mod quota_headers;
pub mod quota_ledger;
pub mod quota_lease;
pub mod quota_tracker;
pub mod request_context;
pub mod scaffold;
pub mod secrets;
pub mod server;
mod sse;
pub mod stats;
pub mod store;
pub mod upgrade;
pub mod virtual_key;

/// The example configuration shipped with the binary, kept loadable by tests.
pub const EXAMPLE_CONFIG: &str = include_str!("../example-config.json");

//! Agent discovery and compatibility control-plane contracts.
//!
//! This module is intentionally outside the routing data plane. Descriptors
//! are declarative data only: they cannot carry scripts, installation actions,
//! model choices, or routing rules.

pub mod commands;
pub mod compatibility;
pub mod config_codec;
pub mod connectors;
pub mod discovery;
pub mod drift;
pub mod ownership;
pub mod plan;
pub mod platform;
pub mod registry;
pub(crate) mod safe_fs;
pub mod snapshot;
pub mod transaction;
pub mod types;

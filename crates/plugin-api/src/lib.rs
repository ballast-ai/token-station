//! Stable adapter plugin ABI: the WIT worlds, and the `manifest.json` schema.
//!
//! # What crosses the boundary
//!
//! Adapter functions take and return JSON documents, not WIT records. The
//! Canonical IR is defined once, in `token-station-protocol`, where exact
//! `parse -> serialize` fixtures pin its wire format. Mirroring it into WIT
//! would create a second definition to keep in step, and WIT cannot express the
//! open JSON the IR carries: `ToolDef::parameters`, the raw inbound body, and
//! every `extensions` bag.
//!
//! That is why this crate does not depend on `token-station-protocol`. It knows
//! the *names* of the documents that cross the boundary, not their contents.
//! The cost of the choice is one serialization per call, and type errors that
//! surface at run time rather than at build time; the conformance suite is what
//! catches those.
//!
//! # Scope: northbound only
//!
//! This crate carries the agent (northbound) ABI: `agent-adapter-v1`. The
//! provider (southbound) ABI that used to live beside it — `provider-adapter-v1`
//! and its `host.sign` import — was retired when the host moved every
//! southbound translation onto the South-owned `token-station:adapter@2.0.0`
//! component ABI (`provider-adapter-v2`, loaded by `south-provider-runtime`).
//! Southbound packages are therefore not described by this manifest schema;
//! the host's plugin directory scan dispatches on `api_version` before it
//! decides which schema to parse.
//!
//! # Versioning
//!
//! [`AdapterKind::expected_api_version`] returns the WIT `world` an adapter must
//! be built against, and the manifest must name the same one. A breaking change
//! ships a `-v2` world alongside `-v1`; it never edits `-v1` in place, because
//! installed plugins are compiled artifacts that cannot be migrated.

mod manifest;

pub use manifest::{
    AdapterKind, AdapterManifest, AdapterMetadata, AdapterPermissions, Capability, ConformanceSpec,
    ManifestError, validate_package_relative_path, validate_plugin_name,
};

/// The adapter ABI, as WIT source.
///
/// Embedded so a host can hand it to plugin authors and to `wit-bindgen` without
/// depending on this crate's source layout. Its worlds are named by
/// [`AdapterKind::expected_api_version`], and a test in this crate asserts the
/// two never drift apart.
pub const ADAPTER_WIT: &str = include_str!("../wit/adapter.wit");

#[cfg(test)]
mod tests {
    use super::{ADAPTER_WIT, AdapterKind};
    use wit_parser::Resolve;

    fn resolve() -> Resolve {
        let mut resolve = Resolve::new();
        resolve
            .push_str("adapter.wit", ADAPTER_WIT)
            .expect("wit/adapter.wit must parse");
        resolve
    }

    #[test]
    fn wit_source_parses() {
        let _ = resolve();
    }

    #[test]
    fn every_adapter_kind_has_a_world_named_after_its_api_version() {
        let resolve = resolve();

        let expected = AdapterKind::Agent.expected_api_version();
        assert!(
            resolve.worlds.iter().any(|(_, w)| w.name == expected),
            "wit/adapter.wit declares no world named `{expected}`"
        );
    }

    #[test]
    fn no_world_reaches_the_network_or_the_file_system() {
        let resolve = resolve();

        for (_, world) in &resolve.worlds {
            for (key, _) in &world.imports {
                let name = resolve.name_world_key(key);
                assert!(
                    !name.contains("wasi:sockets") && !name.contains("wasi:filesystem"),
                    "world `{}` imports `{name}`; adapters have no network and no file system",
                    world.name
                );
            }
        }
    }

    #[test]
    fn no_world_names_a_credential() {
        let resolve = resolve();

        for (_, world) in &resolve.worlds {
            for (key, _) in &world.imports {
                let name = resolve.name_world_key(key);
                assert!(
                    !name.contains("host"),
                    "world `{}` imports `{name}`; the northbound ABI never touches a credential",
                    world.name
                );
            }
        }
    }
}

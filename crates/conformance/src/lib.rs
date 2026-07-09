//! Conformance runner primitives for adapter plugin validation.
//!
//! Runs in three places, per the adapter architecture: a plugin's own CI, an
//! instance installing a plugin, and an instance upgrading or canarying one. A
//! package that fails is stored as a draft and never enters the runtime
//! registry, so these checks are the whole of what stands between a third-party
//! `.wasm` and the request path.
//!
//! This crate currently implements the manifest and identity gates. The fixture
//! gates — request and response translation, error mapping, determinism, and the
//! sandbox bounds — arrive with the WASM runtime.

use token_station_plugin_api::{AdapterManifest, AdapterMetadata, ManifestError};

/// The manifest gate: everything the host can decide before loading any code.
///
/// # Errors
///
/// Returns the [`ManifestError`] that disqualified the package.
pub fn accepts_manifest(manifest: &AdapterManifest) -> Result<(), ManifestError> {
    manifest.validate()
}

/// The identity gate: what a loaded adapter reports must be what it declared.
///
/// A package whose `metadata()` disagrees with its `manifest.json` has either
/// been repackaged around a signature, or been built from different source than
/// it claims. Either way the registry's record of what is installed would be
/// wrong, and every later decision keyed on that record — canary by version,
/// rollback, capability dispatch — would act on a fiction.
#[must_use]
pub fn reported_identity_matches(reported: &AdapterMetadata, manifest: &AdapterManifest) -> bool {
    *reported == manifest.metadata()
}

#[cfg(test)]
mod tests {
    use super::{accepts_manifest, reported_identity_matches};
    use std::collections::BTreeSet;
    use token_station_plugin_api::{
        AdapterKind, AdapterManifest, AdapterPermissions, Capability, ConformanceSpec,
        ManifestError,
    };

    fn manifest() -> AdapterManifest {
        AdapterManifest {
            name: "provider-openai-compatible".to_owned(),
            version: "1.0.0".to_owned(),
            kind: AdapterKind::Provider,
            api_version: "provider-adapter-v1".to_owned(),
            agent_protocols: Vec::new(),
            agent_tools: Vec::new(),
            providers: vec!["openai-compatible".to_owned()],
            capabilities: BTreeSet::from([Capability::Chat]),
            permissions: AdapterPermissions::new(false, false, ["provider_api_key"]),
            conformance: ConformanceSpec {
                required_suite: "provider-protocol-v1".to_owned(),
                fixtures: "fixtures/".to_owned(),
            },
        }
    }

    #[test]
    fn accepts_a_well_formed_manifest() {
        assert_eq!(accepts_manifest(&manifest()), Ok(()));
    }

    #[test]
    fn surfaces_the_reason_a_manifest_was_refused() {
        let mut refused = manifest();
        refused.permissions.network = true;

        assert_eq!(
            accepts_manifest(&refused),
            Err(ManifestError::NetworkPermissionDenied)
        );
    }

    #[test]
    fn identity_gate_accepts_a_faithful_adapter() {
        let manifest = manifest();

        assert!(reported_identity_matches(&manifest.metadata(), &manifest));
    }

    #[test]
    fn identity_gate_rejects_an_adapter_reporting_a_version_it_did_not_declare() {
        let manifest = manifest();
        let mut reported = manifest.metadata();
        reported.version = "9.9.9".to_owned();

        assert!(!reported_identity_matches(&reported, &manifest));
    }
}

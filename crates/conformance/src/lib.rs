//! Conformance runner: the whole of what stands between a third-party `.wasm`
//! and the request path.
//!
//! Runs in three places, per the adapter architecture: a plugin's own CI, an
//! instance installing a plugin, and an instance upgrading or canarying one. A
//! package that fails is stored as a draft and never enters the runtime
//! registry, which is why a [`Report`] names every check that ran and why
//! [`Check`] is a closed enumeration: the registry has to record *what* it
//! refused a package for, and act on that record later.
//!
//! # Three gates, in order
//!
//! 1. [`accepts_manifest`] — everything decidable before loading any code.
//! 2. [`reported_identity_matches`] — the loaded adapter is the package that was
//!    vetted.
//! 3. [`run_agent_suite`] — the adapter translates what it claims to,
//!    deterministically, and tolerates a newer peer's fields.
//!
//! This crate gates the northbound (agent) side only. Southbound provider
//! components are South's: `south-component-conformance` holds their suite and
//! `south-provider-runtime` their load gates.
//!
//! # Why this crate does not know about WASM
//!
//! The suite is written against [`AgentAdapter`], not against a component
//! instance. `plugin-runtime` implements that trait over a WASM component; a
//! plugin author implements it over a native build and runs the same suite in
//! their own CI without a WASM toolchain. The seam is also why these gates
//! exist and are tested before any runtime does.
//!
//! What that seam cannot carry is the architecture's security row — no network, no
//! file system, bounded memory and time. Those are properties of the sandbox the
//! runtime constructs, not answers an adapter can be asked for, and no fixture
//! here pretends to check them.

mod adapter;
mod fixture;
mod report;
mod suite;

pub use adapter::{AdapterResult, AgentAdapter};
pub use fixture::{AgentFamily, Case, Family, FixtureError, FixturePack};
pub use report::{Check, Outcome, Report, Verdict};
pub use suite::run_agent_suite;

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
            name: "agent-openai".to_owned(),
            version: "1.0.0".to_owned(),
            kind: AdapterKind::Agent,
            api_version: "agent-adapter-v1".to_owned(),
            agent_protocols: vec!["openai-chat-completions".to_owned()],
            agent_tools: Vec::new(),
            capabilities: BTreeSet::from([Capability::Chat]),
            permissions: AdapterPermissions::new(false, false, Vec::<String>::new()),
            conformance: ConformanceSpec {
                required_suite: "agent-protocol-v1".to_owned(),
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

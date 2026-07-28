//! Durable identities for the editable, saved and running configuration.
//!
//! The user configuration remains `token-station.json`. Revision metadata is
//! kept in a sibling sidecar so CLI readers never need to understand desktop
//! control-plane state.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const LEDGER_VERSION: u32 = 1;
const LEDGER_FILE: &str = "token-station.state.json";

#[derive(Clone, Debug)]
pub(crate) struct ConfigState {
    saved: Value,
    draft: Value,
    draft_fingerprint: String,
    saved_fingerprint: String,
    draft_revision: u64,
    saved_revision: u64,
    ledger: RevisionLedger,
    ledger_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RevisionLedger {
    version: u32,
    last_allocated_revision: u64,
    committed_revision: u64,
    committed_fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_fingerprint: Option<String>,
}

impl ConfigState {
    pub(crate) fn read_only(config_path: &Path, draft: &Value) -> Self {
        let fingerprint = fingerprint(draft).expect("serde_json::Value always serializes");
        Self {
            saved: draft.clone(),
            draft: draft.clone(),
            draft_fingerprint: fingerprint.clone(),
            saved_fingerprint: fingerprint,
            draft_revision: 0,
            saved_revision: 0,
            ledger: RevisionLedger {
                version: LEDGER_VERSION,
                last_allocated_revision: 0,
                committed_revision: 0,
                committed_fingerprint: String::new(),
                pending_revision: None,
                pending_fingerprint: None,
            },
            ledger_path: config_path.with_file_name(LEDGER_FILE),
        }
    }

    /// Reconciles the sidecar with the configuration that was actually loaded.
    ///
    /// A missing sidecar or an external configuration edit allocates a fresh
    /// revision. A crash after the config rename but before the final sidecar
    /// write promotes the matching pending revision.
    pub(crate) fn load(config_path: &Path, draft: &Value) -> Result<Self, String> {
        let ledger_path = config_path.with_file_name(LEDGER_FILE);
        let fingerprint = fingerprint(draft)?;
        let config_exists = config_path.is_file();
        let mut ledger = read_ledger(&ledger_path).unwrap_or_else(|| RevisionLedger {
            version: LEDGER_VERSION,
            last_allocated_revision: 0,
            committed_revision: 0,
            committed_fingerprint: String::new(),
            pending_revision: None,
            pending_fingerprint: None,
        });

        let revision = if !config_exists {
            ledger.pending_revision = None;
            ledger.pending_fingerprint = None;
            0
        } else if ledger.pending_fingerprint.as_deref() == Some(&fingerprint) {
            let revision = ledger
                .pending_revision
                .unwrap_or_else(|| ledger.last_allocated_revision.saturating_add(1));
            ledger.last_allocated_revision = ledger.last_allocated_revision.max(revision);
            ledger.committed_revision = revision;
            ledger.committed_fingerprint.clone_from(&fingerprint);
            ledger.pending_revision = None;
            ledger.pending_fingerprint = None;
            revision
        } else if ledger.committed_fingerprint == fingerprint {
            ledger.pending_revision = None;
            ledger.pending_fingerprint = None;
            ledger.committed_revision
        } else {
            let revision = ledger.last_allocated_revision.saturating_add(1).max(1);
            ledger.last_allocated_revision = revision;
            ledger.committed_revision = revision;
            ledger.committed_fingerprint.clone_from(&fingerprint);
            ledger.pending_revision = None;
            ledger.pending_fingerprint = None;
            revision
        };

        if config_exists {
            write_ledger(&ledger_path, &ledger)?;
        }
        Ok(Self {
            saved: draft.clone(),
            draft: draft.clone(),
            draft_fingerprint: fingerprint.clone(),
            saved_fingerprint: fingerprint,
            draft_revision: revision,
            saved_revision: revision,
            ledger,
            ledger_path,
        })
    }

    #[must_use]
    pub(crate) const fn draft_revision(&self) -> u64 {
        self.draft_revision
    }

    #[must_use]
    pub(crate) const fn saved_revision(&self) -> u64 {
        self.saved_revision
    }

    #[must_use]
    pub(crate) fn is_dirty(&self) -> bool {
        self.draft_fingerprint != self.saved_fingerprint
    }

    #[must_use]
    pub(crate) fn draft(&self) -> &Value {
        &self.draft
    }

    /// Records a successful in-memory edit. Repeating the same value keeps its
    /// revision; returning exactly to the saved content reuses saved_revision.
    pub(crate) fn observe_draft(&mut self, draft: &Value) -> Result<(), String> {
        let next = fingerprint(draft)?;
        if next == self.draft_fingerprint {
            return Ok(());
        }
        if next == self.saved_fingerprint {
            self.draft = draft.clone();
            self.draft_fingerprint = next;
            self.draft_revision = self.saved_revision;
        } else {
            // Reserve the identity durably before exposing it in memory. A
            // crash with an unsaved draft must not allow a different draft to
            // reuse the same revision after restart.
            let mut ledger = self.ledger.clone();
            ledger.last_allocated_revision =
                ledger.last_allocated_revision.saturating_add(1).max(1);
            write_ledger(&self.ledger_path, &ledger)?;
            self.draft_revision = ledger.last_allocated_revision;
            self.draft = draft.clone();
            self.draft_fingerprint = next;
            self.ledger = ledger;
        }
        Ok(())
    }

    /// Writes the pending journal before the caller atomically replaces the
    /// configuration file.
    pub(crate) fn prepare_save(&mut self, draft: &Value) -> Result<u64, String> {
        self.observe_draft(draft)?;
        if self.draft_revision == 0 {
            self.ledger.last_allocated_revision =
                self.ledger.last_allocated_revision.saturating_add(1).max(1);
            self.draft_revision = self.ledger.last_allocated_revision;
        }
        self.ledger.pending_revision = Some(self.draft_revision);
        self.ledger.pending_fingerprint = Some(self.draft_fingerprint.clone());
        write_ledger(&self.ledger_path, &self.ledger)?;
        Ok(self.draft_revision)
    }

    /// Commits the in-memory saved snapshot after the configuration rename.
    ///
    /// The saved fact is updated before the final sidecar write. If that write
    /// fails, the pending entry remains recoverable on the next launch.
    pub(crate) fn finish_save(&mut self, draft: &Value) -> Result<(), String> {
        self.saved = draft.clone();
        self.saved_fingerprint.clone_from(&self.draft_fingerprint);
        self.saved_revision = self.draft_revision;
        self.ledger.committed_revision = self.saved_revision;
        self.ledger
            .committed_fingerprint
            .clone_from(&self.saved_fingerprint);
        self.ledger.pending_revision = None;
        self.ledger.pending_fingerprint = None;
        write_ledger(&self.ledger_path, &self.ledger)
    }
}

fn fingerprint(value: &Value) -> Result<String, String> {
    let bytes =
        serde_json::to_vec(&canonicalize(value)).map_err(|_| "配置指纹生成失败".to_owned())?;
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(encoded)
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let sorted = object
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize(value)))
                .collect::<std::collections::BTreeMap<_, _>>();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
        _ => value.clone(),
    }
}

fn read_ledger(path: &Path) -> Option<RevisionLedger> {
    let bytes = fs::read(path).ok()?;
    let ledger: RevisionLedger = serde_json::from_slice(&bytes).ok()?;
    (ledger.version == LEDGER_VERSION
        && ledger.committed_fingerprint.len() <= 64
        && ledger
            .pending_fingerprint
            .as_ref()
            .is_none_or(|value| value.len() == 64))
    .then_some(ledger)
}

fn write_ledger(path: &Path, ledger: &RevisionLedger) -> Result<(), String> {
    let mut rendered =
        serde_json::to_vec_pretty(ledger).map_err(|_| "序列化 revision sidecar 失败".to_owned())?;
    rendered.push(b'\n');
    crate::agent_integration::safe_fs::write_atomic_private(path, &rendered)
        .map_err(|_| "提交 revision sidecar 失败".to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{json, Value};

    use super::{fingerprint, read_ledger, write_ledger, ConfigState, RevisionLedger};

    fn scratch(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "token-station-config-state-{}-{label}",
            std::process::id()
        ));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        root
    }

    use std::path::PathBuf;

    #[test]
    fn edit_identity_is_stable_and_returning_to_saved_reuses_its_revision() {
        let root = scratch("dirty");
        let config = root.join("token-station.json");
        let a = json!({"model": "a"});
        fs::write(&config, serde_json::to_vec(&a).unwrap()).unwrap();
        let mut state = ConfigState::load(&config, &a).unwrap();
        let saved = state.saved_revision();

        state.observe_draft(&a).unwrap();
        assert_eq!(state.draft_revision(), saved);
        assert!(!state.is_dirty());

        let b = json!({"model": "b"});
        state.observe_draft(&b).unwrap();
        assert_ne!(state.draft_revision(), saved);
        assert!(state.is_dirty());
        let b_revision = state.draft_revision();
        state.observe_draft(&b).unwrap();
        assert_eq!(state.draft_revision(), b_revision);

        state.observe_draft(&a).unwrap();
        assert_eq!(state.draft_revision(), saved);
        assert!(!state.is_dirty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn first_save_allocates_a_non_zero_revision() {
        let root = scratch("first-save");
        let config = root.join("token-station.json");
        let draft = json!({"model": "a"});
        let mut state = ConfigState::load(&config, &draft).unwrap();

        assert_eq!(state.saved_revision(), 0);
        assert_eq!(state.prepare_save(&draft).unwrap(), 1);
        fs::write(&config, serde_json::to_vec(&draft).unwrap()).unwrap();
        state.finish_save(&draft).unwrap();
        assert_eq!(state.saved_revision(), 1);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn object_key_order_does_not_change_the_revision_identity() {
        let left: Value = serde_json::from_str(r#"{"a":1,"b":{"x":2,"y":3}}"#).unwrap();
        let right: Value = serde_json::from_str(r#"{"b":{"y":3,"x":2},"a":1}"#).unwrap();
        assert_eq!(fingerprint(&left).unwrap(), fingerprint(&right).unwrap());
    }

    #[test]
    fn pending_revision_recovers_a_crash_after_the_config_rename() {
        let root = scratch("pending");
        let config = root.join("token-station.json");
        let a = json!({"model": "a"});
        fs::write(&config, serde_json::to_vec(&a).unwrap()).unwrap();
        let mut state = ConfigState::load(&config, &a).unwrap();
        let b = json!({"model": "b"});
        state.observe_draft(&b).unwrap();
        let pending = state.prepare_save(&b).unwrap();
        fs::write(&config, serde_json::to_vec(&b).unwrap()).unwrap();

        let recovered = ConfigState::load(&config, &b).unwrap();
        assert_eq!(recovered.saved_revision(), pending);
        let ledger = read_ledger(&root.join("token-station.state.json")).unwrap();
        assert_eq!(ledger.committed_revision, pending);
        assert_eq!(ledger.pending_revision, None);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn external_config_changes_receive_a_new_revision() {
        let root = scratch("external");
        let config = root.join("token-station.json");
        let a = json!({"model": "a"});
        fs::write(&config, serde_json::to_vec(&a).unwrap()).unwrap();
        let first = ConfigState::load(&config, &a).unwrap();
        let b = json!({"model": "b"});
        fs::write(&config, serde_json::to_vec(&b).unwrap()).unwrap();
        let second = ConfigState::load(&config, &b).unwrap();
        assert!(second.saved_revision() > first.saved_revision());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_unsaved_draft_revision_is_never_reused_after_restart() {
        let root = scratch("unsaved-restart");
        let config = root.join("token-station.json");
        let a = json!({"model": "a"});
        let b = json!({"model": "b"});
        let c = json!({"model": "c"});
        fs::write(&config, serde_json::to_vec(&a).unwrap()).unwrap();

        let mut first_session = ConfigState::load(&config, &a).unwrap();
        first_session.observe_draft(&b).unwrap();
        let abandoned_revision = first_session.draft_revision();
        drop(first_session);

        let mut second_session = ConfigState::load(&config, &a).unwrap();
        assert_eq!(
            second_session.draft_revision(),
            second_session.saved_revision()
        );
        second_session.observe_draft(&c).unwrap();
        assert!(second_session.draft_revision() > abandoned_revision);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_failed_revision_reservation_does_not_publish_the_draft_in_memory() {
        let root = scratch("reservation-failure");
        let config = root.join("token-station.json");
        let ledger = root.join("token-station.state.json");
        let a = json!({"model": "a"});
        let b = json!({"model": "b"});
        fs::write(&config, serde_json::to_vec(&a).unwrap()).unwrap();
        let mut state = ConfigState::load(&config, &a).unwrap();
        let revision = state.draft_revision();
        fs::remove_file(&ledger).unwrap();
        fs::create_dir(&ledger).unwrap();

        assert!(state.observe_draft(&b).is_err());
        assert_eq!(state.draft(), &a);
        assert_eq!(state.draft_revision(), revision);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_pending_entry_for_an_uncommitted_config_is_discarded() {
        let root = scratch("pending-before-config");
        let config = root.join("token-station.json");
        let a = json!({"model": "a"});
        let b = json!({"model": "b"});
        fs::write(&config, serde_json::to_vec(&a).unwrap()).unwrap();
        let mut state = ConfigState::load(&config, &a).unwrap();
        state.observe_draft(&b).unwrap();
        state.prepare_save(&b).unwrap();

        let recovered = ConfigState::load(&config, &a).unwrap();
        assert_eq!(recovered.saved, a);
        assert_eq!(recovered.draft_revision(), recovered.saved_revision());
        let ledger = read_ledger(&root.join("token-station.state.json")).unwrap();
        assert_eq!(ledger.pending_revision, None);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn sidecar_rejects_unknown_versions_without_echoing_content() {
        let root = scratch("version");
        let path = root.join("token-station.state.json");
        let ledger = RevisionLedger {
            version: 99,
            last_allocated_revision: 1,
            committed_revision: 1,
            committed_fingerprint: fingerprint(&json!({"secret": "never-persisted"})).unwrap(),
            pending_revision: None,
            pending_fingerprint: None,
        };
        write_ledger(&path, &ledger).unwrap();
        assert!(read_ledger(&path).is_none());
        fs::remove_dir_all(root).ok();
    }
}

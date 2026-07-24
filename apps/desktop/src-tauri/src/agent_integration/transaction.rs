use std::collections::{BTreeSet, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;

use super::config_codec::{parse_source_bytes, ConfigDocument};
use super::ownership::{
    compute_owned_value_macs, ownership_matches, CompanionOwnership, OwnershipKey, OwnershipRecord,
    OwnershipStore,
};
use super::plan::{
    file_revision_hash, read_config_source, OwnershipDisposition, PreparedChangePlan,
};
use super::safe_fs::{atomic_replace, sync_parent};
use super::snapshot::{MasterKeyStore, SnapshotRequest, SnapshotStore};
use super::types::{CompatibilityStatus, ConfirmationKind, PlanIntent};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionStage {
    Confirmation,
    Admission,
    Ownership,
    Revision,
    Snapshot,
    TargetWrite,
    PostWriteRead,
    PostWriteParse,
    PostWriteSelfCheck,
    OwnershipCommit,
    Restore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStatus {
    NotRequired,
    Restored,
    RepairRequired,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionFailure {
    pub operation_id: String,
    pub stage: TransactionStage,
    pub reason_code: String,
    pub recovery: RecoveryStatus,
    pub recovery_reason_code: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionOutcome {
    pub operation_id: String,
    pub agent_id: String,
    pub target_config_path: String,
    pub before_hash: String,
    pub after_hash: String,
    pub snapshot_id: String,
    pub ownership_revision: u64,
    pub maintenance_warning: Option<String>,
}

pub struct ConfirmedOperation {
    pub operation_id: String,
    pub confirmed_at_ms: u64,
    pub confirmations: BTreeSet<ConfirmationKind>,
}

pub struct RuntimeAdmission {
    pub compatibility_sequence: u64,
    pub status: CompatibilityStatus,
}

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        )
        .unwrap_or(u64::MAX)
    }
}

pub trait PostWriteVerifier: Send + Sync {
    fn verify(&self, path: &Path, document: &ConfigDocument) -> Result<(), String>;
}

pub struct ParseOnlyVerifier;

impl PostWriteVerifier for ParseOnlyVerifier {
    fn verify(&self, _path: &Path, _document: &ConfigDocument) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtomicWriteStage {
    ParentCreate,
    TempCreate,
    TempWrite,
    TempFlush,
    TempFsync,
    Permission,
    PreReplaceRevision,
    Replace,
    DirectoryFsync,
    Remove,
}

pub struct AtomicWriteFailure {
    pub stage: AtomicWriteStage,
    pub target_replaced: bool,
}

pub trait AtomicConfigWriter: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    fn replace(
        &self,
        target: &Path,
        bytes: &[u8],
        permissions: Option<u32>,
        owner: Option<&str>,
        expected_current_hash: &str,
        normalize_new_owner: bool,
    ) -> Result<(), AtomicWriteFailure>;

    fn remove(
        &self,
        target: &Path,
        expected_current_hash: &str,
        normalize_new_owner: bool,
    ) -> Result<(), AtomicWriteFailure>;
}

pub struct FsAtomicConfigWriter;

impl AtomicConfigWriter for FsAtomicConfigWriter {
    fn replace(
        &self,
        target: &Path,
        bytes: &[u8],
        permissions: Option<u32>,
        owner: Option<&str>,
        expected_current_hash: &str,
        normalize_new_owner: bool,
    ) -> Result<(), AtomicWriteFailure> {
        let parent = target.parent().ok_or(AtomicWriteFailure {
            stage: AtomicWriteStage::ParentCreate,
            target_replaced: false,
        })?;
        std::fs::create_dir_all(parent).map_err(|_| AtomicWriteFailure {
            stage: AtomicWriteStage::ParentCreate,
            target_replaced: false,
        })?;
        let temporary = temporary_path(target).map_err(|stage| AtomicWriteFailure {
            stage,
            target_replaced: false,
        })?;
        let result = (|| {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary).map_err(|_| AtomicWriteFailure {
                stage: AtomicWriteStage::TempCreate,
                target_replaced: false,
            })?;
            file.write_all(bytes).map_err(|_| AtomicWriteFailure {
                stage: AtomicWriteStage::TempWrite,
                target_replaced: false,
            })?;
            file.flush().map_err(|_| AtomicWriteFailure {
                stage: AtomicWriteStage::TempFlush,
                target_replaced: false,
            })?;
            file.sync_all().map_err(|_| AtomicWriteFailure {
                stage: AtomicWriteStage::TempFsync,
                target_replaced: false,
            })?;
            apply_metadata(&file, permissions, owner).map_err(|_| AtomicWriteFailure {
                stage: AtomicWriteStage::Permission,
                target_replaced: false,
            })?;
            file.sync_all().map_err(|_| AtomicWriteFailure {
                stage: AtomicWriteStage::TempFsync,
                target_replaced: false,
            })?;
            drop(file);

            verify_revision(target, expected_current_hash, normalize_new_owner).map_err(|_| {
                AtomicWriteFailure {
                    stage: AtomicWriteStage::PreReplaceRevision,
                    target_replaced: false,
                }
            })?;
            atomic_replace(&temporary, target).map_err(|_| AtomicWriteFailure {
                stage: AtomicWriteStage::Replace,
                target_replaced: false,
            })?;
            sync_parent(parent).map_err(|_| AtomicWriteFailure {
                stage: AtomicWriteStage::DirectoryFsync,
                target_replaced: true,
            })?;
            Ok(())
        })();
        if result.is_err() {
            std::fs::remove_file(&temporary).ok();
        }
        result
    }

    fn remove(
        &self,
        target: &Path,
        expected_current_hash: &str,
        normalize_new_owner: bool,
    ) -> Result<(), AtomicWriteFailure> {
        verify_revision(target, expected_current_hash, normalize_new_owner).map_err(|_| {
            AtomicWriteFailure {
                stage: AtomicWriteStage::PreReplaceRevision,
                target_replaced: false,
            }
        })?;
        std::fs::remove_file(target).map_err(|_| AtomicWriteFailure {
            stage: AtomicWriteStage::Remove,
            target_replaced: false,
        })?;
        let parent = target.parent().ok_or(AtomicWriteFailure {
            stage: AtomicWriteStage::DirectoryFsync,
            target_replaced: true,
        })?;
        sync_parent(parent).map_err(|_| AtomicWriteFailure {
            stage: AtomicWriteStage::DirectoryFsync,
            target_replaced: true,
        })
    }
}

#[cfg(unix)]
fn apply_metadata(
    file: &std::fs::File,
    permissions: Option<u32>,
    owner: Option<&str>,
) -> Result<(), ()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(owner) = owner {
        let (uid, gid) = owner.split_once(':').ok_or(())?;
        let uid = uid.parse::<u32>().map_err(|_| ())?;
        let gid = gid.parse::<u32>().map_err(|_| ())?;
        std::os::unix::fs::fchown(file, Some(uid), Some(gid)).map_err(|_| ())?;
    }
    let mode = permissions.unwrap_or(0o600);
    file.set_permissions(std::fs::Permissions::from_mode(mode))
        .map_err(|_| ())?;
    Ok(())
}

#[cfg(not(unix))]
fn apply_metadata(
    _file: &std::fs::File,
    _permissions: Option<u32>,
    _owner: Option<&str>,
) -> Result<(), ()> {
    Ok(())
}

fn temporary_path(target: &Path) -> Result<PathBuf, AtomicWriteStage> {
    let file_name = target
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or(AtomicWriteStage::TempCreate)?;
    for _ in 0..16 {
        let mut random = [0_u8; 12];
        getrandom::fill(&mut random).map_err(|_| AtomicWriteStage::TempCreate)?;
        let suffix = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = target.with_file_name(format!(".{file_name}.{suffix}.token-station.tmp"));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(AtomicWriteStage::TempCreate)
}

fn verify_revision(
    target: &Path,
    expected_hash: &str,
    normalize_new_owner: bool,
) -> Result<(), String> {
    let mut source = read_config_source(target)?;
    if normalize_new_owner {
        source.original_owner = None;
    }
    if file_revision_hash(target, &source)? == expected_hash {
        Ok(())
    } else {
        Err("target revision changed".to_string())
    }
}

pub struct TransactionEngine<'a> {
    snapshots: &'a dyn SnapshotStore,
    ownership: &'a dyn OwnershipStore,
    keys: &'a dyn MasterKeyStore,
    writer: &'a dyn AtomicConfigWriter,
    verifier: &'a dyn PostWriteVerifier,
    clock: &'a dyn Clock,
    consumed: Mutex<HashSet<String>>,
    operation_lock: Mutex<()>,
}

impl<'a> TransactionEngine<'a> {
    #[must_use]
    pub fn new(
        snapshots: &'a dyn SnapshotStore,
        ownership: &'a dyn OwnershipStore,
        keys: &'a dyn MasterKeyStore,
        writer: &'a dyn AtomicConfigWriter,
        verifier: &'a dyn PostWriteVerifier,
        clock: &'a dyn Clock,
    ) -> Self {
        Self {
            snapshots,
            ownership,
            keys,
            writer,
            verifier,
            clock,
            consumed: Mutex::new(HashSet::new()),
            operation_lock: Mutex::new(()),
        }
    }

    pub fn apply_connection(
        &self,
        plan: &PreparedChangePlan,
        confirmation: &ConfirmedOperation,
        admission: &RuntimeAdmission,
        now_ms: u64,
    ) -> Result<TransactionOutcome, TransactionFailure> {
        if plan.view.intent != PlanIntent::Connect {
            return Err(failure(
                plan,
                TransactionStage::Admission,
                "operation_intent_mismatch",
            ));
        }
        self.apply_prepared(plan, confirmation, admission, now_ms)
    }

    pub fn apply_disconnect(
        &self,
        plan: &PreparedChangePlan,
        confirmation: &ConfirmedOperation,
        admission: &RuntimeAdmission,
        now_ms: u64,
    ) -> Result<TransactionOutcome, TransactionFailure> {
        if plan.view.intent != PlanIntent::Disconnect {
            return Err(failure(
                plan,
                TransactionStage::Admission,
                "operation_intent_mismatch",
            ));
        }
        self.apply_prepared(plan, confirmation, admission, now_ms)
    }

    pub fn apply_snapshot_restore(
        &self,
        plan: &PreparedChangePlan,
        confirmation: &ConfirmedOperation,
        admission: &RuntimeAdmission,
        now_ms: u64,
    ) -> Result<TransactionOutcome, TransactionFailure> {
        if plan.view.intent != PlanIntent::Restore {
            return Err(failure(
                plan,
                TransactionStage::Admission,
                "operation_intent_mismatch",
            ));
        }
        self.apply_prepared(plan, confirmation, admission, now_ms)
    }

    fn apply_prepared(
        &self,
        plan: &PreparedChangePlan,
        confirmation: &ConfirmedOperation,
        admission: &RuntimeAdmission,
        now_ms: u64,
    ) -> Result<TransactionOutcome, TransactionFailure> {
        self.validate_confirmation(plan, confirmation, now_ms)?;
        self.validate_admission(plan, admission, now_ms)?;
        let _operation_guard = self
            .operation_lock
            .lock()
            .map_err(|_| failure(plan, TransactionStage::Revision, "operation_lock_poisoned"))?;
        let ownership_key = OwnershipKey {
            agent_id: plan.view.agent_id.clone(),
            installation_path: plan.view.installation_path.clone(),
            target_config_path: plan.view.target_config_path.clone(),
        };
        let active_ownership = self
            .ownership
            .load(&ownership_key)
            .map_err(|_| failure(plan, TransactionStage::Ownership, "ownership_read_failed"))?;
        match (
            plan.view.intent,
            active_ownership.as_ref(),
            plan.ownership.as_ref(),
        ) {
            (PlanIntent::Connect, None, None) => {}
            (PlanIntent::Connect, _, _) => {
                return Err(failure(
                    plan,
                    TransactionStage::Ownership,
                    "ownership_already_active",
                ));
            }
            (PlanIntent::Disconnect | PlanIntent::Restore, Some(active), Some(binding))
                if active.revision == binding.record.revision
                    && active.key() == binding.record.key() => {}
            (PlanIntent::Disconnect | PlanIntent::Restore, _, _) => {
                return Err(failure(
                    plan,
                    TransactionStage::Ownership,
                    "ownership_changed_or_missing",
                ));
            }
        }
        let target = Path::new(&plan.view.target_config_path);
        let current = read_config_source(target)
            .map_err(|_| failure(plan, TransactionStage::Revision, "target_read_failed"))?;
        let current_hash = file_revision_hash(target, &current)
            .map_err(|_| failure(plan, TransactionStage::Revision, "revision_hash_failed"))?;
        if current_hash != plan.view.before_hash {
            return Err(failure(
                plan,
                TransactionStage::Revision,
                "before_hash_changed",
            ));
        }
        let mut companion_currents = Vec::with_capacity(plan.companions.len());
        for companion in &plan.companions {
            let target = Path::new(&companion.target_path);
            let source = read_config_source(target).map_err(|_| {
                failure(
                    plan,
                    TransactionStage::Revision,
                    "companion_target_read_failed",
                )
            })?;
            let hash = file_revision_hash(target, &source).map_err(|_| {
                failure(
                    plan,
                    TransactionStage::Revision,
                    "companion_revision_hash_failed",
                )
            })?;
            if hash != companion.before_hash {
                return Err(failure(
                    plan,
                    TransactionStage::Revision,
                    "companion_before_hash_changed",
                ));
            }
            companion_currents.push(source);
        }
        if let Some(binding) = &plan.ownership {
            let document = parse_source_bytes(
                current.existed.then_some(current.exact_bytes.as_slice()),
                plan.format,
                plan.label,
            )
            .map_err(|_| {
                failure(
                    plan,
                    TransactionStage::Ownership,
                    "ownership_source_parse_failed",
                )
            })?;
            let key = self.keys.load().map_err(|_| {
                failure(
                    plan,
                    TransactionStage::Ownership,
                    "ownership_key_unavailable",
                )
            })?;
            let matches_record =
                ownership_matches(&binding.record, &document, &key).map_err(|_| {
                    failure(plan, TransactionStage::Ownership, "ownership_check_failed")
                })?;
            let projected_document = parse_source_bytes(
                Some(plan.projected_bytes.as_slice()),
                plan.format,
                plan.label,
            )
            .map_err(|_| {
                failure(
                    plan,
                    TransactionStage::Ownership,
                    "ownership_projection_parse_failed",
                )
            })?;
            let matches_projection =
                compute_owned_value_macs(&document, &binding.record.owned_paths, &key)
                    .and_then(|observed| {
                        compute_owned_value_macs(
                            &projected_document,
                            &binding.record.owned_paths,
                            &key,
                        )
                        .map(|projected| observed == projected)
                    })
                    .map_err(|_| {
                        failure(plan, TransactionStage::Ownership, "ownership_check_failed")
                    })?;
            if !matches_record && !matches_projection {
                return Err(failure(
                    plan,
                    TransactionStage::Ownership,
                    "owned_values_changed",
                ));
            }
            for (companion, source) in plan.companions.iter().zip(&companion_currents) {
                let owned = binding
                    .record
                    .companion_files
                    .iter()
                    .find(|owned| owned.target_config_path == companion.target_path)
                    .ok_or_else(|| {
                        failure(
                            plan,
                            TransactionStage::Ownership,
                            "companion_ownership_missing",
                        )
                    })?;
                let document = parse_source_bytes(
                    source.existed.then_some(source.exact_bytes.as_slice()),
                    companion.format,
                    companion.label,
                )
                .map_err(|_| {
                    failure(
                        plan,
                        TransactionStage::Ownership,
                        "companion_ownership_source_parse_failed",
                    )
                })?;
                let observed = compute_owned_value_macs(&document, &owned.owned_paths, &key)
                    .map_err(|_| {
                        failure(
                            plan,
                            TransactionStage::Ownership,
                            "companion_ownership_check_failed",
                        )
                    })?;
                let projected_document = parse_source_bytes(
                    Some(companion.projected_bytes.as_slice()),
                    companion.format,
                    companion.label,
                )
                .map_err(|_| {
                    failure(
                        plan,
                        TransactionStage::Ownership,
                        "companion_ownership_projection_parse_failed",
                    )
                })?;
                let projected =
                    compute_owned_value_macs(&projected_document, &owned.owned_paths, &key)
                        .map_err(|_| {
                            failure(
                                plan,
                                TransactionStage::Ownership,
                                "companion_ownership_check_failed",
                            )
                        })?;
                if observed != owned.owned_value_macs && observed != projected {
                    return Err(failure(
                        plan,
                        TransactionStage::Ownership,
                        "companion_owned_values_changed",
                    ));
                }
            }
        }
        self.consume(plan)?;
        let snapshot = self
            .snapshots
            .create(SnapshotRequest {
                operation_id: &plan.view.operation_id,
                agent_id: &plan.view.agent_id,
                target_config_path: &plan.view.target_config_path,
                before_hash: plan.view.before_hash.clone(),
                source: &current,
                created_at_ms: now_ms,
                connector_id: &plan.view.connector_id,
                app_version: env!("CARGO_PKG_VERSION"),
                pinned: plan.view.intent == PlanIntent::Connect,
            })
            .map_err(|_| failure(plan, TransactionStage::Snapshot, "snapshot_create_failed"))?;
        let mut companion_snapshots = Vec::with_capacity(plan.companions.len());
        for (companion, source) in plan.companions.iter().zip(&companion_currents) {
            let created = self
                .snapshots
                .create(SnapshotRequest {
                    operation_id: &plan.view.operation_id,
                    agent_id: &plan.view.agent_id,
                    target_config_path: &companion.target_path,
                    before_hash: companion.before_hash.clone(),
                    source,
                    created_at_ms: now_ms,
                    connector_id: &plan.view.connector_id,
                    app_version: env!("CARGO_PKG_VERSION"),
                    pinned: plan.view.intent == PlanIntent::Connect,
                })
                .map_err(|_| {
                    failure(
                        plan,
                        TransactionStage::Snapshot,
                        "companion_snapshot_create_failed",
                    )
                })?;
            companion_snapshots.push(created);
        }
        if self.clock.now_ms() > plan.view.expires_at_ms {
            return Err(failure(
                plan,
                TransactionStage::Confirmation,
                "plan_expired_after_snapshot",
            ));
        }

        if let Err(write_failure) = self.writer.replace(
            target,
            plan.projected_bytes.as_slice(),
            current.original_permissions.or(Some(0o600)),
            current.original_owner.as_deref(),
            &plan.view.before_hash,
            false,
        ) {
            let mut error = failure(
                plan,
                TransactionStage::TargetWrite,
                atomic_reason(write_failure.stage),
            );
            if write_failure.target_replaced {
                self.recover_files(
                    plan,
                    &snapshot.record.snapshot_id,
                    &companion_snapshots,
                    0,
                    true,
                    &mut error,
                );
            }
            return Err(error);
        }

        for (index, (companion, source)) in
            plan.companions.iter().zip(&companion_currents).enumerate()
        {
            if let Err(write_failure) = self.writer.replace(
                Path::new(&companion.target_path),
                companion.projected_bytes.as_slice(),
                source.original_permissions.or(Some(0o600)),
                source.original_owner.as_deref(),
                &companion.before_hash,
                false,
            ) {
                let mut error = failure(
                    plan,
                    TransactionStage::TargetWrite,
                    atomic_reason(write_failure.stage),
                );
                self.recover_files(
                    plan,
                    &snapshot.record.snapshot_id,
                    &companion_snapshots,
                    index + usize::from(write_failure.target_replaced),
                    true,
                    &mut error,
                );
                return Err(error);
            }
        }

        let after_source = match read_config_source(target) {
            Ok(source) => source,
            Err(_) => {
                let mut error = failure(
                    plan,
                    TransactionStage::PostWriteRead,
                    "post_write_read_failed",
                );
                self.recover_files(
                    plan,
                    &snapshot.record.snapshot_id,
                    &companion_snapshots,
                    plan.companions.len(),
                    true,
                    &mut error,
                );
                return Err(error);
            }
        };
        let mut normalized_after = after_source.clone();
        if !plan.view.target_existed {
            normalized_after.original_owner = None;
        }
        let observed_after_hash = match file_revision_hash(target, &normalized_after) {
            Ok(hash) => hash,
            Err(_) => {
                let mut error = failure(plan, TransactionStage::PostWriteRead, "after_hash_failed");
                self.recover_files(
                    plan,
                    &snapshot.record.snapshot_id,
                    &companion_snapshots,
                    plan.companions.len(),
                    true,
                    &mut error,
                );
                return Err(error);
            }
        };
        if observed_after_hash != plan.view.expected_after_hash {
            let mut error = failure(plan, TransactionStage::PostWriteRead, "after_hash_mismatch");
            self.recover_files(
                plan,
                &snapshot.record.snapshot_id,
                &companion_snapshots,
                plan.companions.len(),
                true,
                &mut error,
            );
            return Err(error);
        }
        let document = match parse_source_bytes(
            Some(after_source.exact_bytes.as_slice()),
            plan.format,
            plan.label,
        ) {
            Ok(document) => document,
            Err(_) => {
                let mut error = failure(
                    plan,
                    TransactionStage::PostWriteParse,
                    "post_write_parse_failed",
                );
                self.recover_files(
                    plan,
                    &snapshot.record.snapshot_id,
                    &companion_snapshots,
                    plan.companions.len(),
                    true,
                    &mut error,
                );
                return Err(error);
            }
        };
        if self.verifier.verify(target, &document).is_err() {
            let mut error = failure(
                plan,
                TransactionStage::PostWriteSelfCheck,
                "post_write_self_check_failed",
            );
            self.recover_files(
                plan,
                &snapshot.record.snapshot_id,
                &companion_snapshots,
                plan.companions.len(),
                true,
                &mut error,
            );
            return Err(error);
        }
        let mut companion_documents = Vec::with_capacity(plan.companions.len());
        for companion in &plan.companions {
            let target = Path::new(&companion.target_path);
            let source = match read_config_source(target) {
                Ok(source) => source,
                Err(_) => {
                    let mut error = failure(
                        plan,
                        TransactionStage::PostWriteRead,
                        "companion_post_write_read_failed",
                    );
                    self.recover_files(
                        plan,
                        &snapshot.record.snapshot_id,
                        &companion_snapshots,
                        plan.companions.len(),
                        true,
                        &mut error,
                    );
                    return Err(error);
                }
            };
            let mut normalized = source.clone();
            if !companion.target_existed {
                normalized.original_owner = None;
            }
            let hash = file_revision_hash(target, &normalized).map_err(|_| {
                failure(
                    plan,
                    TransactionStage::PostWriteRead,
                    "companion_after_hash_failed",
                )
            });
            if hash.as_deref() != Ok(companion.expected_after_hash.as_str()) {
                let mut error = failure(
                    plan,
                    TransactionStage::PostWriteRead,
                    "companion_after_hash_mismatch",
                );
                self.recover_files(
                    plan,
                    &snapshot.record.snapshot_id,
                    &companion_snapshots,
                    plan.companions.len(),
                    true,
                    &mut error,
                );
                return Err(error);
            }
            let parsed = match parse_source_bytes(
                Some(source.exact_bytes.as_slice()),
                companion.format,
                companion.label,
            ) {
                Ok(document) => document,
                Err(_) => {
                    let mut error = failure(
                        plan,
                        TransactionStage::PostWriteParse,
                        "companion_post_write_parse_failed",
                    );
                    self.recover_files(
                        plan,
                        &snapshot.record.snapshot_id,
                        &companion_snapshots,
                        plan.companions.len(),
                        true,
                        &mut error,
                    );
                    return Err(error);
                }
            };
            if self.verifier.verify(target, &parsed).is_err() {
                let mut error = failure(
                    plan,
                    TransactionStage::PostWriteSelfCheck,
                    "companion_post_write_self_check_failed",
                );
                self.recover_files(
                    plan,
                    &snapshot.record.snapshot_id,
                    &companion_snapshots,
                    plan.companions.len(),
                    true,
                    &mut error,
                );
                return Err(error);
            }
            companion_documents.push(parsed);
        }
        let key = match self.keys.load() {
            Ok(key) => key,
            Err(_) => {
                let mut error = failure(
                    plan,
                    TransactionStage::OwnershipCommit,
                    "ownership_key_unavailable",
                );
                self.recover_files(
                    plan,
                    &snapshot.record.snapshot_id,
                    &companion_snapshots,
                    plan.companions.len(),
                    true,
                    &mut error,
                );
                return Err(error);
            }
        };
        let owned_value_macs =
            match compute_owned_value_macs(&document, &plan.view.owned_paths, &key) {
                Ok(macs) => macs,
                Err(_) => {
                    let mut error = failure(
                        plan,
                        TransactionStage::OwnershipCommit,
                        "ownership_mac_failed",
                    );
                    self.recover_files(
                        plan,
                        &snapshot.record.snapshot_id,
                        &companion_snapshots,
                        plan.companions.len(),
                        true,
                        &mut error,
                    );
                    return Err(error);
                }
            };
        let companion_ownership = plan
            .companions
            .iter()
            .zip(&companion_documents)
            .zip(&companion_snapshots)
            .map(|((companion, document), snapshot)| {
                compute_owned_value_macs(document, &companion.owned_paths, &key).map(|macs| {
                    CompanionOwnership {
                        target_config_path: companion.target_path.clone(),
                        baseline_snapshot_id: snapshot.record.snapshot_id.clone(),
                        last_transaction_snapshot_id: snapshot.record.snapshot_id.clone(),
                        before_hash: companion.before_hash.clone(),
                        managed_after_hash: companion.expected_after_hash.clone(),
                        owned_paths: companion.owned_paths.clone(),
                        owned_value_macs: macs,
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>();
        let companion_ownership = match companion_ownership {
            Ok(value) => value,
            Err(_) => {
                let mut error = failure(
                    plan,
                    TransactionStage::OwnershipCommit,
                    "companion_ownership_mac_failed",
                );
                self.recover_files(
                    plan,
                    &snapshot.record.snapshot_id,
                    &companion_snapshots,
                    plan.companions.len(),
                    true,
                    &mut error,
                );
                return Err(error);
            }
        };
        let ownership_result = match plan.view.intent {
            PlanIntent::Connect => self.ownership.commit(
                OwnershipRecord {
                    schema_version: 1,
                    revision: 0,
                    agent_id: plan.view.agent_id.clone(),
                    installation_path: plan.view.installation_path.clone(),
                    target_config_path: plan.view.target_config_path.clone(),
                    connector_id: plan.view.connector_id.clone(),
                    baseline_snapshot_id: snapshot.record.snapshot_id.clone(),
                    last_transaction_snapshot_id: snapshot.record.snapshot_id.clone(),
                    before_hash: plan.view.before_hash.clone(),
                    managed_after_hash: plan.view.expected_after_hash.clone(),
                    owned_paths: plan.view.owned_paths.clone(),
                    owned_value_macs,
                    companion_files: companion_ownership.clone(),
                    acquired_at_ms: now_ms,
                    updated_at_ms: now_ms,
                },
                None,
            ),
            PlanIntent::Disconnect => {
                let binding = plan
                    .ownership
                    .as_ref()
                    .expect("disconnect binding validated");
                if binding.disposition != OwnershipDisposition::Remove {
                    Err("ownership disposition mismatch".to_string())
                } else {
                    self.ownership
                        .remove(&binding.record.key(), binding.record.revision)
                        .map(|()| {
                            let mut removed = binding.record.clone();
                            removed.revision = 0;
                            removed
                        })
                }
            }
            PlanIntent::Restore => {
                let binding = plan.ownership.as_ref().expect("restore binding validated");
                if binding.disposition != OwnershipDisposition::Update {
                    Err("ownership disposition mismatch".to_string())
                } else {
                    let mut updated = binding.record.clone();
                    updated.last_transaction_snapshot_id = snapshot.record.snapshot_id.clone();
                    updated.managed_after_hash = plan.view.expected_after_hash.clone();
                    updated.owned_value_macs = owned_value_macs;
                    let mut companion_missing = false;
                    for companion in &mut updated.companion_files {
                        if let Some(projected) = companion_ownership.iter().find(|projected| {
                            projected.target_config_path == companion.target_config_path
                        }) {
                            companion.last_transaction_snapshot_id =
                                projected.last_transaction_snapshot_id.clone();
                            companion.managed_after_hash = projected.managed_after_hash.clone();
                            companion.owned_value_macs = projected.owned_value_macs.clone();
                        } else {
                            companion_missing = true;
                        }
                    }
                    updated.updated_at_ms = now_ms;
                    if companion_missing {
                        Err("companion ownership projection missing".to_string())
                    } else {
                        self.ownership
                            .commit(updated, Some(binding.record.revision))
                    }
                }
            }
        };
        let ownership = match ownership_result {
            Ok(ownership) => ownership,
            Err(_) => {
                let mut error = failure(
                    plan,
                    TransactionStage::OwnershipCommit,
                    "ownership_commit_failed",
                );
                self.recover_files(
                    plan,
                    &snapshot.record.snapshot_id,
                    &companion_snapshots,
                    plan.companions.len(),
                    true,
                    &mut error,
                );
                return Err(error);
            }
        };
        let mut maintenance_warning = snapshot.maintenance_warning;
        if plan.view.intent == PlanIntent::Disconnect {
            let baseline_id = &plan
                .ownership
                .as_ref()
                .expect("disconnect binding validated")
                .record
                .baseline_snapshot_id;
            if self.snapshots.set_pinned(baseline_id, false).is_err() {
                maintenance_warning = Some("断开已成功，但基线快照取消固定失败".to_string());
            }
            for companion in &plan
                .ownership
                .as_ref()
                .expect("disconnect binding validated")
                .record
                .companion_files
            {
                if self
                    .snapshots
                    .set_pinned(&companion.baseline_snapshot_id, false)
                    .is_err()
                {
                    maintenance_warning =
                        Some("断开已成功，但 companion 基线快照取消固定失败".to_string());
                }
            }
        }
        Ok(TransactionOutcome {
            operation_id: plan.view.operation_id.clone(),
            agent_id: plan.view.agent_id.clone(),
            target_config_path: plan.view.target_config_path.clone(),
            before_hash: plan.view.before_hash.clone(),
            after_hash: plan.view.expected_after_hash.clone(),
            snapshot_id: snapshot.record.snapshot_id,
            ownership_revision: ownership.revision,
            maintenance_warning,
        })
    }

    fn validate_confirmation(
        &self,
        plan: &PreparedChangePlan,
        confirmation: &ConfirmedOperation,
        now_ms: u64,
    ) -> Result<(), TransactionFailure> {
        if confirmation.operation_id != plan.view.operation_id
            || confirmation.confirmed_at_ms < plan.view.created_at_ms
            || confirmation.confirmed_at_ms > plan.view.expires_at_ms
            || now_ms > plan.view.expires_at_ms
            || !plan
                .view
                .required_confirmations
                .iter()
                .all(|required| confirmation.confirmations.contains(required))
        {
            return Err(failure(
                plan,
                TransactionStage::Confirmation,
                "confirmation_missing_mismatched_or_expired",
            ));
        }
        Ok(())
    }

    fn validate_admission(
        &self,
        plan: &PreparedChangePlan,
        admission: &RuntimeAdmission,
        now_ms: u64,
    ) -> Result<(), TransactionFailure> {
        let status_allowed = if plan.view.intent == PlanIntent::Connect {
            admission.status == CompatibilityStatus::DetectedVerified
                && admission.status == plan.view.compatibility_evidence.status
                && plan
                    .view
                    .compatibility_expires_at_ms
                    .is_none_or(|expiry| expiry > now_ms)
        } else {
            admission.status == plan.view.compatibility_evidence.status
        };
        if admission.compatibility_sequence != plan.view.compatibility_sequence || !status_allowed {
            return Err(failure(
                plan,
                TransactionStage::Admission,
                "compatibility_changed_or_blocked",
            ));
        }
        Ok(())
    }

    fn consume(&self, plan: &PreparedChangePlan) -> Result<(), TransactionFailure> {
        let mut consumed = self.consumed.lock().map_err(|_| {
            failure(
                plan,
                TransactionStage::Confirmation,
                "operation_store_poisoned",
            )
        })?;
        if !consumed.insert(plan.view.operation_id.clone()) {
            return Err(failure(
                plan,
                TransactionStage::Confirmation,
                "operation_already_consumed",
            ));
        }
        Ok(())
    }

    fn recover_files(
        &self,
        plan: &PreparedChangePlan,
        primary_snapshot_id: &str,
        companion_snapshots: &[super::snapshot::SnapshotCreateResult],
        written_companions: usize,
        primary_written: bool,
        error: &mut TransactionFailure,
    ) {
        error.recovery = RecoveryStatus::RepairRequired;
        for index in (0..written_companions).rev() {
            let companion = &plan.companions[index];
            let snapshot_id = &companion_snapshots[index].record.snapshot_id;
            if self
                .restore_projection(
                    Path::new(&companion.target_path),
                    snapshot_id,
                    &companion.expected_after_hash,
                    &companion.before_hash,
                    !companion.target_existed,
                )
                .is_err()
            {
                error.recovery_reason_code =
                    Some("companion_restore_failed_or_target_changed".to_string());
                return;
            }
        }
        if !primary_written {
            error.recovery = RecoveryStatus::Restored;
            error.recovery_reason_code = None;
            return;
        }
        if self
            .restore_projection(
                Path::new(&plan.view.target_config_path),
                primary_snapshot_id,
                &plan.view.expected_after_hash,
                &plan.view.before_hash,
                !plan.view.target_existed,
            )
            .is_err()
        {
            error.recovery_reason_code = Some("restore_write_failed_or_target_changed".to_string());
            return;
        }
        error.recovery = RecoveryStatus::Restored;
        error.recovery_reason_code = None;
    }

    fn restore_projection(
        &self,
        target: &Path,
        snapshot_id: &str,
        expected_after_hash: &str,
        expected_before_hash: &str,
        normalize_new_owner: bool,
    ) -> Result<(), ()> {
        let snapshot = match self.snapshots.load(snapshot_id) {
            Ok(snapshot) => snapshot,
            Err(_) => return Err(()),
        };
        let result = if snapshot.record.original_existed {
            self.writer.replace(
                target,
                snapshot.exact_bytes.as_slice(),
                snapshot.record.original_permissions,
                snapshot.record.original_owner.as_deref(),
                expected_after_hash,
                normalize_new_owner,
            )
        } else {
            self.writer
                .remove(target, expected_after_hash, normalize_new_owner)
        };
        if result.is_err() {
            return Err(());
        }
        match read_config_source(target).and_then(|source| file_revision_hash(target, &source)) {
            Ok(hash) if hash == expected_before_hash => Ok(()),
            _ => Err(()),
        }
    }
}

fn failure(
    plan: &PreparedChangePlan,
    stage: TransactionStage,
    reason_code: &str,
) -> TransactionFailure {
    TransactionFailure {
        operation_id: plan.view.operation_id.clone(),
        stage,
        reason_code: reason_code.to_string(),
        recovery: RecoveryStatus::NotRequired,
        recovery_reason_code: None,
    }
}

fn atomic_reason(stage: AtomicWriteStage) -> &'static str {
    match stage {
        AtomicWriteStage::ParentCreate => "target_parent_create_failed",
        AtomicWriteStage::TempCreate => "target_temp_create_failed",
        AtomicWriteStage::TempWrite => "target_temp_write_failed",
        AtomicWriteStage::TempFlush => "target_temp_flush_failed",
        AtomicWriteStage::TempFsync => "target_temp_fsync_failed",
        AtomicWriteStage::Permission => "target_permission_restore_failed",
        AtomicWriteStage::PreReplaceRevision => "target_changed_before_replace",
        AtomicWriteStage::Replace => "target_atomic_replace_failed",
        AtomicWriteStage::DirectoryFsync => "target_directory_fsync_failed",
        AtomicWriteStage::Remove => "target_remove_failed",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::Arc;

    use serde_json::Value;
    use zeroize::Zeroizing;

    use super::*;
    use crate::agent_integration::connectors::{
        ClaudeCodeConnector, ClaudeDesktopConnector, ConnectInput, HermesConnector,
        OpenClawConnector,
    };
    use crate::agent_integration::ownership::FileOwnershipStore;
    use crate::agent_integration::plan::{
        build_connection_plan, build_disconnect_plan, build_snapshot_restore_plan,
        read_config_source, ConfigSource,
    };
    use crate::agent_integration::snapshot::{
        DecryptedSnapshot, FileSnapshotStore, MasterKeyStore, SnapshotCreateResult,
    };
    use crate::agent_integration::types::SnapshotRecord;
    use crate::agent_integration::types::{
        AllowedAction, BinarySource, CompatibilityDecision, Diagnostic, DiscoveryEvidence,
        DiscoveryRecord, DiscoverySource, Platform, ReasonCode,
    };

    struct TestKeys {
        fail: AtomicBool,
        value: [u8; 32],
    }

    struct FixedClock(u64);

    impl Clock for FixedClock {
        fn now_ms(&self) -> u64 {
            self.0
        }
    }

    static TEST_CLOCK: FixedClock = FixedClock(1_002);

    struct AtomicClock(AtomicU64);

    impl Clock for AtomicClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    struct AdvancingSnapshots<'a> {
        inner: &'a dyn SnapshotStore,
        clock: &'a AtomicClock,
        advance_to: u64,
    }

    impl SnapshotStore for AdvancingSnapshots<'_> {
        fn create(&self, request: SnapshotRequest<'_>) -> Result<SnapshotCreateResult, String> {
            let result = self.inner.create(request)?;
            self.clock.0.store(self.advance_to, Ordering::SeqCst);
            Ok(result)
        }

        fn load(&self, snapshot_id: &str) -> Result<DecryptedSnapshot, String> {
            self.inner.load(snapshot_id)
        }

        fn list(
            &self,
            agent_id: &str,
            target_config_path: &str,
        ) -> Result<Vec<SnapshotRecord>, String> {
            self.inner.list(agent_id, target_config_path)
        }

        fn set_pinned(&self, snapshot_id: &str, pinned: bool) -> Result<(), String> {
            self.inner.set_pinned(snapshot_id, pinned)
        }
    }

    impl TestKeys {
        fn available() -> Self {
            Self {
                fail: AtomicBool::new(false),
                value: [42_u8; 32],
            }
        }
    }

    impl MasterKeyStore for TestKeys {
        fn load_or_create(&self, _allow_create: bool) -> Result<Zeroizing<[u8; 32]>, String> {
            self.load()
        }

        fn load(&self) -> Result<Zeroizing<[u8; 32]>, String> {
            if self.fail.load(Ordering::SeqCst) {
                Err("keychain locked".to_string())
            } else {
                Ok(Zeroizing::new(self.value))
            }
        }
    }

    struct RejectVerifier;

    impl PostWriteVerifier for RejectVerifier {
        fn verify(&self, _path: &Path, _document: &ConfigDocument) -> Result<(), String> {
            Err("fixture secret must never be surfaced".to_string())
        }
    }

    struct FailOwnershipStore;

    impl OwnershipStore for FailOwnershipStore {
        fn load(&self, _key: &OwnershipKey) -> Result<Option<OwnershipRecord>, String> {
            Ok(None)
        }

        fn commit(
            &self,
            _record: OwnershipRecord,
            _expected_revision: Option<u64>,
        ) -> Result<OwnershipRecord, String> {
            Err("fixture commit failure".to_string())
        }

        fn remove(&self, _key: &OwnershipKey, _expected_revision: u64) -> Result<(), String> {
            Err("unused".to_string())
        }
    }

    struct FailBeforeReplaceWriter {
        stage: AtomicWriteStage,
    }

    impl AtomicConfigWriter for FailBeforeReplaceWriter {
        fn replace(
            &self,
            _target: &Path,
            _bytes: &[u8],
            _permissions: Option<u32>,
            _owner: Option<&str>,
            _expected_current_hash: &str,
            _normalize_new_owner: bool,
        ) -> Result<(), AtomicWriteFailure> {
            Err(AtomicWriteFailure {
                stage: self.stage,
                target_replaced: false,
            })
        }

        fn remove(
            &self,
            _target: &Path,
            _expected_current_hash: &str,
            _normalize_new_owner: bool,
        ) -> Result<(), AtomicWriteFailure> {
            unreachable!()
        }
    }

    struct FailRecoveryWriter {
        calls: AtomicUsize,
    }

    impl AtomicConfigWriter for FailRecoveryWriter {
        fn replace(
            &self,
            target: &Path,
            bytes: &[u8],
            permissions: Option<u32>,
            owner: Option<&str>,
            expected_current_hash: &str,
            normalize_new_owner: bool,
        ) -> Result<(), AtomicWriteFailure> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                FsAtomicConfigWriter.replace(
                    target,
                    bytes,
                    permissions,
                    owner,
                    expected_current_hash,
                    normalize_new_owner,
                )
            } else {
                Err(AtomicWriteFailure {
                    stage: AtomicWriteStage::Permission,
                    target_replaced: false,
                })
            }
        }

        fn remove(
            &self,
            _target: &Path,
            _expected_current_hash: &str,
            _normalize_new_owner: bool,
        ) -> Result<(), AtomicWriteFailure> {
            Err(AtomicWriteFailure {
                stage: AtomicWriteStage::Remove,
                target_replaced: false,
            })
        }
    }

    fn scratch(label: &str) -> PathBuf {
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random).unwrap();
        std::env::temp_dir().join(format!(
            "token-station-transaction-{label}-{}-{}",
            std::process::id(),
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ))
    }

    fn discovery(target: &Path) -> DiscoveryRecord {
        DiscoveryRecord {
            agent_id: "claude-code".to_string(),
            executable_path: "/opt/claude".to_string(),
            canonical_path: "/opt/claude".to_string(),
            binary_source: BinarySource::Path,
            modified_at_ms: None,
            binary_sha256: None,
            upgrade_command: None,
            version_raw: Some("2.1.211".to_string()),
            version_normalized: Some("2.1.211".to_string()),
            environment: Platform::Macos,
            evidence: vec![DiscoveryEvidence {
                source: DiscoverySource::Path,
                observed_path: "/opt/claude".to_string(),
                is_path_default: true,
            }],
            is_path_default: true,
            runnable: true,
            config_candidates: vec![target.to_str().unwrap().to_string()],
            config_fingerprint: None,
            conflict_group: None,
            diagnostics: Vec::<Diagnostic>::new(),
            scanned_at_ms: 1,
        }
    }

    fn verified() -> CompatibilityDecision {
        CompatibilityDecision {
            agent_id: "claude-code".to_string(),
            installation_path: Some("/opt/claude".to_string()),
            status: CompatibilityStatus::DetectedVerified,
            reason_code: ReasonCode::DefaultAdmission,
            message: "admitted".to_string(),
            matched_catalog_version: Some("fixture".to_string()),
            connector_id: Some("claude-code-v1".to_string()),
            allowed_actions: BTreeSet::from([AllowedAction::PreviewConnect]),
        }
    }

    fn claude_desktop_discovery(target: &Path) -> DiscoveryRecord {
        let mut record = discovery(target);
        record.agent_id = "claude-desktop".to_string();
        record.executable_path = "/Applications/Claude.app/Contents/MacOS/Claude".to_string();
        record.canonical_path = record.executable_path.clone();
        record
    }

    fn claude_desktop_verified() -> CompatibilityDecision {
        let mut decision = verified();
        decision.agent_id = "claude-desktop".to_string();
        decision.installation_path =
            Some("/Applications/Claude.app/Contents/MacOS/Claude".to_string());
        decision.connector_id = Some("claude-desktop-3p-v1".to_string());
        decision
    }

    fn prepare_claude_desktop(target: &Path, secret: &str) -> PreparedChangePlan {
        build_connection_plan(
            &ClaudeDesktopConnector,
            &claude_desktop_discovery(target),
            &claude_desktop_verified(),
            target,
            &read_config_source(target).unwrap(),
            &ConnectInput {
                base_url: "http://127.0.0.1:8787/agents/claude-desktop",
                token: Some(secret),
                adapter_ready: true,
            },
            1,
            None,
            1_000,
            "0d".repeat(16),
        )
        .unwrap()
    }

    fn openclaw_discovery(target: &Path) -> DiscoveryRecord {
        DiscoveryRecord {
            agent_id: "openclaw".to_string(),
            executable_path: "/opt/openclaw".to_string(),
            canonical_path: "/opt/openclaw".to_string(),
            binary_source: BinarySource::Path,
            modified_at_ms: None,
            binary_sha256: None,
            upgrade_command: None,
            version_raw: Some("2026.6.11".to_string()),
            version_normalized: Some("2026.6.11".to_string()),
            environment: Platform::Macos,
            evidence: vec![DiscoveryEvidence {
                source: DiscoverySource::Path,
                observed_path: "/opt/openclaw".to_string(),
                is_path_default: true,
            }],
            is_path_default: true,
            runnable: true,
            config_candidates: vec![target.to_str().unwrap().to_string()],
            config_fingerprint: None,
            conflict_group: None,
            diagnostics: Vec::new(),
            scanned_at_ms: 1,
        }
    }

    fn openclaw_verified() -> CompatibilityDecision {
        CompatibilityDecision {
            agent_id: "openclaw".to_string(),
            installation_path: Some("/opt/openclaw".to_string()),
            status: CompatibilityStatus::DetectedVerified,
            reason_code: ReasonCode::DefaultAdmission,
            message: "admitted".to_string(),
            matched_catalog_version: Some("fixture".to_string()),
            connector_id: Some("openclaw-v1".to_string()),
            allowed_actions: BTreeSet::from([AllowedAction::PreviewConnect]),
        }
    }

    fn hermes_discovery(target: &Path) -> DiscoveryRecord {
        DiscoveryRecord {
            agent_id: "nous-hermes-agent".to_string(),
            executable_path: "/opt/hermes".to_string(),
            canonical_path: "/opt/hermes".to_string(),
            binary_source: BinarySource::Path,
            modified_at_ms: None,
            binary_sha256: None,
            upgrade_command: None,
            version_raw: Some("Hermes Agent v0.18.0 (2026.7.1)".to_string()),
            version_normalized: Some("0.18.0".to_string()),
            environment: Platform::Macos,
            evidence: vec![DiscoveryEvidence {
                source: DiscoverySource::Path,
                observed_path: "/opt/hermes".to_string(),
                is_path_default: true,
            }],
            is_path_default: true,
            runnable: true,
            config_candidates: vec![target.to_str().unwrap().to_string()],
            config_fingerprint: None,
            conflict_group: None,
            diagnostics: Vec::new(),
            scanned_at_ms: 1,
        }
    }

    fn hermes_verified() -> CompatibilityDecision {
        CompatibilityDecision {
            agent_id: "nous-hermes-agent".to_string(),
            installation_path: Some("/opt/hermes".to_string()),
            status: CompatibilityStatus::DetectedVerified,
            reason_code: ReasonCode::DefaultAdmission,
            message: "admitted".to_string(),
            matched_catalog_version: Some("fixture".to_string()),
            connector_id: Some("hermes-v1".to_string()),
            allowed_actions: BTreeSet::from([AllowedAction::PreviewConnect]),
        }
    }

    fn prepare_hermes(target: &Path, secret: &str) -> PreparedChangePlan {
        let source = read_config_source(target).unwrap();
        build_connection_plan(
            &HermesConnector,
            &hermes_discovery(target),
            &hermes_verified(),
            target,
            &source,
            &ConnectInput {
                base_url: "http://127.0.0.1:8787/v1",
                token: Some(secret),
                adapter_ready: true,
            },
            1,
            None,
            1_000,
            "78".repeat(16),
        )
        .unwrap()
    }

    fn prepare_openclaw(target: &Path, secret: &str) -> PreparedChangePlan {
        let source = read_config_source(target).unwrap();
        build_connection_plan(
            &OpenClawConnector,
            &openclaw_discovery(target),
            &openclaw_verified(),
            target,
            &source,
            &ConnectInput {
                base_url: "http://127.0.0.1:8787/v1",
                token: Some(secret),
                adapter_ready: true,
            },
            1,
            None,
            1_000,
            "45".repeat(16),
        )
        .unwrap()
    }

    fn prepare(target: &Path, secret: &str) -> PreparedChangePlan {
        let source = read_config_source(target).unwrap();
        build_connection_plan(
            &ClaudeCodeConnector,
            &discovery(target),
            &verified(),
            target,
            &source,
            &ConnectInput {
                base_url: "http://127.0.0.1:8787",
                token: Some(secret),
                adapter_ready: true,
            },
            1,
            None,
            1_000,
            "ab".repeat(16),
        )
        .unwrap()
    }

    fn confirmation(plan: &PreparedChangePlan) -> ConfirmedOperation {
        confirmation_at(plan, 1_001)
    }

    fn confirmation_at(plan: &PreparedChangePlan, confirmed_at_ms: u64) -> ConfirmedOperation {
        ConfirmedOperation {
            operation_id: plan.view.operation_id.clone(),
            confirmed_at_ms,
            confirmations: plan.view.required_confirmations.iter().copied().collect(),
        }
    }

    fn admission() -> RuntimeAdmission {
        RuntimeAdmission {
            compatibility_sequence: 1,
            status: CompatibilityStatus::DetectedVerified,
        }
    }

    fn write_initial(target: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o640)).unwrap();
        }
    }

    #[test]
    fn transaction_rejects_non_connectable_runtime_admission_before_writing() {
        let root = scratch("runtime-admission");
        let target = root.join("settings.json");
        let initial = br#"{"unowned":"keep"}"#;
        write_initial(&target, initial);
        let plan = prepare(&target, "vk-admission");
        let keys = Arc::new(TestKeys::available());
        let snapshots = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
        let ownership = FileOwnershipStore::new(root.join("ownership"));
        let engine = TransactionEngine::new(
            &snapshots,
            &ownership,
            keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &FixedClock(1_001),
        );
        let unknown = RuntimeAdmission {
            compatibility_sequence: 1,
            status: CompatibilityStatus::DetectedUnknown,
        };

        let failure = engine
            .apply_connection(&plan, &confirmation(&plan), &unknown, 1_001)
            .expect_err("a non-connectable status must fail before writing");
        assert_eq!(failure.stage, TransactionStage::Admission);
        assert_eq!(std::fs::read(&target).unwrap(), initial);
        assert!(snapshots.list_agent("claude-code").unwrap().is_empty());

        let blocked = RuntimeAdmission {
            compatibility_sequence: 1,
            status: CompatibilityStatus::DetectedBlocked,
        };
        let failure = engine
            .apply_connection(&plan, &confirmation(&plan), &blocked, 1_001)
            .expect_err("a newly blocked version must fail before writing");
        assert_eq!(failure.stage, TransactionStage::Admission);
        assert_eq!(std::fs::read(&target).unwrap(), initial);
        assert!(snapshots.list_agent("claude-code").unwrap().is_empty());

        engine
            .apply_connection(
                &plan,
                &confirmation(&plan),
                &RuntimeAdmission {
                    compatibility_sequence: 1,
                    status: CompatibilityStatus::DetectedVerified,
                },
                1_001,
            )
            .expect("the single connectable status may proceed");
        assert_ne!(std::fs::read(&target).unwrap(), initial);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn transaction_requires_confirmation_and_revision_before_snapshot_or_write() {
        let root = scratch("gates");
        let target = root.join("settings.json");
        let initial = br#"{"unowned":"keep"}"#;
        write_initial(&target, initial);
        let plan = prepare(&target, "vk-gate-secret");
        let keys = Arc::new(TestKeys::available());
        let snapshots = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
        let ownership = FileOwnershipStore::new(root.join("ownership"));
        let engine = TransactionEngine::new(
            &snapshots,
            &ownership,
            keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &TEST_CLOCK,
        );
        let missing = ConfirmedOperation {
            operation_id: plan.view.operation_id.clone(),
            confirmed_at_ms: 1_001,
            confirmations: BTreeSet::new(),
        };
        let error = engine
            .apply_connection(&plan, &missing, &admission(), 1_002)
            .expect_err("missing confirmation is rejected");
        assert_eq!(error.stage, TransactionStage::Confirmation);
        assert_eq!(std::fs::read(&target).unwrap(), initial);
        assert!(!snapshots.root().exists());

        std::fs::write(&target, br#"{"unowned":"changed"}"#).unwrap();
        let error = engine
            .apply_connection(&plan, &confirmation(&plan), &admission(), 1_003)
            .expect_err("changed source is rejected");
        assert_eq!(error.stage, TransactionStage::Revision);
        assert!(!snapshots.root().exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn transaction_success_creates_encrypted_snapshot_and_ownership_then_rejects_replay() {
        let root = scratch("success");
        let target = root.join("settings.json");
        let marker = "unowned-marker";
        let secret = "vk-transaction-secret";
        write_initial(&target, format!("{{\"unowned\":\"{marker}\"}}").as_bytes());
        let legacy_backup = target.with_extension("json.token-station.bak");
        let legacy_marker = b"legacy-backup-must-remain-byte-identical";
        std::fs::write(&legacy_backup, legacy_marker).unwrap();
        let plan = prepare(&target, secret);
        let keys = Arc::new(TestKeys::available());
        let snapshots = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
        let ownership = FileOwnershipStore::new(root.join("ownership"));
        let engine = TransactionEngine::new(
            &snapshots,
            &ownership,
            keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &TEST_CLOCK,
        );
        let outcome = engine
            .apply_connection(&plan, &confirmation(&plan), &admission(), 1_002)
            .unwrap();
        let written = std::fs::read_to_string(&target).unwrap();
        assert!(written.contains(marker));
        assert!(written.contains(secret));
        let ownership_key = OwnershipKey {
            agent_id: "claude-code".to_string(),
            installation_path: "/opt/claude".to_string(),
            target_config_path: target.to_str().unwrap().to_string(),
        };
        assert_eq!(ownership.load(&ownership_key).unwrap().unwrap().revision, 1);
        assert_eq!(outcome.ownership_revision, 1);
        let serialized = serde_json::to_string(&outcome).unwrap();
        assert!(!serialized.contains(secret));
        let replay = engine
            .apply_connection(&plan, &confirmation(&plan), &admission(), 1_003)
            .expect_err("replay is rejected");
        assert!(matches!(
            replay.stage,
            TransactionStage::Ownership | TransactionStage::Confirmation
        ));
        assert_eq!(std::fs::read(&legacy_backup).unwrap(), legacy_marker);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn claude_desktop_profile_and_meta_commit_together_and_recover_together() {
        let root = scratch("claude-desktop-bundle");
        let target = root.join("7f60d1f4-8d8c-4f5c-9f4c-2c2530c4f9f2.json");
        let meta = root.join("_meta.json");
        let original_profile = br#"{"unknownProfile":"keep"}"#;
        let original_meta = br#"{"unknownMeta":"keep","entries":[{"id":"existing","name":"Existing"}],"appliedId":"existing"}"#;
        write_initial(&target, original_profile);
        write_initial(&meta, original_meta);
        let plan = prepare_claude_desktop(&target, "vk-claude-desktop-secret");
        assert_eq!(plan.companions.len(), 1);
        assert_eq!(plan.view.related_config_paths, [meta.to_string_lossy()]);
        let keys = Arc::new(TestKeys::available());
        let snapshots = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
        let ownership = FileOwnershipStore::new(root.join("ownership"));
        let engine = TransactionEngine::new(
            &snapshots,
            &ownership,
            keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &TEST_CLOCK,
        );

        engine
            .apply_connection(&plan, &confirmation(&plan), &admission(), 1_002)
            .unwrap();

        let profile: Value = serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        let metadata: Value = serde_json::from_slice(&std::fs::read(&meta).unwrap()).unwrap();
        assert_eq!(profile["unknownProfile"], "keep");
        assert_eq!(profile["inferenceProvider"], "gateway");
        assert_eq!(metadata["unknownMeta"], "keep");
        assert_eq!(
            metadata["appliedId"],
            "7f60d1f4-8d8c-4f5c-9f4c-2c2530c4f9f2"
        );
        assert!(metadata["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == "existing"));
        let ownership_key = OwnershipKey {
            agent_id: "claude-desktop".to_string(),
            installation_path: "/Applications/Claude.app/Contents/MacOS/Claude".to_string(),
            target_config_path: target.to_string_lossy().into_owned(),
        };
        let owned = ownership.load(&ownership_key).unwrap().unwrap();
        assert_eq!(owned.companion_files.len(), 1);
        let baseline = snapshots.load(&owned.baseline_snapshot_id).unwrap();
        let baseline_source = ConfigSource {
            existed: baseline.record.original_existed,
            exact_bytes: baseline.exact_bytes,
            original_permissions: baseline.record.original_permissions,
            original_owner: baseline.record.original_owner.clone(),
        };
        let current = read_config_source(&target).unwrap();
        let master_key = keys.load().unwrap();
        let mut restore = build_snapshot_restore_plan(
            &ClaudeDesktopConnector,
            &claude_desktop_discovery(&target),
            &claude_desktop_verified(),
            &target,
            &current,
            &owned,
            &baseline.record,
            &baseline_source,
            &master_key,
            1,
            None,
            1_003,
            "0f".repeat(16),
        )
        .unwrap();
        crate::agent_integration::plan::attach_restore_companions(
            &mut restore,
            &ClaudeDesktopConnector,
            &owned,
            &baseline.record,
            &snapshots,
            &master_key,
        )
        .unwrap();
        let restore_confirmation = ConfirmedOperation {
            operation_id: restore.view.operation_id.clone(),
            confirmed_at_ms: 1_003,
            confirmations: restore
                .view
                .required_confirmations
                .iter()
                .copied()
                .collect(),
        };
        engine
            .apply_snapshot_restore(&restore, &restore_confirmation, &admission(), 1_004)
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(&target).unwrap()).unwrap(),
            serde_json::from_slice::<Value>(original_profile).unwrap()
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(&meta).unwrap()).unwrap(),
            serde_json::from_slice::<Value>(original_meta).unwrap()
        );

        let owned = ownership.load(&ownership_key).unwrap().unwrap();
        let baseline = snapshots.load(&owned.baseline_snapshot_id).unwrap();
        let baseline_source = ConfigSource {
            existed: baseline.record.original_existed,
            exact_bytes: baseline.exact_bytes,
            original_permissions: baseline.record.original_permissions,
            original_owner: baseline.record.original_owner.clone(),
        };
        let current = read_config_source(&target).unwrap();
        let mut disconnect = build_disconnect_plan(
            &ClaudeDesktopConnector,
            &claude_desktop_discovery(&target),
            &claude_desktop_verified(),
            &target,
            &current,
            &owned,
            &baseline.record,
            &baseline_source,
            &master_key,
            1,
            None,
            1_005,
            "0e".repeat(16),
        )
        .unwrap();
        crate::agent_integration::plan::attach_disconnect_companions(
            &mut disconnect,
            &ClaudeDesktopConnector,
            &owned,
            &snapshots,
            &master_key,
        )
        .unwrap();
        let disconnect_confirmation = ConfirmedOperation {
            operation_id: disconnect.view.operation_id.clone(),
            confirmed_at_ms: 1_005,
            confirmations: disconnect
                .view
                .required_confirmations
                .iter()
                .copied()
                .collect(),
        };
        engine
            .apply_disconnect(&disconnect, &disconnect_confirmation, &admission(), 1_006)
            .unwrap();
        let restored_profile: Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        let restored_meta: Value = serde_json::from_slice(&std::fs::read(&meta).unwrap()).unwrap();
        assert_eq!(
            restored_profile,
            serde_json::from_slice::<Value>(original_profile).unwrap()
        );
        assert_eq!(
            restored_meta,
            serde_json::from_slice::<Value>(original_meta).unwrap()
        );
        assert!(ownership.load(&ownership_key).unwrap().is_none());

        std::fs::remove_dir_all(root).ok();

        let rollback_root = scratch("claude-desktop-bundle-rollback");
        let rollback_target = rollback_root.join("7f60d1f4-8d8c-4f5c-9f4c-2c2530c4f9f2.json");
        let rollback_meta = rollback_root.join("_meta.json");
        write_initial(&rollback_target, original_profile);
        write_initial(&rollback_meta, original_meta);
        let rollback_plan = prepare_claude_desktop(&rollback_target, "vk-claude-desktop-rollback");
        let rollback_keys = Arc::new(TestKeys::available());
        let rollback_snapshots =
            FileSnapshotStore::new(rollback_root.join("snapshots"), rollback_keys.clone());
        let rollback_ownership = FileOwnershipStore::new(rollback_root.join("ownership"));
        let rollback_engine = TransactionEngine::new(
            &rollback_snapshots,
            &rollback_ownership,
            rollback_keys.as_ref(),
            &FsAtomicConfigWriter,
            &RejectVerifier,
            &TEST_CLOCK,
        );
        let failure = rollback_engine
            .apply_connection(
                &rollback_plan,
                &confirmation(&rollback_plan),
                &admission(),
                1_002,
            )
            .expect_err("post-write failure rolls both files back");
        assert_eq!(failure.recovery, RecoveryStatus::Restored);
        assert_eq!(std::fs::read(&rollback_target).unwrap(), original_profile);
        assert_eq!(std::fs::read(&rollback_meta).unwrap(), original_meta);
        std::fs::remove_dir_all(rollback_root).ok();
    }

    #[test]
    fn transaction_disconnect_restores_only_owned_paths_and_preserves_later_user_fields() {
        let root = scratch("disconnect");
        let target = root.join("settings.json");
        let initial = br#"{"unowned":"keep","env":{"USER_VALUE":"original"}}"#;
        write_initial(&target, initial);
        let connect = prepare(&target, "vk-disconnect-secret");
        let keys = Arc::new(TestKeys::available());
        let snapshots = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
        let ownership_store = FileOwnershipStore::new(root.join("ownership"));
        let engine = TransactionEngine::new(
            &snapshots,
            &ownership_store,
            keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &TEST_CLOCK,
        );
        engine
            .apply_connection(&connect, &confirmation(&connect), &admission(), 1_002)
            .unwrap();

        let mut current_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        current_json["later_user_field"] = serde_json::json!({ "enabled": true });
        std::fs::write(&target, serde_json::to_vec_pretty(&current_json).unwrap()).unwrap();
        let ownership_key = OwnershipKey {
            agent_id: "claude-code".to_string(),
            installation_path: "/opt/claude".to_string(),
            target_config_path: target.to_str().unwrap().to_string(),
        };
        let ownership = ownership_store.load(&ownership_key).unwrap().unwrap();
        let baseline = snapshots.load(&ownership.baseline_snapshot_id).unwrap();
        let baseline_source = ConfigSource {
            existed: baseline.record.original_existed,
            exact_bytes: baseline.exact_bytes,
            original_permissions: baseline.record.original_permissions,
            original_owner: baseline.record.original_owner.clone(),
        };
        let current = read_config_source(&target).unwrap();
        let disconnect = build_disconnect_plan(
            &ClaudeCodeConnector,
            &discovery(&target),
            &verified(),
            &target,
            &current,
            &ownership,
            &baseline.record,
            &baseline_source,
            &keys.load().unwrap(),
            1,
            None,
            2_000,
            "cd".repeat(16),
        )
        .unwrap();
        let outcome = engine
            .apply_disconnect(
                &disconnect,
                &confirmation_at(&disconnect, 2_001),
                &admission(),
                2_002,
            )
            .unwrap();
        assert_eq!(outcome.ownership_revision, 0);
        let restored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(restored["unowned"], "keep");
        assert_eq!(restored["later_user_field"]["enabled"], true);
        assert_eq!(restored["env"]["USER_VALUE"], "original");
        assert!(restored["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
        assert!(ownership_store.load(&ownership_key).unwrap().is_none());
        let records = snapshots
            .list("claude-code", target.to_str().unwrap())
            .unwrap();
        assert!(records.iter().all(|record| !record.pinned));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn transaction_disconnect_accepts_owned_paths_already_at_baseline() {
        let root = scratch("disconnect-already-restored");
        let target = root.join("settings.json");
        write_initial(&target, br#"{"unowned":"keep"}"#);
        let connect = prepare(&target, "vk-disconnect-already-restored");
        let keys = Arc::new(TestKeys::available());
        let snapshots = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
        let ownership_store = FileOwnershipStore::new(root.join("ownership"));
        let engine = TransactionEngine::new(
            &snapshots,
            &ownership_store,
            keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &TEST_CLOCK,
        );
        engine
            .apply_connection(&connect, &confirmation(&connect), &admission(), 1_002)
            .unwrap();

        let ownership_key = OwnershipKey {
            agent_id: "claude-code".to_string(),
            installation_path: "/opt/claude".to_string(),
            target_config_path: target.to_str().unwrap().to_string(),
        };
        let ownership = ownership_store.load(&ownership_key).unwrap().unwrap();
        let baseline = snapshots.load(&ownership.baseline_snapshot_id).unwrap();
        let baseline_source = ConfigSource {
            existed: baseline.record.original_existed,
            exact_bytes: baseline.exact_bytes,
            original_permissions: baseline.record.original_permissions,
            original_owner: baseline.record.original_owner.clone(),
        };

        // An external tool removed Token Station managed fields but kept the ownership record.
        // This matches the baseline state on managed paths. Permit an idempotent disconnect and clear ownership.
        std::fs::write(
            &target,
            br#"{"unowned":"keep","later_user_field":{"enabled":true}}"#,
        )
        .unwrap();
        let current = read_config_source(&target).unwrap();
        let disconnect = build_disconnect_plan(
            &ClaudeCodeConnector,
            &discovery(&target),
            &verified(),
            &target,
            &current,
            &ownership,
            &baseline.record,
            &baseline_source,
            &keys.load().unwrap(),
            1,
            None,
            2_000,
            "ef".repeat(16),
        )
        .expect("owned paths already matching the baseline should be recoverable");

        let outcome = engine
            .apply_disconnect(
                &disconnect,
                &confirmation_at(&disconnect, 2_001),
                &admission(),
                2_002,
            )
            .unwrap();
        assert_eq!(outcome.ownership_revision, 0);
        let restored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(restored["unowned"], "keep");
        assert_eq!(restored["later_user_field"]["enabled"], true);
        assert!(ownership_store.load(&ownership_key).unwrap().is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn transaction_snapshot_restore_updates_ownership_without_replacing_unowned_fields() {
        let root = scratch("restore");
        let target = root.join("settings.json");
        write_initial(&target, br#"{"unowned":"baseline"}"#);
        let connect = prepare(&target, "vk-restore-secret");
        let keys = Arc::new(TestKeys::available());
        let snapshots = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
        let ownership_store = FileOwnershipStore::new(root.join("ownership"));
        let engine = TransactionEngine::new(
            &snapshots,
            &ownership_store,
            keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &TEST_CLOCK,
        );
        engine
            .apply_connection(&connect, &confirmation(&connect), &admission(), 1_002)
            .unwrap();
        let mut current_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        current_json["unowned"] = serde_json::json!("changed-after-connect");
        current_json["plugin"] = serde_json::json!({ "enabled": true });
        std::fs::write(&target, serde_json::to_vec_pretty(&current_json).unwrap()).unwrap();

        let ownership_key = OwnershipKey {
            agent_id: "claude-code".to_string(),
            installation_path: "/opt/claude".to_string(),
            target_config_path: target.to_str().unwrap().to_string(),
        };
        let ownership = ownership_store.load(&ownership_key).unwrap().unwrap();
        let baseline = snapshots.load(&ownership.baseline_snapshot_id).unwrap();
        let baseline_source = ConfigSource {
            existed: baseline.record.original_existed,
            exact_bytes: baseline.exact_bytes,
            original_permissions: baseline.record.original_permissions,
            original_owner: baseline.record.original_owner.clone(),
        };
        let current = read_config_source(&target).unwrap();
        let restore = build_snapshot_restore_plan(
            &ClaudeCodeConnector,
            &discovery(&target),
            &verified(),
            &target,
            &current,
            &ownership,
            &baseline.record,
            &baseline_source,
            &keys.load().unwrap(),
            1,
            None,
            3_000,
            "ef".repeat(16),
        )
        .unwrap();
        let outcome = engine
            .apply_snapshot_restore(
                &restore,
                &confirmation_at(&restore, 3_001),
                &admission(),
                3_002,
            )
            .unwrap();
        assert_eq!(outcome.ownership_revision, 2);
        let restored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(restored["unowned"], "changed-after-connect");
        assert_eq!(restored["plugin"]["enabled"], true);
        assert!(restored["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
        let updated = ownership_store.load(&ownership_key).unwrap().unwrap();
        assert_eq!(updated.revision, 2);
        assert_eq!(updated.baseline_snapshot_id, ownership.baseline_snapshot_id);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn openclaw_transaction_connect_restore_disconnect_preserves_json5_comments() {
        let root = scratch("openclaw-round-trip");
        let target = root.join("openclaw.json");
        let initial = include_bytes!("../../tests/fixtures/config/openclaw/openclaw.input.json5");
        write_initial(&target, initial);
        let connect = prepare_openclaw(&target, "vk-openclaw-secret");
        let keys = Arc::new(TestKeys::available());
        let snapshots = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
        let ownership_store = FileOwnershipStore::new(root.join("ownership"));
        let engine = TransactionEngine::new(
            &snapshots,
            &ownership_store,
            keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &TEST_CLOCK,
        );
        engine
            .apply_connection(&connect, &confirmation(&connect), &admission(), 1_002)
            .unwrap();
        let connected = std::fs::read_to_string(&target).unwrap();
        assert!(connected.contains("keep this comment"), "{connected}");
        assert!(connected.contains("vk-openclaw-secret"));
        let replay = engine
            .apply_connection(&connect, &confirmation(&connect), &admission(), 1_003)
            .expect_err("OpenClaw connect plan is one-shot");
        assert!(matches!(
            replay.stage,
            TransactionStage::Ownership | TransactionStage::Confirmation
        ));

        let key = OwnershipKey {
            agent_id: "openclaw".to_string(),
            installation_path: "/opt/openclaw".to_string(),
            target_config_path: target.to_str().unwrap().to_string(),
        };
        let ownership = ownership_store.load(&key).unwrap().unwrap();
        let baseline = snapshots.load(&ownership.baseline_snapshot_id).unwrap();
        let baseline_source = ConfigSource {
            existed: baseline.record.original_existed,
            exact_bytes: baseline.exact_bytes,
            original_permissions: baseline.record.original_permissions,
            original_owner: baseline.record.original_owner.clone(),
        };
        let current = read_config_source(&target).unwrap();
        let restore = build_snapshot_restore_plan(
            &OpenClawConnector,
            &openclaw_discovery(&target),
            &openclaw_verified(),
            &target,
            &current,
            &ownership,
            &baseline.record,
            &baseline_source,
            &keys.load().unwrap(),
            1,
            None,
            2_000,
            "56".repeat(16),
        )
        .unwrap();
        engine
            .apply_snapshot_restore(
                &restore,
                &confirmation_at(&restore, 2_001),
                &admission(),
                2_002,
            )
            .unwrap();
        let restored = std::fs::read_to_string(&target).unwrap();
        assert!(restored.contains("keep this comment"), "{restored}");
        assert!(restored.contains("existing.invalid"), "{restored}");
        assert!(!restored.contains("vk-openclaw-secret"));

        let ownership = ownership_store.load(&key).unwrap().unwrap();
        let current = read_config_source(&target).unwrap();
        let disconnect = build_disconnect_plan(
            &OpenClawConnector,
            &openclaw_discovery(&target),
            &openclaw_verified(),
            &target,
            &current,
            &ownership,
            &baseline.record,
            &baseline_source,
            &keys.load().unwrap(),
            1,
            None,
            3_000,
            "67".repeat(16),
        )
        .unwrap();
        engine
            .apply_disconnect(
                &disconnect,
                &confirmation_at(&disconnect, 3_001),
                &admission(),
                3_002,
            )
            .unwrap();
        assert!(ownership_store.load(&key).unwrap().is_none());
        assert!(std::fs::read_to_string(&target)
            .unwrap()
            .contains("keep this comment"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn hermes_transaction_connect_restore_disconnect_preserves_yaml_comments() {
        let root = scratch("hermes-round-trip");
        let target = root.join("config.yaml");
        let initial = include_bytes!("../../tests/fixtures/config/hermes/config.input.yaml");
        write_initial(&target, initial);
        let connect = prepare_hermes(&target, "vk-hermes-secret");
        assert!(!format!("{:?}", connect.view).contains("vk-hermes-secret"));
        let keys = Arc::new(TestKeys::available());
        let snapshots = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
        let ownership_store = FileOwnershipStore::new(root.join("ownership"));
        let engine = TransactionEngine::new(
            &snapshots,
            &ownership_store,
            keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &TEST_CLOCK,
        );
        engine
            .apply_connection(&connect, &confirmation(&connect), &admission(), 1_002)
            .unwrap();
        let connected = std::fs::read_to_string(&target).unwrap();
        assert!(
            connected.contains("# keep Hermes root comment"),
            "{connected}"
        );
        assert!(
            connected.contains("unknown_model_setting: keep-me"),
            "{connected}"
        );
        assert!(connected.contains("vk-hermes-secret"));

        let key = OwnershipKey {
            agent_id: "nous-hermes-agent".to_string(),
            installation_path: "/opt/hermes".to_string(),
            target_config_path: target.to_str().unwrap().to_string(),
        };
        let ownership = ownership_store.load(&key).unwrap().unwrap();
        let baseline = snapshots.load(&ownership.baseline_snapshot_id).unwrap();
        for entry in std::fs::read_dir(root.join("snapshots")).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                let persisted = std::fs::read(&path).unwrap();
                assert!(!String::from_utf8_lossy(&persisted).contains("original-fixture-key"));
            }
        }
        let baseline_source = ConfigSource {
            existed: baseline.record.original_existed,
            exact_bytes: baseline.exact_bytes,
            original_permissions: baseline.record.original_permissions,
            original_owner: baseline.record.original_owner.clone(),
        };
        let current = read_config_source(&target).unwrap();
        let restore = build_snapshot_restore_plan(
            &HermesConnector,
            &hermes_discovery(&target),
            &hermes_verified(),
            &target,
            &current,
            &ownership,
            &baseline.record,
            &baseline_source,
            &keys.load().unwrap(),
            1,
            None,
            2_000,
            "89".repeat(16),
        )
        .unwrap();
        engine
            .apply_snapshot_restore(
                &restore,
                &confirmation_at(&restore, 2_001),
                &admission(),
                2_002,
            )
            .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), initial);

        let ownership = ownership_store.load(&key).unwrap().unwrap();
        let current = read_config_source(&target).unwrap();
        let disconnect = build_disconnect_plan(
            &HermesConnector,
            &hermes_discovery(&target),
            &hermes_verified(),
            &target,
            &current,
            &ownership,
            &baseline.record,
            &baseline_source,
            &keys.load().unwrap(),
            1,
            None,
            3_000,
            "9a".repeat(16),
        )
        .unwrap();
        engine
            .apply_disconnect(
                &disconnect,
                &confirmation_at(&disconnect, 3_001),
                &admission(),
                3_002,
            )
            .unwrap();
        assert!(ownership_store.load(&key).unwrap().is_none());
        assert_eq!(std::fs::read(&target).unwrap(), initial);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn transaction_snapshot_and_pre_replace_failures_leave_target_untouched() {
        let root = scratch("no-write-failures");
        let target = root.join("settings.json");
        let initial = br#"{"unowned":"keep"}"#;
        write_initial(&target, initial);
        let plan = prepare(&target, "vk-failure-secret");
        let keys = Arc::new(TestKeys::available());
        keys.fail.store(true, Ordering::SeqCst);
        let snapshots = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
        let ownership = FileOwnershipStore::new(root.join("ownership"));
        let engine = TransactionEngine::new(
            &snapshots,
            &ownership,
            keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &TEST_CLOCK,
        );
        let error = engine
            .apply_connection(&plan, &confirmation(&plan), &admission(), 1_002)
            .expect_err("key failure is rejected");
        assert_eq!(error.stage, TransactionStage::Snapshot);
        assert_eq!(std::fs::read(&target).unwrap(), initial);

        for stage in [
            AtomicWriteStage::TempCreate,
            AtomicWriteStage::TempFsync,
            AtomicWriteStage::Replace,
        ] {
            let root2 = scratch("writer-failure");
            let target2 = root2.join("settings.json");
            write_initial(&target2, initial);
            let plan2 = prepare(&target2, "vk-writer-secret");
            let keys2 = Arc::new(TestKeys::available());
            let snapshots2 = FileSnapshotStore::new(root2.join("snapshots"), keys2.clone());
            let ownership2 = FileOwnershipStore::new(root2.join("ownership"));
            let writer = FailBeforeReplaceWriter { stage };
            let engine2 = TransactionEngine::new(
                &snapshots2,
                &ownership2,
                keys2.as_ref(),
                &writer,
                &ParseOnlyVerifier,
                &TEST_CLOCK,
            );
            let error = engine2
                .apply_connection(&plan2, &confirmation(&plan2), &admission(), 1_002)
                .expect_err("writer failure is rejected");
            assert_eq!(error.recovery, RecoveryStatus::NotRequired, "{stage:?}");
            assert_eq!(std::fs::read(&target2).unwrap(), initial, "{stage:?}");
            std::fs::remove_dir_all(root2).ok();
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn transaction_missing_target_is_removed_on_failure_and_created_private_on_success() {
        let root = scratch("missing-target");
        let target = root.join("nested/settings.json");
        let keys = Arc::new(TestKeys::available());
        let snapshots = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
        let ownership = FileOwnershipStore::new(root.join("ownership"));

        let rejected = prepare(&target, "vk-missing-target-rejected");
        let rejecting_engine = TransactionEngine::new(
            &snapshots,
            &ownership,
            keys.as_ref(),
            &FsAtomicConfigWriter,
            &RejectVerifier,
            &TEST_CLOCK,
        );
        let error = rejecting_engine
            .apply_connection(&rejected, &confirmation(&rejected), &admission(), 1_002)
            .expect_err("post-write failure restores an originally missing target");
        assert_eq!(error.recovery, RecoveryStatus::Restored);
        assert!(!target.exists());

        let accepted = prepare(&target, "vk-missing-target-success");
        let accepting_engine = TransactionEngine::new(
            &snapshots,
            &ownership,
            keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &TEST_CLOCK,
        );
        accepting_engine
            .apply_connection(&accepted, &confirmation(&accepted), &admission(), 1_002)
            .unwrap();
        assert!(target.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn disconnect_rejects_user_changes_to_owned_values_without_writing() {
        let root = scratch("owned-conflict");
        let target = root.join("settings.json");
        write_initial(&target, br#"{"unowned":"keep"}"#);
        let connect = prepare(&target, "vk-owned-original");
        let keys = Arc::new(TestKeys::available());
        let snapshots = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
        let ownership_store = FileOwnershipStore::new(root.join("ownership"));
        let engine = TransactionEngine::new(
            &snapshots,
            &ownership_store,
            keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &TEST_CLOCK,
        );
        engine
            .apply_connection(&connect, &confirmation(&connect), &admission(), 1_002)
            .unwrap();
        let ownership_key = OwnershipKey {
            agent_id: "claude-code".to_string(),
            installation_path: "/opt/claude".to_string(),
            target_config_path: target.to_str().unwrap().to_string(),
        };
        let ownership = ownership_store.load(&ownership_key).unwrap().unwrap();
        let baseline = snapshots.load(&ownership.baseline_snapshot_id).unwrap();
        let baseline_source = ConfigSource {
            existed: baseline.record.original_existed,
            exact_bytes: baseline.exact_bytes,
            original_permissions: baseline.record.original_permissions,
            original_owner: baseline.record.original_owner.clone(),
        };
        let mut changed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        changed["env"]["ANTHROPIC_AUTH_TOKEN"] = serde_json::json!("user-overrode-owned-value");
        let changed_bytes = serde_json::to_vec_pretty(&changed).unwrap();
        std::fs::write(&target, &changed_bytes).unwrap();
        let snapshot_count = snapshots
            .list("claude-code", target.to_str().unwrap())
            .unwrap()
            .len();
        let current = read_config_source(&target).unwrap();
        let error = build_disconnect_plan(
            &ClaudeCodeConnector,
            &discovery(&target),
            &verified(),
            &target,
            &current,
            &ownership,
            &baseline.record,
            &baseline_source,
            &keys.load().unwrap(),
            1,
            None,
            2_000,
            "12".repeat(16),
        )
        .err()
        .expect("owned value conflict requires a new explicit decision");
        assert!(error.contains("owned paths"));
        assert_eq!(std::fs::read(&target).unwrap(), changed_bytes);
        assert_eq!(
            snapshots
                .list("claude-code", target.to_str().unwrap())
                .unwrap()
                .len(),
            snapshot_count
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn blocked_compatibility_still_allows_bound_disconnect_recovery() {
        let root = scratch("blocked-disconnect");
        let target = root.join("settings.json");
        write_initial(&target, br#"{"unowned":"keep"}"#);
        let connect = prepare(&target, "vk-blocked-disconnect");
        let keys = Arc::new(TestKeys::available());
        let snapshots = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
        let ownership_store = FileOwnershipStore::new(root.join("ownership"));
        let engine = TransactionEngine::new(
            &snapshots,
            &ownership_store,
            keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &TEST_CLOCK,
        );
        engine
            .apply_connection(&connect, &confirmation(&connect), &admission(), 1_002)
            .unwrap();
        let ownership_key = OwnershipKey {
            agent_id: "claude-code".to_string(),
            installation_path: "/opt/claude".to_string(),
            target_config_path: target.to_str().unwrap().to_string(),
        };
        let ownership = ownership_store.load(&ownership_key).unwrap().unwrap();
        let baseline = snapshots.load(&ownership.baseline_snapshot_id).unwrap();
        let baseline_source = ConfigSource {
            existed: baseline.record.original_existed,
            exact_bytes: baseline.exact_bytes,
            original_permissions: baseline.record.original_permissions,
            original_owner: baseline.record.original_owner.clone(),
        };
        let mut blocked = verified();
        blocked.status = CompatibilityStatus::DetectedBlocked;
        blocked.reason_code = ReasonCode::BlockedVersionMatch;
        blocked.allowed_actions = BTreeSet::from([
            AllowedAction::Disconnect,
            AllowedAction::ViewSnapshots,
            AllowedAction::RestoreSnapshot,
        ]);
        let current = read_config_source(&target).unwrap();
        let disconnect = build_disconnect_plan(
            &ClaudeCodeConnector,
            &discovery(&target),
            &blocked,
            &target,
            &current,
            &ownership,
            &baseline.record,
            &baseline_source,
            &keys.load().unwrap(),
            2,
            None,
            2_000,
            "34".repeat(16),
        )
        .unwrap();
        engine
            .apply_disconnect(
                &disconnect,
                &confirmation_at(&disconnect, 2_001),
                &RuntimeAdmission {
                    compatibility_sequence: 2,
                    status: CompatibilityStatus::DetectedBlocked,
                },
                2_002,
            )
            .unwrap();
        let restored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(restored["unowned"], "keep");
        assert!(restored["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
        assert!(ownership_store.load(&ownership_key).unwrap().is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn transaction_self_check_or_ownership_failure_restores_exact_original() {
        for (label, reject_verify, reject_ownership) in
            [("self-check", true, false), ("ownership", false, true)]
        {
            let root = scratch(label);
            let target = root.join("settings.json");
            let initial = br#"{"unowned":"keep","env":{"USER_VALUE":"yes"}}"#;
            write_initial(&target, initial);
            let plan = prepare(&target, "vk-rollback-secret");
            let keys = Arc::new(TestKeys::available());
            let snapshots = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
            let real_ownership = FileOwnershipStore::new(root.join("ownership"));
            let ownership: &dyn OwnershipStore = if reject_ownership {
                &FailOwnershipStore
            } else {
                &real_ownership
            };
            let verifier: &dyn PostWriteVerifier = if reject_verify {
                &RejectVerifier
            } else {
                &ParseOnlyVerifier
            };
            let engine = TransactionEngine::new(
                &snapshots,
                ownership,
                keys.as_ref(),
                &FsAtomicConfigWriter,
                verifier,
                &TEST_CLOCK,
            );
            let error = engine
                .apply_connection(&plan, &confirmation(&plan), &admission(), 1_002)
                .expect_err("post-write failure is reported");
            assert_eq!(error.recovery, RecoveryStatus::Restored, "{label}");
            assert_eq!(std::fs::read(&target).unwrap(), initial, "{label}");
            let serialized = serde_json::to_string(&error).unwrap();
            assert!(!serialized.contains("vk-rollback-secret"));
            std::fs::remove_dir_all(root).ok();
        }
    }

    #[test]
    fn transaction_rechecks_expiry_after_snapshot_before_target_write() {
        let root = scratch("snapshot-expiry");
        let target = root.join("settings.json");
        let initial = br#"{"unowned":"keep"}"#;
        write_initial(&target, initial);
        let plan = prepare(&target, "vk-expiry-secret");
        let keys = Arc::new(TestKeys::available());
        let inner = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
        let clock = AtomicClock(AtomicU64::new(1_002));
        let snapshots = AdvancingSnapshots {
            inner: &inner,
            clock: &clock,
            advance_to: plan.view.expires_at_ms.saturating_add(1),
        };
        let ownership = FileOwnershipStore::new(root.join("ownership"));
        let engine = TransactionEngine::new(
            &snapshots,
            &ownership,
            keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &clock,
        );
        let error = engine
            .apply_connection(&plan, &confirmation(&plan), &admission(), 1_002)
            .expect_err("expiry after snapshot is rejected");
        assert_eq!(error.stage, TransactionStage::Confirmation);
        assert_eq!(std::fs::read(&target).unwrap(), initial);
        assert_eq!(
            inner
                .list("claude-code", target.to_str().unwrap())
                .unwrap()
                .len(),
            1
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn transaction_reports_primary_and_restore_failure_separately() {
        let root = scratch("restore-failure");
        let target = root.join("settings.json");
        let initial = br#"{"unowned":"keep"}"#;
        write_initial(&target, initial);
        let plan = prepare(&target, "vk-repair-required-secret");
        let keys = Arc::new(TestKeys::available());
        let snapshots = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
        let ownership = FileOwnershipStore::new(root.join("ownership"));
        let writer = FailRecoveryWriter {
            calls: AtomicUsize::new(0),
        };
        let engine = TransactionEngine::new(
            &snapshots,
            &ownership,
            keys.as_ref(),
            &writer,
            &RejectVerifier,
            &TEST_CLOCK,
        );
        let error = engine
            .apply_connection(&plan, &confirmation(&plan), &admission(), 1_002)
            .expect_err("restore failure is reported");
        assert_eq!(error.stage, TransactionStage::PostWriteSelfCheck);
        assert_eq!(error.recovery, RecoveryStatus::RepairRequired);
        assert_eq!(
            error.recovery_reason_code.as_deref(),
            Some("restore_write_failed_or_target_changed")
        );
        let serialized = serde_json::to_string(&error).unwrap();
        assert!(!serialized.contains("vk-repair-required-secret"));
        assert_ne!(std::fs::read(&target).unwrap(), initial);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn transaction_atomic_writer_helpers_and_intent_boundaries_are_covered() {
        assert!(SystemClock.now_ms() > 0);
        let document = parse_source_bytes(
            Some(br#"{"ok":true}"#),
            super::super::config_codec::DocumentFormat::Json,
            "fixture",
        )
        .unwrap();
        ParseOnlyVerifier
            .verify(Path::new("/unused"), &document)
            .unwrap();
        assert_eq!(
            temporary_path(Path::new("/")).unwrap_err(),
            AtomicWriteStage::TempCreate
        );
        for (stage, reason) in [
            (
                AtomicWriteStage::ParentCreate,
                "target_parent_create_failed",
            ),
            (AtomicWriteStage::TempCreate, "target_temp_create_failed"),
            (AtomicWriteStage::TempWrite, "target_temp_write_failed"),
            (AtomicWriteStage::TempFlush, "target_temp_flush_failed"),
            (AtomicWriteStage::TempFsync, "target_temp_fsync_failed"),
            (
                AtomicWriteStage::Permission,
                "target_permission_restore_failed",
            ),
            (
                AtomicWriteStage::PreReplaceRevision,
                "target_changed_before_replace",
            ),
            (AtomicWriteStage::Replace, "target_atomic_replace_failed"),
            (
                AtomicWriteStage::DirectoryFsync,
                "target_directory_fsync_failed",
            ),
            (AtomicWriteStage::Remove, "target_remove_failed"),
        ] {
            assert_eq!(atomic_reason(stage), reason);
        }

        let root = scratch("atomic-writer");
        let target = root.join("settings.json");
        write_initial(&target, br#"{"value":1}"#);
        let before = read_config_source(&target).unwrap();
        let before_hash = file_revision_hash(&target, &before).unwrap();
        assert!(FsAtomicConfigWriter
            .replace(
                &target,
                br#"{"value":2}"#,
                Some(0o600),
                None,
                &before_hash,
                false,
            )
            .is_ok());
        assert!(FsAtomicConfigWriter
            .replace(
                &target,
                b"unchanged",
                Some(0o600),
                None,
                &before_hash,
                false
            )
            .is_err());
        #[cfg(unix)]
        {
            let current = read_config_source(&target).unwrap();
            let current_hash = file_revision_hash(&target, &current).unwrap();
            let failure = match FsAtomicConfigWriter.replace(
                &target,
                b"unchanged",
                Some(0o600),
                Some("invalid"),
                &current_hash,
                false,
            ) {
                Err(failure) => failure,
                Ok(()) => panic!("invalid owner must be rejected"),
            };
            assert_eq!(failure.stage, AtomicWriteStage::Permission);
        }
        let missing = root.join("missing.json");
        let missing_source = read_config_source(&missing).unwrap();
        let missing_hash = file_revision_hash(&missing, &missing_source).unwrap();
        let failure = match FsAtomicConfigWriter.remove(&missing, &missing_hash, false) {
            Err(failure) => failure,
            Ok(()) => panic!("missing target must not be removable"),
        };
        assert_eq!(failure.stage, AtomicWriteStage::Remove);
        let current = read_config_source(&target).unwrap();
        let current_hash = file_revision_hash(&target, &current).unwrap();
        assert!(FsAtomicConfigWriter
            .remove(&target, &current_hash, false)
            .is_ok());
        assert!(!target.exists());

        let plan_target = root.join("intent.json");
        write_initial(&plan_target, br#"{"unowned":"keep"}"#);
        let keys = Arc::new(TestKeys::available());
        let snapshots = FileSnapshotStore::new(root.join("snapshots"), keys.clone());
        let ownership = FileOwnershipStore::new(root.join("ownership"));
        let engine = TransactionEngine::new(
            &snapshots,
            &ownership,
            keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &TEST_CLOCK,
        );
        let mut connect_mismatch = prepare(&plan_target, "vk-intent-1");
        connect_mismatch.view.intent = PlanIntent::Disconnect;
        assert_eq!(
            engine
                .apply_connection(
                    &connect_mismatch,
                    &confirmation(&connect_mismatch),
                    &admission(),
                    1_002
                )
                .unwrap_err()
                .reason_code,
            "operation_intent_mismatch"
        );
        let disconnect_mismatch = prepare(&plan_target, "vk-intent-2");
        assert!(engine
            .apply_disconnect(
                &disconnect_mismatch,
                &confirmation(&disconnect_mismatch),
                &admission(),
                1_002
            )
            .is_err());
        let restore_mismatch = prepare(&plan_target, "vk-intent-3");
        assert!(engine
            .apply_snapshot_restore(
                &restore_mismatch,
                &confirmation(&restore_mismatch),
                &admission(),
                1_002
            )
            .is_err());
        std::fs::remove_dir_all(root).ok();
    }
}

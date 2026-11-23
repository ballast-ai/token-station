//! Local backup, export, import and restore — single-machine disaster recovery
//! without a cloud round-trip.
//!
//! Two operations, kept apart on purpose:
//!
//! - **Config export / import** moves the configuration as validated JSON.
//!   Secret *values* never live in the config (only Keychain references do), so
//!   an export is safe to hand to another machine; import re-validates before it
//!   writes, and writes atomically, so a bad file replaces nothing.
//! - **Backup / restore** copies the whole local state — the config and the
//!   metrics database — into (or out of) a directory. A restore backs the
//!   current files up first, so restoring is itself reversible.

use std::path::{Path, PathBuf};

use crate::config::ClientConfig;
use crate::store::SchemaCompatibility;

const BACKUP_CONFIG: &str = "config.json";
const BACKUP_METRICS: &str = "metrics.sqlite";

/// The config as pretty JSON. The value is already validated — it came from
/// [`ClientConfig::load`] — so this only serializes it.
///
/// # Errors
///
/// Serialization failure, which for a well-formed config does not happen.
pub fn export_config(config: &ClientConfig) -> Result<String, String> {
    let mut json = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    json.push('\n');
    Ok(json)
}

/// Parses, validates and atomically writes a config from exported JSON.
///
/// # Errors
///
/// When the JSON is not a valid config, or the write fails. An invalid import
/// replaces nothing at `dest`.
pub fn import_config(json: &str, dest: &Path) -> Result<(), String> {
    let config: ClientConfig =
        serde_json::from_str(json).map_err(|error| format!("{}: {error}", dest.display()))?;
    config.save(dest).map_err(|error| error.to_string())
}

/// Copies the config and (if present) the metrics database into `dest_dir`,
/// creating it if needed. Returns the directory written.
///
/// # Errors
///
/// A filesystem failure creating the directory or copying a file.
pub fn backup(config_path: &Path, metrics_path: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    if !dest_dir.exists() {
        crate::private_fs::ensure_private_dir(dest_dir)
            .map_err(|error| format!("{}: {error}", dest_dir.display()))?;
    }
    let _destination_lease =
        crate::store::metrics_lifecycle_lock(&dest_dir.join(BACKUP_METRICS), true)?;

    copy(config_path, &dest_dir.join(BACKUP_CONFIG))?;
    // The metrics store is optional (it can be disabled); back it up only if it
    // is actually there.
    if metrics_path.exists() {
        let destination = dest_dir.join(BACKUP_METRICS);
        let staged = unique_sibling(&destination, "snapshot")?;
        crate::store::snapshot_database(metrics_path, &staged)?;
        match swap_in(&staged, &destination) {
            Ok(Some(old)) => {
                std::fs::remove_file(&old)
                    .map_err(|error| format!("{}: {error}", old.display()))?;
            }
            Ok(None) => {}
            Err(error) => {
                let _ = std::fs::remove_file(&staged);
                return Err(format!(
                    "publish metrics snapshot `{}`: {error}",
                    destination.display()
                ));
            }
        }
    }
    Ok(dest_dir.to_path_buf())
}

/// Restores config + metrics from a backup directory, copying the current files
/// aside to `<file>.pre-restore` first so the restore can be undone.
///
/// # Errors
///
/// A missing backup file, or a filesystem failure copying one into place.
pub fn restore(backup_dir: &Path, config_path: &Path, metrics_path: &Path) -> Result<(), String> {
    // Hold this across validation, staging, replacement, rollback, and cleanup.
    // Read-only SQLite helpers do not acquire a writer lease and cannot self-lock.
    let _restore_lease = crate::store::metrics_lifecycle_lock(metrics_path, true)?;
    let backup_config = backup_dir.join(BACKUP_CONFIG);
    if !backup_config.exists() {
        return Err(format!(
            "backup `{}` has no {BACKUP_CONFIG}",
            backup_dir.display()
        ));
    }

    // The config being restored must be a valid config, not arbitrary bytes.
    ClientConfig::load(&backup_config).map_err(|error| error.to_string())?;

    let backup_metrics = backup_dir.join(BACKUP_METRICS);
    if backup_metrics.exists() {
        match crate::store::inspect_schema(&backup_metrics)? {
            SchemaCompatibility::Current { .. } | SchemaCompatibility::Older { .. } => {}
            SchemaCompatibility::Missing => {
                return Err(format!(
                    "backup metrics `{}` disappeared during validation",
                    backup_metrics.display()
                ));
            }
            SchemaCompatibility::Newer { found, supported } => {
                return Err(format!(
                    "backup metrics schema {found} is newer than supported schema {supported}"
                ));
            }
        }
    }

    // Stage and revalidate every member before the first live path changes.
    let staged_config = unique_sibling(config_path, "restore")?;
    copy(&backup_config, &staged_config)?;
    ClientConfig::load(&staged_config).map_err(|error| {
        let _ = std::fs::remove_file(&staged_config);
        error.to_string()
    })?;
    let staged_metrics = if backup_metrics.exists() {
        let staged = unique_sibling(metrics_path, "restore")?;
        if let Err(error) = crate::store::snapshot_database(&backup_metrics, &staged) {
            let _ = std::fs::remove_file(&staged_config);
            let _ = std::fs::remove_file(&staged);
            return Err(error);
        }
        Some(staged)
    } else {
        None
    };

    stash_current(config_path)?;
    if staged_metrics.is_some() {
        stash_current(metrics_path)?;
    }

    let config_old = swap_in(&staged_config, config_path)?;
    let metrics_old = if let Some(staged) = &staged_metrics {
        match swap_in(staged, metrics_path) {
            Ok(old) => old,
            Err(error) => {
                rollback_swap(config_path, config_old.as_deref())?;
                return Err(error);
            }
        }
    } else {
        None
    };
    if let Some(old) = config_old {
        std::fs::remove_file(&old).map_err(|error| format!("{}: {error}", old.display()))?;
    }
    if let Some(old) = metrics_old {
        std::fs::remove_file(&old).map_err(|error| format!("{}: {error}", old.display()))?;
    }
    Ok(())
}

fn unique_sibling(path: &Path, tag: &str) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{}: path has no parent", path.display()))?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    // Directory aliases are supported. Stage in the resolved directory while
    // retaining create-new and owner-only checks for the file itself.
    let parent =
        std::fs::canonicalize(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| format!("{}: path has no file name", path.display()))?;
    for _ in 0..16 {
        let mut random = [0_u8; 12];
        getrandom::fill(&mut random).map_err(|error| format!("randomness: {error}"))?;
        let suffix = random.iter().fold(String::new(), |mut output, byte| {
            use std::fmt::Write as _;
            let _ = write!(output, "{byte:02x}");
            output
        });
        let candidate = parent.join(format!(".{name}.{tag}-{suffix}.tmp"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "cannot allocate a staging path beside {}",
        path.display()
    ))
}

/// Moves the live file aside, then publishes the staged file. Returns the
/// private old-file path so the caller can roll the whole restore back.
fn swap_in(staged: &Path, target: &Path) -> Result<Option<PathBuf>, String> {
    let old = if target.exists() {
        let old = unique_sibling(target, "old")?;
        std::fs::rename(target, &old)
            .map_err(|error| format!("stash live `{}`: {error}", target.display()))?;
        Some(old)
    } else {
        None
    };
    if let Err(error) = std::fs::rename(staged, target) {
        if let Some(old) = &old {
            let _ = std::fs::rename(old, target);
        }
        return Err(format!("publish restored `{}`: {error}", target.display()));
    }
    Ok(old)
}

fn rollback_swap(target: &Path, old: Option<&Path>) -> Result<(), String> {
    let Some(old) = old else {
        std::fs::remove_file(target).map_err(|error| format!("{}: {error}", target.display()))?;
        return Ok(());
    };
    std::fs::remove_file(target).map_err(|error| format!("{}: {error}", target.display()))?;
    std::fs::rename(old, target).map_err(|error| {
        format!(
            "rollback restored `{}` from `{}`: {error}",
            target.display(),
            old.display()
        )
    })
}

/// Moves an existing file to `<name>.pre-restore` so a restore is reversible. A
/// file that is not there yet needs no stash.
fn stash_current(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let file_name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| format!("{}: path has no file name", path.display()))?;
    let stash = path.with_file_name(format!("{file_name}.pre-restore"));
    copy(path, &stash)
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    let staged = unique_sibling(to, "copy")?;
    let result = (|| {
        let mut source = std::fs::File::open(from)?;
        let mut target = token_station_private_fs::open_new_private_file(&staged)?;
        std::io::copy(&mut source, &mut target)?;
        target.sync_all()
    })()
    .map_err(|error: std::io::Error| {
        format!("copy `{}` -> `{}`: {error}", from.display(), to.display())
    });
    if let Err(error) = result {
        let _ = std::fs::remove_file(&staged);
        return Err(error);
    }
    let old = match swap_in(&staged, to) {
        Ok(old) => old,
        Err(error) => {
            let _ = std::fs::remove_file(&staged);
            return Err(error);
        }
    };
    if let Some(old) = old {
        std::fs::remove_file(&old).map_err(|error| format!("{}: {error}", old.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{backup, export_config, import_config, restore};
    use crate::config::ClientConfig;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ts-backup-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn sample_config() -> ClientConfig {
        // The shipped example is a valid config; use it rather than hand-build one.
        serde_json::from_str(crate::EXAMPLE_CONFIG).expect("the shipped example parses")
    }

    #[test]
    fn restore_refuses_a_live_store_before_changing_any_files() {
        let dir = scratch("active-writer");
        let config = dir.join("config.json");
        let metrics = dir.join("metrics.sqlite");
        import_config(&export_config(&sample_config()).unwrap(), &config).unwrap();
        let writer = crate::store::SqliteStore::open(&metrics).unwrap();
        let saved = dir.join("backup");
        backup(&config, &metrics, &saved).unwrap();
        let before = std::fs::read(&metrics).unwrap();
        let error =
            restore(&saved, &config, &metrics).expect_err("a live writer must block restore");
        assert!(error.contains("in use"), "{error}");
        assert_eq!(std::fs::read(&metrics).unwrap(), before);
        assert!(!dir.join("config.json.pre-restore").exists());
        drop(writer);
        restore(&saved, &config, &metrics).unwrap();
        crate::store::SqliteStore::open(&metrics).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn backup_keeps_new_directory_and_database_private() {
        let dir = scratch("backup-private");
        let config = dir.join("config.json");
        let metrics = dir.join("metrics.sqlite");
        import_config(&export_config(&sample_config()).unwrap(), &config).unwrap();
        let writer = crate::store::SqliteStore::open(&metrics).unwrap();
        let saved = dir.join("backup");
        backup(&config, &metrics, &saved).unwrap();
        token_station_private_fs::verify_private_dir(&saved).unwrap();
        for name in ["config.json", "metrics.sqlite"] {
            token_station_private_fs::verify_private_file(&saved.join(name)).unwrap();
        }
        drop(writer);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn backup_preserves_shared_destination_permissions_and_replaces_private_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("shared-destination");
        let config = dir.join("config.json");
        let metrics = dir.join("metrics.sqlite");
        import_config(&export_config(&sample_config()).unwrap(), &config).unwrap();
        let writer = crate::store::SqliteStore::open(&metrics).unwrap();
        let saved = dir.join("existing");
        std::fs::create_dir(&saved).unwrap();
        std::fs::set_permissions(&saved, std::fs::Permissions::from_mode(0o755)).unwrap();
        for _ in 0..2 {
            backup(&config, &metrics, &saved).unwrap();
            assert_eq!(
                std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777,
                0o755
            );
            for name in ["config.json", "metrics.sqlite"] {
                token_station_private_fs::verify_private_file(&saved.join(name)).unwrap();
            }
        }
        assert!(
            backup(&config, &metrics, &dir).is_err(),
            "backup must not replace an active source database"
        );
        drop(writer);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn restore_resolves_symlink_aliases_and_refuses_hardlinks_without_mutation() {
        let dir = scratch("path-aliases");
        let config = dir.join("config.json");
        let metrics = dir.join("metrics.sqlite");
        import_config(&export_config(&sample_config()).unwrap(), &config).unwrap();
        let writer = crate::store::SqliteStore::open(&metrics).unwrap();
        let saved = dir.join("backup");
        backup(&config, &metrics, &saved).unwrap();
        let alias = dir.join("linked.sqlite");
        std::os::unix::fs::symlink(&metrics, &alias).unwrap();
        assert!(
            restore(&saved, &config, &alias)
                .unwrap_err()
                .contains("in use")
        );
        let parent_alias = dir.join("linked-dir");
        std::os::unix::fs::symlink(&dir, &parent_alias).unwrap();
        assert!(
            restore(&saved, &config, &parent_alias.join("metrics.sqlite"))
                .unwrap_err()
                .contains("in use")
        );
        let second = crate::store::SqliteStore::open(&alias).unwrap();
        drop(second);
        drop(writer);
        std::fs::hard_link(&metrics, dir.join("hardlink.sqlite")).unwrap();
        assert!(
            restore(&saved, &config, &metrics)
                .unwrap_err()
                .contains("hard link")
        );
        assert!(
            crate::store::SqliteStore::open(&metrics)
                .err()
                .unwrap()
                .contains("hard link")
        );
        assert!(!dir.join("config.json.pre-restore").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn restore_lease_blocks_new_writers_until_released() {
        let dir = scratch("exclusive-lease");
        let metrics = dir.join("metrics.sqlite");
        let lease = crate::store::metrics_lifecycle_lock(&metrics, true).unwrap();
        assert!(
            crate::store::SqliteStore::open(&metrics)
                .err()
                .unwrap()
                .contains("in use")
        );
        assert!(!metrics.exists());
        drop(lease);
        let writer = crate::store::SqliteStore::open(&metrics).unwrap();
        let second = crate::store::SqliteStore::open(&metrics).unwrap();
        drop(writer);
        assert!(crate::store::metrics_lifecycle_lock(&metrics, true).is_err());
        drop(second);
        crate::store::metrics_lifecycle_lock(&metrics, true).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn a_dangling_database_symlink_uses_the_target_lifecycle_lock() {
        let dir = scratch("dangling-alias");
        let metrics = dir.join("metrics.sqlite");
        let alias = dir.join("linked.sqlite");
        std::os::unix::fs::symlink("metrics.sqlite", &alias).unwrap();
        let writer = crate::store::SqliteStore::open(&alias).unwrap();
        assert!(crate::store::metrics_lifecycle_lock(&metrics, true).is_err());
        drop(writer);
        crate::store::metrics_lifecycle_lock(&metrics, true).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "Subprocess helper for the cross-process lifecycle regression"]
    fn writer_process_helper() {
        use std::io::Read as _;
        let Some(directory) = std::env::var_os("TS_BACKUP_TEST_DIRECTORY") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        let _store = crate::store::SqliteStore::open(&directory.join("metrics.sqlite")).unwrap();
        std::fs::write(directory.join("ready"), b"ready").unwrap();
        let _ = std::io::stdin().read(&mut [0_u8; 1]);
    }

    #[test]
    fn restore_refuses_another_process_and_recovers_after_process_death() {
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let dir = scratch("process-writer");
        let config = dir.join("config.json");
        let metrics = dir.join("metrics.sqlite");
        import_config(&export_config(&sample_config()).unwrap(), &config).unwrap();
        let mut child = ChildGuard(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "backup::tests::writer_process_helper",
                    "--ignored",
                    "--nocapture",
                ])
                .env("TS_BACKUP_TEST_DIRECTORY", &dir)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !dir.join("ready").exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "writer child exited before readiness"
            );
            assert!(
                std::time::Instant::now() < deadline,
                "writer child did not become ready"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let saved = dir.join("backup");
        backup(&config, &metrics, &saved).unwrap();
        assert!(
            restore(&saved, &config, &metrics)
                .unwrap_err()
                .contains("in use")
        );
        assert!(!dir.join("config.json.pre-restore").exists());
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        restore(&saved, &config, &metrics).unwrap();
        crate::store::SqliteStore::open(&metrics).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_config_survives_an_export_import_round_trip() {
        let dir = scratch("roundtrip");
        let config = sample_config();

        let exported = export_config(&config).expect("exports");
        let dest = dir.join("config.json");
        import_config(&exported, &dest).expect("imports a valid config");

        let reloaded = ClientConfig::load(&dest).expect("the imported file is a valid config");
        assert_eq!(
            reloaded, config,
            "the config is unchanged by the round trip"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn importing_junk_replaces_nothing() {
        let dir = scratch("junk");
        let dest = dir.join("config.json");
        // Seed a valid config, then try to import garbage over it.
        import_config(&export_config(&sample_config()).unwrap(), &dest).expect("seed");
        let before = std::fs::read_to_string(&dest).expect("reads");

        assert!(import_config("{ not valid", &dest).is_err());
        let after = std::fs::read_to_string(&dest).expect("still there");
        assert_eq!(before, after, "a failed import leaves the good file intact");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn backup_then_restore_round_trips_config_and_metrics() {
        let dir = scratch("backup");
        let config_path = dir.join("config.json");
        let metrics_path = dir.join("metrics.sqlite");
        import_config(&export_config(&sample_config()).unwrap(), &config_path).expect("config");
        crate::store::SqliteStore::open(&metrics_path).expect("metrics store");
        {
            let connection = rusqlite::Connection::open(&metrics_path).expect("metrics opens");
            connection
                .execute("CREATE TABLE backup_canary(value TEXT)", [])
                .expect("canary table");
            connection
                .execute("INSERT INTO backup_canary VALUES ('kept')", [])
                .expect("canary row");
        }

        let backup_dir = dir.join("backup-1");
        backup(&config_path, &metrics_path, &backup_dir).expect("backs up");

        // Clobber the live files, then restore.
        std::fs::write(&metrics_path, b"corrupted").expect("clobber");
        restore(&backup_dir, &config_path, &metrics_path).expect("restores");

        let restored = rusqlite::Connection::open(&metrics_path).expect("restored metrics opens");
        let canary: String = restored
            .query_row("SELECT value FROM backup_canary", [], |row| row.get(0))
            .expect("snapshot preserved rows");
        assert_eq!(canary, "kept");
        // The restore is reversible: the clobbered version was stashed.
        assert!(
            dir.join("metrics.sqlite.pre-restore").exists(),
            "the pre-restore state was kept"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_corrupt_metrics_backup_is_refused_before_live_config_changes() {
        let dir = scratch("corrupt-metrics");
        let config_path = dir.join("config.json");
        let metrics_path = dir.join("metrics.sqlite");
        import_config(&export_config(&sample_config()).unwrap(), &config_path).expect("config");
        crate::store::SqliteStore::open(&metrics_path).expect("metrics store");
        let before_config = std::fs::read(&config_path).expect("live config reads");

        let backup_dir = dir.join("backup-corrupt");
        std::fs::create_dir_all(&backup_dir).expect("backup dir");
        std::fs::write(
            backup_dir.join(super::BACKUP_CONFIG),
            export_config(&sample_config()).unwrap(),
        )
        .expect("backup config");
        std::fs::write(
            backup_dir.join(super::BACKUP_METRICS),
            b"not a sqlite database",
        )
        .expect("corrupt metrics");

        restore(&backup_dir, &config_path, &metrics_path)
            .expect_err("all backup members must validate before any live file changes");
        assert_eq!(
            std::fs::read(&config_path).expect("live config remains"),
            before_config
        );
        assert!(
            !config_path.with_extension("json.pre-restore").exists(),
            "validation failure must not begin the restore transaction"
        );
        std::fs::remove_dir_all(dir).ok();
    }
}

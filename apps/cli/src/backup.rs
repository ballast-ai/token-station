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
    std::fs::create_dir_all(dest_dir)
        .map_err(|error| format!("{}: {error}", dest_dir.display()))?;

    copy(config_path, &dest_dir.join(BACKUP_CONFIG))?;
    // The metrics store is optional (it can be disabled); back it up only if it
    // is actually there.
    if metrics_path.exists() {
        let destination = dest_dir.join(BACKUP_METRICS);
        let staged = unique_sibling(&destination, "snapshot")?;
        crate::store::snapshot_database(metrics_path, &staged)?;
        match std::fs::rename(&staged, &destination) {
            Ok(()) => {}
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
    std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
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
    std::fs::copy(path, &stash).map_err(|error| format!("{}: {error}", stash.display()))?;
    Ok(())
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|error| format!("copy `{}` -> `{}`: {error}", from.display(), to.display()))
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

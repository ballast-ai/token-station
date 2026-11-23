//! Backup compatibility through the actual CLI and private filesystem boundary.

use std::path::PathBuf;
use std::process::Command;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random).unwrap();
        let directory = std::env::temp_dir().join(format!(
            "ts-backup-cli-{}-{:016x}",
            std::process::id(),
            u64::from_ne_bytes(random)
        ));
        std::fs::create_dir(&directory).unwrap();
        let mut config: serde_json::Value =
            serde_json::from_str(token_station_cli::EXAMPLE_CONFIG).unwrap();
        config["data"]["dir"] = serde_json::json!(directory.join("data"));
        std::fs::create_dir(directory.join("data")).unwrap();
        std::fs::write(
            directory.join("token-station.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        drop(
            token_station_cli::store::SqliteStore::open(&directory.join("data/metrics.sqlite"))
                .unwrap(),
        );
        Self(directory)
    }

    fn run(&self, args: &[&str]) {
        let result = Command::new(env!("CARGO_BIN_EXE_token-station-cli"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn default_relative_config_path_round_trips_through_backup_and_restore() {
    let fixture = Fixture::new();
    let original = std::fs::read(fixture.0.join("token-station.json")).unwrap();
    fixture.run(&["backup", "saved"]);
    fixture.run(&["restore", "saved"]);
    assert_eq!(
        std::fs::read(fixture.0.join("token-station.json")).unwrap(),
        original
    );
    assert_eq!(
        std::fs::read(fixture.0.join("token-station.json.pre-restore")).unwrap(),
        original
    );
}

#[test]
#[cfg(unix)]
fn existing_backup_directory_symlink_remains_usable_with_private_outputs() {
    let fixture = Fixture::new();
    let destination = fixture.0.join("saved");
    std::fs::create_dir(&destination).unwrap();
    std::os::unix::fs::symlink(&destination, fixture.0.join("linked-backup")).unwrap();
    fixture.run(&["backup", "linked-backup"]);
    for name in ["config.json", "metrics.sqlite"] {
        token_station_private_fs::verify_private_file(&destination.join(name)).unwrap();
    }
    assert!(fixture.0.join("linked-backup").is_symlink());
}

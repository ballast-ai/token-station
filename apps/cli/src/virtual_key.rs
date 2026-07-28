//! The local virtual key: what stands between the loopback port and every
//! other process on this machine.
//!
//! Generated automatically on first start, never printed by the server, and
//! required on every endpoint by default. Loopback is a boundary
//! against the network, not against local software — any browser tab that can
//! issue `fetch("http://127.0.0.1:8787/...")` is on this side of it, and what
//! it would reach is a proxy holding the operator's provider keys.
//!
//! The key lives in a `0600` file in the data directory rather than in the
//! configuration: the config file is meant to be shareable and diffable, and a
//! file that holds a credential is neither.

use std::io::Write;
use std::path::Path;

/// `ts-` + 48 hex characters (24 random bytes).
const KEY_BYTES: usize = 24;

/// Loads the virtual key, creating it on first start.
///
/// Returns the key and whether this call created it. The caller uses the key
/// in memory and reports only the owner-only file path; startup output ends up
/// in logs and must never carry credential material.
///
/// # Errors
///
/// A message for the operator; never contains the key.
pub fn load_or_create(data_dir: &Path) -> Result<(String, bool), String> {
    let path = data_dir.join("virtual-key");

    match std::fs::read_to_string(&path) {
        Ok(existing) => {
            let existing = existing.trim();
            if existing.is_empty() {
                return Err(format!(
                    "`{}` exists but is empty; delete it to generate a new key",
                    path.display()
                ));
            }
            Ok((existing.to_owned(), false))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(data_dir)
                .map_err(|error| format!("data dir `{}`: {error}", data_dir.display()))?;

            let mut random = [0u8; KEY_BYTES];
            getrandom::fill(&mut random)
                .map_err(|error| format!("system randomness unavailable: {error}"))?;
            let key = format!("ts-{}", hex(&random));

            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&path)
                .map_err(|error| format!("virtual key `{}`: {error}", path.display()))?;
            file.write_all(key.as_bytes())
                .map_err(|error| format!("virtual key `{}`: {error}", path.display()))?;

            Ok((key, true))
        }
        Err(error) => Err(format!("virtual key `{}`: {error}", path.display())),
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// Constant-time equality: the comparison must not leak how much of a guessed
/// key was right. Loopback keeps remote attackers out, not local ones.
#[must_use]
pub fn matches(presented: &str, expected: &str) -> bool {
    if presented.len() != expected.len() {
        return false;
    }
    presented
        .bytes()
        .zip(expected.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::{load_or_create, matches};

    #[test]
    fn first_start_creates_and_later_starts_reuse() {
        let dir = std::env::temp_dir().join(format!("ts-vkey-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();

        let (first, created) = load_or_create(&dir).expect("creates");
        assert!(created);
        assert!(first.starts_with("ts-"));
        assert_eq!(first.len(), 3 + 48);

        let (second, created) = load_or_create(&dir).expect("reloads");
        assert!(!created, "the second start must not mint a new key");
        assert_eq!(second, first);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("virtual-key"))
                .expect("key file exists")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the key file is owner-only");
        }

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn comparison_rejects_prefixes_and_case_variants() {
        assert!(matches("ts-abc123", "ts-abc123"));
        assert!(!matches("ts-abc12", "ts-abc123"));
        assert!(!matches("ts-abc124", "ts-abc123"));
        assert!(!matches("TS-ABC123", "ts-abc123"));
        assert!(!matches("", "ts-abc123"));
    }
}

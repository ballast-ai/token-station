//! Owner-only request/response bodies kept separately from body-free receipts.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use token_station_private_fs::verify_private_file;

use crate::private_fs::{ensure_private_dir, write_atomic_private};

pub const BODY_DIR_NAME: &str = "request-bodies";
pub const DEFAULT_RETENTION_DAYS: u64 = 7;
pub const MAX_BODY_BYTES: usize = 256 * 1024;
pub const MAX_BODY_FILES: usize = 1_000;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpHeaderSnapshot {
    pub name: String,
    pub value: String,
    pub redacted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRequestSnapshot {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<HttpHeaderSnapshot>,
    pub body: String,
    pub body_truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpResponseSnapshot {
    pub status: u16,
    #[serde(default)]
    pub headers: Vec<HttpHeaderSnapshot>,
    pub body: String,
    pub body_truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamHttpExchange {
    pub ordinal: u32,
    pub upstream: String,
    pub model: String,
    pub request: HttpRequestSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<HttpResponseSnapshot>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpTraceSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_request: Option<HttpRequestSnapshot>,
    #[serde(default)]
    pub upstream_exchanges: Vec<UpstreamHttpExchange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_response: Option<HttpResponseSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaintextExchange {
    pub request_id: String,
    pub captured_at_ms: u64,
    pub input: String,
    pub output: String,
    pub input_truncated: bool,
    pub output_truncated: bool,
    #[serde(default)]
    pub http_trace: HttpTraceSnapshot,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CleanupReport {
    pub scanned: usize,
    pub deleted: usize,
}

#[derive(Debug, Default)]
pub struct BoundedBody {
    text: String,
    truncated: bool,
}

impl BoundedBody {
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut value = Self::default();
        value.push(&String::from_utf8_lossy(bytes));
        value
    }

    pub fn push(&mut self, value: &str) {
        if self.truncated || value.is_empty() {
            return;
        }
        let remaining = MAX_BODY_BYTES.saturating_sub(self.text.len());
        if value.len() <= remaining {
            self.text.push_str(value);
            return;
        }
        let mut boundary = remaining.min(value.len());
        while boundary > 0 && !value.is_char_boundary(boundary) {
            boundary -= 1;
        }
        self.text.push_str(&value[..boundary]);
        self.truncated = true;
    }

    #[must_use]
    pub fn into_parts(self) -> (String, bool) {
        (self.text, self.truncated)
    }
}

#[derive(Debug)]
pub struct BodyLog {
    directory: PathBuf,
    writes_since_cleanup: AtomicU64,
}

impl BodyLog {
    /// Opens the dedicated owner-only directory and applies the default retention policy.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be created, hardened, inspected, or cleaned.
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        let directory = data_dir.join(BODY_DIR_NAME);
        ensure_private_dir(&directory)
            .map_err(|error| format!("open request body directory: {error}"))?;
        let log = Self {
            directory,
            writes_since_cleanup: AtomicU64::new(0),
        };
        log.cleanup(
            Duration::from_secs(DEFAULT_RETENTION_DAYS * 24 * 60 * 60),
            MAX_BODY_FILES,
        )?;
        Ok(log)
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Persists one bounded input/output exchange under its generated request ID.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid ID, serialization or private-file write
    /// failure, or a periodic retention cleanup failure.
    pub fn record(
        &self,
        request_id: &str,
        captured_at_ms: u64,
        input: BoundedBody,
        output: BoundedBody,
    ) -> Result<(), String> {
        self.record_with_http_trace(
            request_id,
            captured_at_ms,
            input,
            output,
            HttpTraceSnapshot::default(),
        )
    }

    /// Persists one bounded exchange together with its redacted HTTP trace.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::record`].
    pub fn record_with_http_trace(
        &self,
        request_id: &str,
        captured_at_ms: u64,
        input: BoundedBody,
        output: BoundedBody,
        http_trace: HttpTraceSnapshot,
    ) -> Result<(), String> {
        let path = self.path_for(request_id)?;
        let (input, input_truncated) = input.into_parts();
        let (output, output_truncated) = output.into_parts();
        let exchange = PlaintextExchange {
            request_id: request_id.to_owned(),
            captured_at_ms,
            input,
            output,
            input_truncated,
            output_truncated,
            http_trace,
        };
        let bytes = serde_json::to_vec(&exchange)
            .map_err(|error| format!("serialize request body snapshot: {error}"))?;
        write_atomic_private(&path, &bytes)
            .map_err(|error| format!("write request body snapshot: {error}"))?;
        if self.writes_since_cleanup.fetch_add(1, Ordering::Relaxed) % 64 == 63 {
            self.cleanup(
                Duration::from_secs(DEFAULT_RETENTION_DAYS * 24 * 60 * 60),
                MAX_BODY_FILES,
            )?;
        }
        Ok(())
    }

    /// Reads one previously captured exchange by its generated request ID.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid ID, a non-private or non-regular snapshot,
    /// an I/O failure, malformed JSON, or a filename/content ID mismatch.
    pub fn read(&self, request_id: &str) -> Result<Option<PlaintextExchange>, String> {
        let path = self.path_for(request_id)?;
        match verify_private_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("verify request body snapshot: {error}")),
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("read request body snapshot: {error}")),
        };
        let exchange: PlaintextExchange = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parse request body snapshot: {error}"))?;
        if exchange.request_id != request_id {
            return Err("request body snapshot id does not match its filename".to_owned());
        }
        Ok(Some(exchange))
    }

    /// Deletes expired snapshots and then the oldest files above `max_files`.
    ///
    /// # Errors
    ///
    /// Returns an error when the private directory cannot be scanned or an
    /// eligible regular snapshot cannot be inspected or deleted.
    pub fn cleanup(&self, retention: Duration, max_files: usize) -> Result<CleanupReport, String> {
        let now = SystemTime::now();
        let cutoff = now.checked_sub(retention).unwrap_or(UNIX_EPOCH);
        let entries = std::fs::read_dir(&self.directory)
            .map_err(|error| format!("scan request body directory: {error}"))?;
        let mut candidates = Vec::new();
        let mut report = CleanupReport::default();
        for entry in entries {
            let entry = entry.map_err(|error| format!("scan request body entry: {error}"))?;
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let Some(request_id) = file_name.strip_suffix(".json") else {
                continue;
            };
            if !valid_request_id(request_id) {
                continue;
            }
            let metadata = std::fs::symlink_metadata(entry.path())
                .map_err(|error| format!("inspect request body entry: {error}"))?;
            if !metadata.file_type().is_file() {
                continue;
            }
            report.scanned += 1;
            let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
            candidates.push((modified, entry.path()));
        }
        candidates.sort();

        let expired = candidates.partition_point(|(modified, _)| *modified < cutoff);
        let overflow = candidates
            .len()
            .saturating_sub(expired)
            .saturating_sub(max_files);
        let delete_count = expired.saturating_add(overflow);
        for (_, path) in candidates.into_iter().take(delete_count) {
            match std::fs::remove_file(path) {
                Ok(()) => report.deleted += 1,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("delete request body snapshot: {error}")),
            }
        }
        Ok(report)
    }

    fn path_for(&self, request_id: &str) -> Result<PathBuf, String> {
        if !valid_request_id(request_id) {
            return Err("invalid request id for body snapshot".to_owned());
        }
        Ok(self.directory.join(format!("{request_id}.json")))
    }
}

#[must_use]
pub fn valid_request_id(request_id: &str) -> bool {
    request_id.len() == 36
        && request_id.starts_with("req_")
        && request_id[4..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[must_use]
pub fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use super::{BodyLog, BoundedBody, HttpTraceSnapshot, MAX_BODY_BYTES, PlaintextExchange};

    fn temp_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "token-station-bodylog-{label}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn writes_reads_and_utf8_truncates_owner_only_snapshots() {
        let root = temp_dir("roundtrip");
        fs::remove_dir_all(&root).ok();
        let log = BodyLog::open(&root).expect("open body log");
        let request_id = "req_0123456789abcdef0123456789abcdef";
        let input = BoundedBody::from_bytes(br#"{"message":"visible prompt"}"#);
        let mut output = BoundedBody::default();
        output.push(&"你".repeat(MAX_BODY_BYTES));
        log.record(request_id, 42, input, output).expect("record");

        let value = log.read(request_id).expect("read").expect("exists");
        assert_eq!(value.request_id, request_id);
        assert!(value.input.contains("visible prompt"));
        assert!(value.output.is_char_boundary(value.output.len()));
        assert!(value.output_truncated);
        assert!(value.output.len() <= MAX_BODY_BYTES);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(log.directory()).unwrap().permissions().mode() & 0o777,
                0o700
            );
            let file = log.directory().join(format!("{request_id}.json"));
            assert_eq!(
                fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn rejects_path_like_ids_and_cleanup_ignores_non_snapshot_entries() {
        let root = temp_dir("cleanup");
        fs::remove_dir_all(&root).ok();
        let log = BodyLog::open(&root).expect("open body log");
        assert!(log.read("../../secrets").is_err());
        assert!(log.read("req_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").is_err());
        fs::write(log.directory().join("keep.txt"), "not a snapshot").unwrap();

        let request_id = "req_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let snapshot = PlaintextExchange {
            request_id: request_id.to_owned(),
            captured_at_ms: 1,
            input: String::new(),
            output: String::new(),
            input_truncated: false,
            output_truncated: false,
            http_trace: HttpTraceSnapshot::default(),
        };
        crate::private_fs::write_atomic_private(
            &log.directory().join(format!("{request_id}.json")),
            &serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();

        let report = log.cleanup(Duration::ZERO, 0).expect("cleanup");
        assert_eq!(report.scanned, 1);
        assert_eq!(report.deleted, 1);
        assert!(log.directory().join("keep.txt").exists());
        fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn read_refuses_a_snapshot_symlink() {
        use std::os::unix::fs::symlink;

        let root = temp_dir("read-symlink");
        fs::remove_dir_all(&root).ok();
        let log = BodyLog::open(&root).expect("open body log");
        let request_id = "req_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let outside = root.join("outside.json");
        fs::write(&outside, "{}").unwrap();
        symlink(&outside, log.directory().join(format!("{request_id}.json"))).unwrap();

        assert!(log.read(request_id).is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "{}");
        fs::remove_dir_all(root).ok();
    }
}

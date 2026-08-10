//! The always-on file log: one JSON line per exchange, size-rotated.
//!
//! Distinct from the metrics store on purpose. The store is queryable history
//! and can be switched off; this is the operational trace that is always
//! written and rotated unconditionally, so "the store was off" never means "we
//! are blind". Both write the same `RequestRecord`, so neither can carry what
//! the other could not.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use token_station_metrics::{Recorder, RequestRecord};

/// Rotate when the active file would pass this size.
const ROTATE_BYTES: u64 = 10 * 1024 * 1024;
/// `requests.log.1` .. `.N` are kept; older falls off.
const KEEP_ROTATED: u32 = 5;

/// A size-rotated JSON-lines log.
pub struct FileLog {
    path: PathBuf,
    file: Mutex<File>,
}

impl FileLog {
    /// Opens (or creates) `requests.log` inside `dir`.
    ///
    /// # Errors
    ///
    /// A message for the operator; the log failing to open is a startup error.
    pub fn open(dir: &std::path::Path) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("data dir `{}`: {error}", dir.display()))?;
        crate::private_fs::ensure_private_dir(dir)
            .map_err(|error| format!("data dir `{}`: {error}", dir.display()))?;
        let path = dir.join("requests.log");
        let file = private_append_options()
            .open(&path)
            .map_err(|error| format!("request log `{}`: {error}", path.display()))?;
        crate::private_fs::make_private(&path)
            .map_err(|error| format!("request log `{}`: {error}", path.display()))?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    fn write_line(&self, line: &str) -> std::io::Result<()> {
        let mut file = self.file.lock().expect("log lock");

        if file.metadata()?.len() + line.len() as u64 > ROTATE_BYTES {
            // requests.log.N-1 -> .N (dropping the oldest), log -> .1
            for index in (1..KEEP_ROTATED).rev() {
                let from = self.path.with_extension(format!("log.{index}"));
                let to = self.path.with_extension(format!("log.{}", index + 1));
                let _ = std::fs::rename(from, to);
            }
            std::fs::rename(&self.path, self.path.with_extension("log.1"))?;
            *file = private_append_options().open(&self.path)?;
        }

        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")
    }
}

fn private_append_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

impl Recorder for FileLog {
    fn record(&self, record: &RequestRecord) {
        let Ok(line) = serde_json::to_string(record) else {
            return;
        };
        if let Err(error) = self.write_line(&line) {
            eprintln!("request log write failed: {error}");
        }
    }
}

/// Fans one record out to every configured sink.
pub struct Recorders(pub Vec<Box<dyn Recorder>>);

impl Recorder for Recorders {
    fn record(&self, record: &RequestRecord) {
        for sink in &self.0 {
            sink.record(record);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FileLog, KEEP_ROTATED, ROTATE_BYTES};
    use token_station_metrics::{Recorder, RequestRecord};

    #[test]
    fn lines_append_and_rotation_keeps_a_bounded_set() {
        let dir = std::env::temp_dir().join(format!("ts-filelog-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();

        let log = FileLog::open(&dir).expect("opens");
        let mut record = RequestRecord::begin(0, "openai-chat-completions");
        // Big enough that a few writes cross the threshold.
        record.requested_model = "x".repeat(usize::try_from(ROTATE_BYTES / 4).expect("fits"));

        for _ in 0..6 {
            log.record(&record);
        }

        assert!(dir.join("requests.log").exists());
        assert!(dir.join("requests.log.1").exists(), "rotation happened");
        let rotated = (1..=KEEP_ROTATED + 2)
            .filter(|index| dir.join(format!("requests.log.{index}")).exists())
            .count();
        assert!(
            rotated <= KEEP_ROTATED as usize,
            "no more than {KEEP_ROTATED} rotated files may survive"
        );

        std::fs::remove_dir_all(dir).ok();
    }
}

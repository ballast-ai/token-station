//! The local client's configuration source: a file on disk.
//!
//! The other implementation of [`ConfigSource`] in this repository holds its
//! configuration in memory, and the one that matters most — the server
//! gateway's, reading a database — lives in the closed repository. None of them
//! shares any code with the others, and that is the point: what they share is
//! the document's shape, its validation, and what happens when a load fails, all
//! of which live in `router-core` where neither line can fork them.

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::PathBuf;

use token_station_router_core::{ConfigSource, RouterConfig};

/// Reads the routing configuration from a JSON file.
///
/// JSON rather than TOML only because it costs no dependency today. The format
/// is not part of any promise: `RouterConfig` is the schema, and a `serde`
/// front-end is a swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileConfigSource {
    path: PathBuf,
}

impl FileConfigSource {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl ConfigSource for FileConfigSource {
    type Error = FileConfigError;

    fn load(&self) -> Result<RouterConfig, Self::Error> {
        let source = fs::read_to_string(&self.path).map_err(|error| FileConfigError {
            path: self.path.clone(),
            detail: error.to_string(),
        })?;

        serde_json::from_str(&source).map_err(|error| FileConfigError {
            path: self.path.clone(),
            detail: error.to_string(),
        })
    }
}

/// The file could not be read, or is not a routing configuration.
///
/// One variant, deliberately: the operator's next action is the same either way
/// — open that path and look. Whether the configuration is *routable* is a
/// different question, answered by `router-core` with a reason of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileConfigError {
    path: PathBuf,
    detail: String,
}

impl fmt::Display for FileConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.detail)
    }
}

impl Error for FileConfigError {}

#[cfg(test)]
mod tests {
    use super::FileConfigSource;
    use std::fs;
    use std::path::PathBuf;
    use token_station_router_core::{ConfigCache, ConfigSource};

    fn scratch(name: &str, contents: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("token-station-{}-{name}.json", std::process::id()));
        fs::write(&path, contents).expect("the temp directory is writable");
        path
    }

    #[test]
    fn a_missing_file_names_the_path_it_looked_at() {
        let source = FileConfigSource::new("/nonexistent/router.json");

        let error = source.load().expect_err("no such file").to_string();

        assert!(error.contains("/nonexistent/router.json"), "{error}");
    }

    #[test]
    fn a_file_that_is_not_json_is_refused() {
        let path = scratch("garbage", "pools = { }");
        let source = FileConfigSource::new(&path);

        assert!(source.load().is_err());

        fs::remove_file(path).ok();
    }

    #[test]
    fn a_misspelled_field_is_refused_rather_than_ignored() {
        // `deny_unknown_fields` in `RouterConfig`. A silently dropped
        // `default_pool` would leave the router serving something nobody wrote.
        let path = scratch(
            "typo",
            r#"{"version":1,"pools":{"cheap":[{"upstream":"ollama_local","model":"llama3.3"}]},"default_pool":"cheap","defualt_pool":"sota"}"#,
        );
        let source = FileConfigSource::new(&path);

        let error = source.load().expect_err("unknown field").to_string();
        assert!(error.contains("defualt_pool"), "{error}");

        fs::remove_file(path).ok();
    }

    #[test]
    fn a_credential_pasted_into_an_upstream_name_is_refused_at_load() {
        let path = scratch(
            "credential",
            r#"{"version":1,"pools":{"cheap":[{"upstream":"sk-live-abc123","model":"gpt-5.5"}]},"default_pool":"cheap"}"#,
        );
        let source = FileConfigSource::new(&path);

        assert!(
            source.load().is_err(),
            "an upstream reference name cannot be a key"
        );

        fs::remove_file(path).ok();
    }

    #[test]
    fn a_well_formed_file_produces_a_router() {
        let path = scratch("good", crate::EXAMPLE_CONFIG);
        let cache = ConfigCache::load(FileConfigSource::new(&path)).expect("the example routes");

        assert_eq!(cache.current().config().default_pool, "cheap");

        fs::remove_file(path).ok();
    }

    #[test]
    fn an_unroutable_file_is_reported_as_invalid_not_as_unreadable() {
        // Parses fine; names a pool that does not exist. The distinction matters
        // to whoever has to fix it: retrying will not help.
        let path = scratch(
            "unroutable",
            r#"{"version":1,"pools":{"cheap":[{"upstream":"ollama_local","model":"llama3.3"}]},"default_pool":"sota"}"#,
        );

        let error = ConfigCache::load(FileConfigSource::new(&path)).expect_err("no pool `sota`");

        assert!(
            matches!(error, token_station_router_core::CacheError::Invalid(_)),
            "{error}"
        );

        fs::remove_file(path).ok();
    }
}

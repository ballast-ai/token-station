use std::convert::Infallible;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use crate::config::{ConfigError, RouterConfig};
use crate::route::Router;

/// Where a router's configuration comes from.
///
/// Two implementations exist from the first day, on purpose: the local client
/// reads a file, the server gateway reads a database. The point of the trait is
/// not polymorphism — neither binary loads the other's source — but that the
/// *shape* of a configuration, its validation, and what happens when a load
/// fails are decided once, here, instead of twice and differently.
///
/// # Not async, and not on the request path
///
/// `load` may block. It is never called while a request is in flight:
/// [`ConfigCache`] holds the last configuration that validated, and something
/// outside this crate — a file watcher, a poller, a CLI `reload` — calls
/// [`ConfigCache::refresh`].
///
/// That is what keeps the data plane from depending on the config plane. It is
/// also why a database source does not drag an async runtime into a crate that
/// has to run inside a CLI with none: the server does its `await` in its own
/// task and hands the result to `refresh`.
pub trait ConfigSource {
    type Error: Error + Send + Sync + 'static;

    /// Fetches the current configuration document.
    ///
    /// # Errors
    ///
    /// Whatever the backing store failed with. A failure is not fatal; see
    /// [`ConfigCache::refresh`].
    fn load(&self) -> Result<RouterConfig, Self::Error>;
}

/// A configuration held in memory.
///
/// The stand-in for the server's database source in this repository, and the way
/// tests get a router without touching a disk. Its existence is what proves the
/// trait is not file-shaped: [`ConfigSource`] cannot assume a path, because this
/// implementation has none.
#[derive(Debug, Clone)]
pub struct StaticConfigSource {
    config: RouterConfig,
}

impl StaticConfigSource {
    #[must_use]
    pub fn new(config: RouterConfig) -> Self {
        Self { config }
    }
}

impl ConfigSource for StaticConfigSource {
    type Error = Infallible;

    fn load(&self) -> Result<RouterConfig, Self::Error> {
        Ok(self.config.clone())
    }
}

/// The last configuration that worked, and a way to try for a newer one.
///
/// A refresh that fails — the config service is unreachable, the file was
/// deleted mid-write, a remote profile is malformed — leaves the previous
/// configuration serving. This type implements the requirement to keep using
/// cached configuration and continue the data plane when the cloud is unreachable.
///
/// The asymmetry with the first load is deliberate. Starting with no
/// configuration at all is a fatal misconfiguration and the process should say
/// so; losing contact with the config plane after a good start is an outage of
/// something the request path was never allowed to depend on.
#[derive(Debug, Clone)]
pub struct ConfigCache<S> {
    source: S,
    current: Arc<Router>,
}

impl<S: ConfigSource> ConfigCache<S> {
    /// Loads and validates once. This one must succeed.
    ///
    /// # Errors
    ///
    /// [`CacheError`] naming whether the source or the configuration was at
    /// fault.
    pub fn load(source: S) -> Result<Self, CacheError<S::Error>> {
        let router = Self::fetch(&source)?;
        Ok(Self {
            source,
            current: Arc::new(router),
        })
    }

    /// The configuration currently serving requests. Never fails, never blocks.
    #[must_use]
    pub fn current(&self) -> &Arc<Router> {
        &self.current
    }

    /// Tries to replace the current configuration.
    ///
    /// On any failure the previous configuration keeps serving, and the reason is
    /// returned for the caller to log. A router that fell back is
    /// indistinguishable, to a request, from one that never tried.
    ///
    /// # Errors
    ///
    /// [`CacheError`], having changed nothing.
    pub fn refresh(&mut self) -> Result<(), CacheError<S::Error>> {
        let router = Self::fetch(&self.source)?;
        self.current = Arc::new(router);
        Ok(())
    }

    fn fetch(source: &S) -> Result<Router, CacheError<S::Error>> {
        let config = source.load().map_err(CacheError::Source)?;
        Router::new(config).map_err(CacheError::Invalid)
    }
}

/// Why a configuration could not be installed.
///
/// The two arms are operationally different. `Source` is an outage: retry.
/// `Invalid` is a bad document: retrying will not help, and somebody has to fix
/// the profile that was pushed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheError<E> {
    Source(E),
    Invalid(ConfigError),
}

impl<E: fmt::Display> fmt::Display for CacheError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(source) => write!(f, "config source failed: {source}"),
            Self::Invalid(error) => write!(f, "configuration is not routable: {error}"),
        }
    }
}

impl<E: Error + 'static> Error for CacheError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Source(error) => Some(error),
            Self::Invalid(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CacheError, ConfigCache, ConfigSource, StaticConfigSource};
    use crate::config::{ConfigError, RouterConfig};
    use crate::test_support::config;
    use std::cell::Cell;
    use std::error::Error;
    use std::fmt;

    #[derive(Debug)]
    struct Offline;

    impl fmt::Display for Offline {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("the config service is unreachable")
        }
    }

    impl Error for Offline {}

    /// Serves a good config once, then whatever the test asked for.
    struct Flaky {
        calls: Cell<u32>,
        later: fn() -> Result<RouterConfig, Offline>,
    }

    impl ConfigSource for Flaky {
        type Error = Offline;

        fn load(&self) -> Result<RouterConfig, Self::Error> {
            let call = self.calls.get();
            self.calls.set(call + 1);
            if call == 0 {
                return Ok(config());
            }
            (self.later)()
        }
    }

    fn flaky(later: fn() -> Result<RouterConfig, Offline>) -> Flaky {
        Flaky {
            calls: Cell::new(0),
            later,
        }
    }

    #[test]
    fn a_static_source_needs_no_filesystem() {
        let cache = ConfigCache::load(StaticConfigSource::new(config())).expect("valid config");

        assert_eq!(cache.current().config().default_pool, "cheap");
    }

    #[test]
    fn the_first_load_must_succeed() {
        let mut broken = config();
        broken.default_pool = "nowhere".to_owned();

        let error = ConfigCache::load(StaticConfigSource::new(broken))
            .expect_err("an unroutable config must not start a router");

        assert!(matches!(
            error,
            CacheError::Invalid(ConfigError::UnknownPool { .. })
        ));
    }

    #[test]
    fn an_unreachable_source_leaves_the_previous_config_serving() {
        let mut cache = ConfigCache::load(flaky(|| Err(Offline))).expect("first load");
        let before = cache.current().clone();

        let error = cache.refresh().expect_err("the source is down");

        assert!(matches!(error, CacheError::Source(Offline)));
        assert_eq!(
            *cache.current().clone(),
            *before,
            "the data plane kept serving"
        );
    }

    #[test]
    fn a_malformed_remote_profile_cannot_take_the_data_plane_down() {
        let mut cache = ConfigCache::load(flaky(|| {
            let mut broken = config();
            broken.rules[0].route_to = "gpu-farm".to_owned();
            Ok(broken)
        }))
        .expect("first load");
        let before = cache.current().clone();

        let error = cache.refresh().expect_err("the profile names no such pool");

        assert!(matches!(error, CacheError::Invalid(_)));
        assert_eq!(*cache.current().clone(), *before);
    }

    #[test]
    fn a_good_refresh_replaces_the_config() {
        let mut cache = ConfigCache::load(flaky(|| {
            let mut newer = config();
            newer.default_pool = "sota".to_owned();
            Ok(newer)
        }))
        .expect("first load");

        assert_eq!(cache.current().config().default_pool, "cheap");
        cache.refresh().expect("the newer profile validates");
        assert_eq!(cache.current().config().default_pool, "sota");
    }
}

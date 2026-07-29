use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use wasmtime::{Config, Engine};

/// What a guest may consume before it is cut off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLimits {
    /// Ceiling on a store's linear memory. A guest allocation that would grow
    /// past it fails inside the guest, which typically traps.
    pub memory_bytes: usize,
    /// Wall-clock deadline for one guest call. A guest still running at the
    /// deadline traps at its next epoch check.
    pub call_timeout: Duration,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            // Generous for protocol translation, far below anything that could
            // pressure the host. An adapter is a codec, not a database.
            memory_bytes: 64 * 1024 * 1024,
            call_timeout: Duration::from_secs(2),
        }
    }
}

/// How often the ticker thread advances the engine epoch. One tick is the
/// resolution of every call deadline.
pub(crate) const EPOCH_TICK: Duration = Duration::from_millis(10);
const MAX_STREAM_INSTANCES: usize = 64;

/// A shared engine, its epoch ticker, and the limits applied to every store.
///
/// One of these per process is the intent; plugins loaded from it share JIT
/// caches and the single ticker thread.
#[derive(Clone)]
pub struct PluginRuntime {
    engine: Engine,
    limits: RuntimeLimits,
    active_streams: Arc<AtomicUsize>,
    // Held so the ticker stops when the last clone drops.
    _ticker: Arc<TickerGuard>,
}

impl PluginRuntime {
    /// Builds an engine with epoch interruption on and starts the ticker.
    ///
    /// # Errors
    ///
    /// Returns the engine construction error, which on a supported target
    /// means the configuration itself was rejected.
    pub fn new(limits: RuntimeLimits) -> wasmtime::Result<Self> {
        let mut config = Config::new();
        config.epoch_interruption(true);
        config.wasm_component_model(true);
        let engine = Engine::new(&config)?;

        let ticker = TickerGuard::start(engine.clone());

        Ok(Self {
            engine,
            limits,
            active_streams: Arc::new(AtomicUsize::new(0)),
            _ticker: Arc::new(ticker),
        })
    }

    pub(crate) fn engine(&self) -> &Engine {
        &self.engine
    }

    pub(crate) fn limits(&self) -> RuntimeLimits {
        self.limits
    }

    /// The number of epoch ticks equivalent to the configured call timeout,
    /// rounded up and never zero.
    pub(crate) fn deadline_ticks(&self) -> u64 {
        let ticks = self
            .limits
            .call_timeout
            .as_millis()
            .div_ceil(EPOCH_TICK.as_millis());
        u64::try_from(ticks).unwrap_or(u64::MAX).max(1)
    }

    pub(crate) fn try_acquire_stream(&self) -> Option<StreamPermit> {
        let acquired = self
            .active_streams
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_STREAM_INSTANCES).then(|| active + 1)
            })
            .is_ok();
        acquired.then(|| StreamPermit {
            active: Arc::clone(&self.active_streams),
        })
    }
}

pub(crate) struct StreamPermit {
    active: Arc<AtomicUsize>,
}

impl Drop for StreamPermit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

impl std::fmt::Debug for PluginRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginRuntime")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

/// Advances the engine epoch until dropped.
struct TickerGuard {
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl TickerGuard {
    fn start(engine: Engine) -> Self {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = Arc::clone(&stop);

        std::thread::Builder::new()
            .name("plugin-epoch-ticker".to_owned())
            .spawn(move || {
                while !observed.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(EPOCH_TICK);
                    engine.increment_epoch();
                }
            })
            .expect("spawning a thread only fails when the process is already dying");

        Self { stop }
    }
}

impl Drop for TickerGuard {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_STREAM_INSTANCES, PluginRuntime, RuntimeLimits};
    use std::time::Duration;

    #[test]
    fn deadline_ticks_round_up_and_never_reach_zero() {
        let runtime = PluginRuntime::new(RuntimeLimits {
            call_timeout: Duration::from_millis(1),
            ..RuntimeLimits::default()
        })
        .expect("engine builds");

        assert_eq!(runtime.deadline_ticks(), 1);

        let runtime = PluginRuntime::new(RuntimeLimits {
            call_timeout: Duration::from_millis(25),
            ..RuntimeLimits::default()
        })
        .expect("engine builds");

        assert_eq!(runtime.deadline_ticks(), 3, "25ms of 10ms ticks rounds up");
    }

    #[test]
    fn stream_instances_share_one_process_wide_budget_across_clones() {
        let runtime = PluginRuntime::new(RuntimeLimits::default()).expect("engine builds");
        let clone = runtime.clone();
        let permits: Vec<_> = (0..MAX_STREAM_INSTANCES)
            .map(|_| clone.try_acquire_stream().expect("within stream budget"))
            .collect();
        assert!(runtime.try_acquire_stream().is_none());
        drop(permits);
        assert!(runtime.try_acquire_stream().is_some());
    }
}

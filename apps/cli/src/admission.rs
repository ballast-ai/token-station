//! Concurrency admission: a bound on how many requests are in flight at once,
//! at three levels — global, per Agent, and per upstream Provider.
//!
//! Without it, forty simultaneous clients become forty simultaneous upstream
//! requests and nothing pushes back. Each level is a counting gate: entering
//! takes a permit, the permit is released when the returned guard drops, and a
//! request that would exceed a limit is refused with a protocol 429 rather than
//! silently piling on. Every default is finite; `0` is invalid and fails
//! closed rather than turning a missing or misspelled limit into "unlimited".
//!
//! Sync on purpose: the data plane runs on a blocking thread, so the gate is a
//! plain atomic, usable from that thread without a runtime.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

const DEFAULT_GLOBAL: u32 = 64;
const DEFAULT_PER_AGENT: u32 = 16;
const DEFAULT_PER_PROVIDER: u32 = 16;

const fn default_global() -> u32 {
    DEFAULT_GLOBAL
}

const fn default_per_agent() -> u32 {
    DEFAULT_PER_AGENT
}

const fn default_per_provider() -> u32 {
    DEFAULT_PER_PROVIDER
}

/// The per-level ceilings. Every value must be greater than zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(default = "default_global")]
    pub global: u32,
    #[serde(default = "default_per_agent")]
    pub per_agent: u32,
    #[serde(default = "default_per_provider")]
    pub per_provider: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            global: DEFAULT_GLOBAL,
            per_agent: DEFAULT_PER_AGENT,
            per_provider: DEFAULT_PER_PROVIDER,
        }
    }
}

/// A held permit. Dropping it returns the permit to its counter.
#[derive(Debug)]
pub struct Permit {
    counter: Option<Arc<AtomicU32>>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        if let Some(counter) = &self.counter {
            counter.fetch_sub(1, Ordering::Release);
        }
    }
}

/// Tries to take one unit against `limit`. `None` means the limit is full — the
/// caller's cue to refuse with 429.
fn try_take(counter: &Arc<AtomicU32>, limit: u32) -> Option<Permit> {
    if limit == 0 {
        return None;
    }
    let mut current = counter.load(Ordering::Acquire);
    loop {
        if current >= limit {
            return None;
        }
        match counter.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                return Some(Permit {
                    counter: Some(Arc::clone(counter)),
                });
            }
            Err(observed) => current = observed,
        }
    }
}

/// The three-level admission gate.
#[derive(Debug)]
pub struct Admission {
    limits: Limits,
    global: Arc<AtomicU32>,
    agents: Mutex<BTreeMap<String, Arc<AtomicU32>>>,
    providers: Mutex<BTreeMap<String, Arc<AtomicU32>>>,
}

impl Admission {
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            global: Arc::new(AtomicU32::new(0)),
            agents: Mutex::new(BTreeMap::new()),
            providers: Mutex::new(BTreeMap::new()),
        }
    }

    #[must_use]
    pub const fn global_limit(&self) -> u32 {
        self.limits.global
    }

    /// A permit against the global ceiling.
    pub fn enter_global(&self) -> Option<Permit> {
        try_take(&self.global, self.limits.global)
    }

    /// A permit against a single Agent's ceiling.
    pub fn enter_agent(&self, agent_id: &str) -> Option<Permit> {
        try_take(&counter(&self.agents, agent_id), self.limits.per_agent)
    }

    /// A permit against a single Provider's ceiling.
    pub fn enter_provider(&self, upstream: &str) -> Option<Permit> {
        try_take(
            &counter(&self.providers, upstream),
            self.limits.per_provider,
        )
    }
}

/// The counter for one key, created on first use.
fn counter(map: &Mutex<BTreeMap<String, Arc<AtomicU32>>>, key: &str) -> Arc<AtomicU32> {
    let mut map = map.lock().expect("admission lock");
    Arc::clone(
        map.entry(key.to_owned())
            .or_insert_with(|| Arc::new(AtomicU32::new(0))),
    )
}

#[cfg(test)]
mod tests {
    use super::{Admission, Limits};

    #[test]
    fn default_limits_bound_every_admission_level() {
        let limits = Limits::default();
        assert!(limits.global > 0);
        assert!(limits.per_agent > 0);
        assert!(limits.per_provider > 0);
    }

    #[test]
    fn the_default_gate_is_finite_and_admits_normal_load() {
        let admission = Admission::new(Limits::default());
        let permits: Vec<_> = (0..Limits::default().global)
            .map(|_| admission.enter_global())
            .collect();
        assert!(permits.iter().all(Option::is_some));
        assert!(admission.enter_global().is_none());
    }

    #[test]
    fn the_global_ceiling_refuses_the_one_over() {
        let admission = Admission::new(Limits {
            global: 2,
            ..Limits::default()
        });
        let _a = admission.enter_global().expect("first admits");
        let _b = admission.enter_global().expect("second admits");
        assert!(
            admission.enter_global().is_none(),
            "third is over the limit"
        );
    }

    #[test]
    fn dropping_a_permit_frees_a_slot() {
        let admission = Admission::new(Limits {
            global: 1,
            ..Limits::default()
        });
        {
            let _a = admission.enter_global().expect("admits");
            assert!(admission.enter_global().is_none(), "full");
        }
        assert!(
            admission.enter_global().is_some(),
            "the slot came back when the guard dropped"
        );
    }

    #[test]
    fn agents_and_providers_are_counted_independently() {
        let admission = Admission::new(Limits {
            per_agent: 1,
            per_provider: 1,
            ..Limits::default()
        });
        let _codex = admission.enter_agent("codex").expect("codex admits");
        // A different agent has its own ceiling.
        let _claude = admission.enter_agent("claude-code").expect("claude admits");
        // The same agent is now full.
        assert!(admission.enter_agent("codex").is_none());

        let _openai = admission.enter_provider("openai").expect("provider admits");
        assert!(admission.enter_provider("openai").is_none());
        assert!(
            admission.enter_provider("anthropic").is_some(),
            "a different provider is independent"
        );
    }
}

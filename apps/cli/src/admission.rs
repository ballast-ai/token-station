//! Concurrency admission: a bound on how many requests are in flight at once,
//! at three levels — global, per Agent, and per upstream Provider.
//!
//! Without it, forty simultaneous clients become forty simultaneous upstream
//! requests and nothing pushes back. Each level is a counting gate: entering
//! takes a permit, the permit is released when the returned guard drops, and a
//! request that would exceed a limit is refused with a protocol 429 rather than
//! silently piling on. A limit of `0` means "unlimited" — the default, so an
//! unconfigured deployment behaves exactly as before.
//!
//! Sync on purpose: the data plane runs on a blocking thread, so the gate is a
//! plain atomic, usable from that thread without a runtime.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

/// The per-level ceilings. `0` is unlimited (the default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(default)]
    pub global: u32,
    #[serde(default)]
    pub per_agent: u32,
    #[serde(default)]
    pub per_provider: u32,
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
        // Unlimited: hand back a permit that counts nothing.
        return Some(Permit { counter: None });
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
    fn an_unlimited_gate_never_refuses() {
        let admission = Admission::new(Limits::default());
        let permits: Vec<_> = (0..1000).map(|_| admission.enter_global()).collect();
        assert!(permits.iter().all(Option::is_some));
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

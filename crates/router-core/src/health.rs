use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use token_station_protocol::ErrorCode;

use crate::decision::UpstreamRef;
use crate::route::Health;

/// When an upstream is taken out of rotation, and for how long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthPolicy {
    /// Consecutive countable failures before ejection.
    pub eject_after: u32,
    /// How long an ejected upstream stays [`Health::Unavailable`].
    pub cooldown: Duration,
}

impl Default for HealthPolicy {
    fn default() -> Self {
        Self {
            eject_after: 3,
            cooldown: Duration::from_secs(30),
        }
    }
}

/// Per-upstream failure counting: the simplified health check from C1#2.
///
/// Timestamps are inputs, never read from a clock — same discipline as
/// [`crate::Router::route`], and for the same reason: the state machine must
/// behave identically under test and in the two production lines, and a test
/// that has to *wait* for a cooldown proves less than one that hands in a
/// later timestamp.
///
/// # Which failures count
///
/// Only the ones that say the upstream is *unwell*: [`ErrorCode::Timeout`],
/// [`ErrorCode::UpstreamUnavailable`], [`ErrorCode::Capacity`]. Deliberately
/// not counted:
///
/// - [`ErrorCode::Auth`] — a rejected credential is a configuration problem.
///   Ejecting on it would mask a typo in a key file as an outage, and the
///   operator would see "upstream down" instead of "fix your key".
/// - [`ErrorCode::RateLimit`] — an upstream that answers 429 is alive and
///   saying so; ejecting it turns a per-minute quota into a 30-second outage.
/// - Everything non-retriable: those describe the request, not the upstream.
///
/// # The recovery path
///
/// Ejection expires into [`Health::Degraded`], not `Healthy`: the router
/// prefers healthy candidates and uses degraded ones only when nothing better
/// remains, so the first request after cooldown is the recovery probe —
/// real traffic, sent only when the alternative is failing anyway. One success
/// restores `Healthy`; one countable failure re-ejects immediately (a probe
/// that fails needs no second opinion).
#[derive(Debug, Default)]
pub struct HealthTracker {
    policy: HealthPolicy,
    states: BTreeMap<UpstreamRef, State>,
}

#[derive(Debug, Clone, Copy)]
enum State {
    /// Counting toward ejection.
    Counting { consecutive: u32 },
    /// Out of rotation until the instant passes; probation afterwards.
    Ejected { until: Instant },
}

impl HealthTracker {
    #[must_use]
    pub fn new(policy: HealthPolicy) -> Self {
        Self {
            policy,
            states: BTreeMap::new(),
        }
    }

    /// The upstream answered. One success clears probation and the counter.
    pub fn observe_success(&mut self, upstream: &UpstreamRef) {
        self.states.remove(upstream);
    }

    /// The upstream failed with `code`. Returns `true` when this observation
    /// ejected it — the caller's cue to log the transition.
    pub fn observe_failure(
        &mut self,
        upstream: &UpstreamRef,
        code: ErrorCode,
        now: Instant,
    ) -> bool {
        if !counts_toward_ejection(code) {
            return false;
        }

        let consecutive = match self.states.get(upstream) {
            Some(State::Counting { consecutive }) => consecutive + 1,
            // A failed probe re-ejects at once: it already proved the point.
            Some(State::Ejected { until }) if now >= *until => self.policy.eject_after,
            // Failures while already ejected change nothing.
            Some(State::Ejected { .. }) => return false,
            None => 1,
        };

        if consecutive >= self.policy.eject_after {
            self.states.insert(
                upstream.clone(),
                State::Ejected {
                    until: now + self.policy.cooldown,
                },
            );
            true
        } else {
            self.states
                .insert(upstream.clone(), State::Counting { consecutive });
            false
        }
    }

    /// The health the router should see for `upstream`, as of `now`.
    #[must_use]
    pub fn health_of(&self, upstream: &UpstreamRef, now: Instant) -> Health {
        match self.states.get(upstream) {
            None | Some(State::Counting { .. }) => Health::Healthy,
            Some(State::Ejected { until }) if now < *until => Health::Unavailable,
            // Cooldown over: usable, but only when nothing healthy remains.
            Some(State::Ejected { .. }) => Health::Degraded,
        }
    }
}

/// See the type-level docs for why this set is exactly these three.
fn counts_toward_ejection(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::Timeout | ErrorCode::UpstreamUnavailable | ErrorCode::Capacity
    )
}

#[cfg(test)]
mod tests {
    use super::{HealthPolicy, HealthTracker};
    use crate::decision::UpstreamRef;
    use crate::route::Health;
    use std::time::{Duration, Instant};
    use token_station_protocol::ErrorCode;

    fn upstream() -> UpstreamRef {
        UpstreamRef::new("openai_personal").expect("valid name")
    }

    fn tracker() -> HealthTracker {
        HealthTracker::new(HealthPolicy {
            eject_after: 3,
            cooldown: Duration::from_secs(30),
        })
    }

    #[test]
    fn ejection_takes_exactly_the_configured_run_of_failures() {
        let mut tracker = tracker();
        let now = Instant::now();
        let target = upstream();

        assert!(!tracker.observe_failure(&target, ErrorCode::Timeout, now));
        assert!(!tracker.observe_failure(&target, ErrorCode::UpstreamUnavailable, now));
        assert_eq!(
            tracker.health_of(&target, now),
            Health::Healthy,
            "two is not three"
        );

        assert!(
            tracker.observe_failure(&target, ErrorCode::Capacity, now),
            "the third ejects, and says so"
        );
        assert_eq!(tracker.health_of(&target, now), Health::Unavailable);
    }

    #[test]
    fn one_success_resets_the_run() {
        let mut tracker = tracker();
        let now = Instant::now();
        let target = upstream();

        tracker.observe_failure(&target, ErrorCode::Timeout, now);
        tracker.observe_failure(&target, ErrorCode::Timeout, now);
        tracker.observe_success(&target);
        tracker.observe_failure(&target, ErrorCode::Timeout, now);

        assert_eq!(
            tracker.health_of(&target, now),
            Health::Healthy,
            "the counter is consecutive, not cumulative"
        );
    }

    #[test]
    fn a_rejected_credential_is_a_config_problem_not_an_outage() {
        let mut tracker = tracker();
        let now = Instant::now();
        let target = upstream();

        for _ in 0..10 {
            assert!(!tracker.observe_failure(&target, ErrorCode::Auth, now));
        }
        assert_eq!(tracker.health_of(&target, now), Health::Healthy);
    }

    #[test]
    fn a_rate_limited_upstream_is_alive_and_saying_so() {
        let mut tracker = tracker();
        let now = Instant::now();
        let target = upstream();

        for _ in 0..10 {
            tracker.observe_failure(&target, ErrorCode::RateLimit, now);
        }
        assert_eq!(
            tracker.health_of(&target, now),
            Health::Healthy,
            "429 must not become a 30-second outage"
        );
    }

    #[test]
    fn cooldown_expires_into_probation_not_health() {
        let mut tracker = tracker();
        let now = Instant::now();
        let target = upstream();

        for _ in 0..3 {
            tracker.observe_failure(&target, ErrorCode::Timeout, now);
        }
        let during = now + Duration::from_secs(10);
        let after = now + Duration::from_secs(31);

        assert_eq!(tracker.health_of(&target, during), Health::Unavailable);
        assert_eq!(
            tracker.health_of(&target, after),
            Health::Degraded,
            "the first request after cooldown is the probe, not business as usual"
        );
    }

    #[test]
    fn a_probe_settles_it_one_way_or_the_other() {
        let mut tracker = tracker();
        let now = Instant::now();
        let target = upstream();

        for _ in 0..3 {
            tracker.observe_failure(&target, ErrorCode::Timeout, now);
        }
        let after = now + Duration::from_secs(31);

        // A successful probe restores full health.
        let mut recovered = tracker;
        recovered.observe_success(&target);
        assert_eq!(recovered.health_of(&target, after), Health::Healthy);

        // A failed probe re-ejects immediately — no second run of three.
        let mut relapsed = HealthTracker::new(HealthPolicy {
            eject_after: 3,
            cooldown: Duration::from_secs(30),
        });
        for _ in 0..3 {
            relapsed.observe_failure(&target, ErrorCode::Timeout, now);
        }
        assert!(
            relapsed.observe_failure(&target, ErrorCode::Timeout, after),
            "one failed probe is proof enough"
        );
        assert_eq!(
            relapsed.health_of(&target, after + Duration::from_secs(1)),
            Health::Unavailable
        );
    }

    #[test]
    fn failures_while_ejected_do_not_extend_the_cooldown() {
        let mut tracker = tracker();
        let now = Instant::now();
        let target = upstream();

        for _ in 0..3 {
            tracker.observe_failure(&target, ErrorCode::Timeout, now);
        }
        // A straggler (e.g. an in-flight request) fails during cooldown.
        tracker.observe_failure(&target, ErrorCode::Timeout, now + Duration::from_secs(5));

        assert_eq!(
            tracker.health_of(&target, now + Duration::from_secs(31)),
            Health::Degraded,
            "the original cooldown still governs"
        );
    }
}

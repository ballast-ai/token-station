//! In-flight leases: the concurrency half of quota-first routing.
//!
//! The router ranks accounts on a snapshot. Under concurrency that snapshot
//! goes stale between requests: N requests can all pick the same soonest-closing
//! account before any of them completes and shows up in its consumption, and the
//! account is over-sent past its window. A lease closes that gap.
//!
//! When a request is dispatched to an account it takes a lease; when the request
//! settles the lease is released. A lease also carries an expiry, so a request
//! that crashes or hangs cannot leak an in-flight count forever — the count self-
//! heals after [`DEFAULT_LEASE_MS`]. The live in-flight count is then folded back
//! into the account's rate headroom ([`apply_inflight_penalty`]) *before* the
//! next ranking, so a filling account looks less free and the router spreads the
//! next request elsewhere. That feedback is all the fairness this case needs:
//! two-random-choice-style spreading falls out of it without a weighted
//! allocator, because the accounts are all one user's own plans, not tenants
//! contending for a shared pool.
//!
//! Clock-injected: every method takes `now_ms` and reads no clock, so behavior
//! is a pure function of the call sequence and unit-testable.

use std::collections::HashMap;

/// A lease expires this long after it is granted, in case the request holding it
/// never settles. Matches `OmniRoute`'s 120-second in-flight lease.
pub const DEFAULT_LEASE_MS: u64 = 120_000;

/// Each in-flight request docks this much rate headroom (permille) when the
/// account's per-minute rate limit is unknown, so spreading still happens
/// without a declared limit. Ten concurrent requests saturate the headroom.
pub const DEFAULT_INFLIGHT_DOCK_PERMILLE: u32 = 100;

/// A handle to one granted lease. Returned by [`InflightLeases::grant`] and
/// handed back to [`InflightLeases::release`] when the request settles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseId {
    account: String,
    serial: u64,
}

/// Per-account in-flight lease tracking with self-expiring leases.
#[derive(Debug, Default)]
pub struct InflightLeases {
    lease_ms: u64,
    next_serial: u64,
    /// account key → (lease serial → expiry timestamp).
    by_account: HashMap<String, HashMap<u64, u64>>,
}

impl InflightLeases {
    /// A tracker whose leases expire after `lease_ms` (use [`DEFAULT_LEASE_MS`]).
    #[must_use]
    pub fn new(lease_ms: u64) -> Self {
        Self {
            lease_ms: lease_ms.max(1),
            next_serial: 0,
            by_account: HashMap::new(),
        }
    }

    /// Take a lease on `account` at `now_ms`. Drop expired leases on the account
    /// first so a long-lived key does not accumulate dead entries.
    pub fn grant(&mut self, now_ms: u64, account: &str) -> LeaseId {
        let expiry = now_ms.saturating_add(self.lease_ms);
        let serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1);
        let leases = self.by_account.entry(account.to_owned()).or_default();
        leases.retain(|_, &mut expires_at| expires_at > now_ms);
        leases.insert(serial, expiry);
        LeaseId {
            account: account.to_owned(),
            serial,
        }
    }

    /// Release a lease when its request settles. Idempotent: releasing an
    /// already-expired or unknown lease is a no-op.
    pub fn release(&mut self, lease: &LeaseId) {
        if let Some(leases) = self.by_account.get_mut(&lease.account) {
            leases.remove(&lease.serial);
            if leases.is_empty() {
                self.by_account.remove(&lease.account);
            }
        }
    }

    /// Live in-flight count for `account` as of `now_ms` — leases granted and not
    /// yet released or expired.
    #[must_use]
    pub fn inflight(&self, now_ms: u64, account: &str) -> u32 {
        self.by_account.get(account).map_or(0, |leases| {
            let live = leases
                .values()
                .filter(|&&expires_at| expires_at > now_ms)
                .count();
            u32::try_from(live).unwrap_or(u32::MAX)
        })
    }
}

/// Fold an account's in-flight load back into its rate headroom. `inflight`
/// requests are impending consumption not yet in the rate window, so they
/// pre-emptively lower headroom and steer the next request elsewhere.
///
/// With a known per-minute rate limit the penalty is the fraction of that budget
/// already committed in-flight; without one, each in-flight request docks a flat
/// [`DEFAULT_INFLIGHT_DOCK_PERMILLE`]. Headroom floors at zero.
#[must_use]
pub fn apply_inflight_penalty(
    headroom_permille: u16,
    inflight: u32,
    rate_limit_per_min: Option<u64>,
) -> u16 {
    let penalty = match rate_limit_per_min {
        Some(limit) if limit > 0 => u32::try_from(u64::from(inflight).saturating_mul(1000) / limit)
            .unwrap_or(1000)
            .min(1000),
        _ => inflight
            .saturating_mul(DEFAULT_INFLIGHT_DOCK_PERMILLE)
            .min(1000),
    };
    let penalty = u16::try_from(penalty).unwrap_or(1000);
    headroom_permille.saturating_sub(penalty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_granted_lease_is_counted_until_released() {
        let mut leases = InflightLeases::new(DEFAULT_LEASE_MS);
        assert_eq!(leases.inflight(0, "acct"), 0);
        let a = leases.grant(0, "acct");
        let b = leases.grant(0, "acct");
        assert_eq!(leases.inflight(0, "acct"), 2);
        leases.release(&a);
        assert_eq!(leases.inflight(0, "acct"), 1);
        leases.release(&b);
        assert_eq!(leases.inflight(0, "acct"), 0);
    }

    #[test]
    fn leases_are_scoped_per_account() {
        let mut leases = InflightLeases::new(DEFAULT_LEASE_MS);
        leases.grant(0, "a");
        leases.grant(0, "b");
        leases.grant(0, "b");
        assert_eq!(leases.inflight(0, "a"), 1);
        assert_eq!(leases.inflight(0, "b"), 2);
    }

    #[test]
    fn an_unreleased_lease_expires_so_the_count_self_heals() {
        let mut leases = InflightLeases::new(DEFAULT_LEASE_MS);
        leases.grant(1_000, "acct"); // never released (request hung)
        assert_eq!(leases.inflight(1_000, "acct"), 1);
        // Still in flight just before expiry…
        assert_eq!(leases.inflight(1_000 + DEFAULT_LEASE_MS - 1, "acct"), 1);
        // …and gone at expiry, without anyone releasing it.
        assert_eq!(leases.inflight(1_000 + DEFAULT_LEASE_MS, "acct"), 0);
    }

    #[test]
    fn releasing_an_unknown_lease_is_a_noop() {
        let mut leases = InflightLeases::new(DEFAULT_LEASE_MS);
        let ghost = LeaseId {
            account: "acct".to_owned(),
            serial: 999,
        };
        leases.release(&ghost); // must not panic
        assert_eq!(leases.inflight(0, "acct"), 0);
    }

    #[test]
    fn inflight_penalty_scales_with_a_known_rate_limit() {
        // 10 req/min: 3 in flight ⇒ 30% of the budget committed ⇒ 300‰ off.
        assert_eq!(apply_inflight_penalty(1000, 3, Some(10)), 700);
        // Enough in flight to exceed the budget floors headroom at zero.
        assert_eq!(apply_inflight_penalty(1000, 20, Some(10)), 0);
    }

    #[test]
    fn inflight_penalty_uses_a_flat_dock_without_a_known_limit() {
        // Default dock is 100‰ each.
        assert_eq!(apply_inflight_penalty(1000, 3, None), 700);
        assert_eq!(apply_inflight_penalty(500, 3, None), 200);
        // Saturates without underflowing.
        assert_eq!(apply_inflight_penalty(200, 50, None), 0);
    }

    // ── P0② concurrency / no-oversend ────────────────────────────────────────

    #[test]
    fn concurrent_grant_release_leaks_no_in_flight_count() {
        // The gateway holds one QuotaTracker (hence one InflightLeases) behind a
        // Mutex and hits it from many request threads. Every grant that settles
        // must release cleanly, whatever the interleaving, so no account is left
        // with a phantom in-flight count that would wrongly steer routing away
        // from it forever. Global serials must stay unique so a release never
        // removes a different request's lease.
        use std::sync::{Arc, Mutex};
        use std::thread;

        let leases = Arc::new(Mutex::new(InflightLeases::new(DEFAULT_LEASE_MS)));
        let threads = 16;
        let per_thread = 500;
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let leases = Arc::clone(&leases);
                thread::spawn(move || {
                    // 16 threads contend over 4 shared accounts.
                    let account = format!("acct-{}", t % 4);
                    for _ in 0..per_thread {
                        let id = leases.lock().expect("lease lock").grant(1_000, &account);
                        leases.lock().expect("lease lock").release(&id);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("thread joins");
        }

        let guard = leases.lock().expect("lease lock");
        for account in 0..4 {
            assert_eq!(
                guard.inflight(1_000, &format!("acct-{account}")),
                0,
                "every granted lease was released — no leak under concurrency"
            );
        }
    }

    #[test]
    fn a_flood_of_unreleased_leases_all_self_heal_at_expiry() {
        // Abnormal-exit path at scale: a thousand requests crash/hang holding a
        // lease and never release. The in-flight count must self-heal at expiry
        // without any release, so a run of failures cannot park an account out
        // of rotation permanently.
        let mut leases = InflightLeases::new(DEFAULT_LEASE_MS);
        for _ in 0..1_000 {
            leases.grant(5_000, "acct");
        }
        assert_eq!(leases.inflight(5_000, "acct"), 1_000);
        assert_eq!(leases.inflight(5_000 + DEFAULT_LEASE_MS - 1, "acct"), 1_000);
        assert_eq!(
            leases.inflight(5_000 + DEFAULT_LEASE_MS, "acct"),
            0,
            "all leaked leases expire together — the count fully self-heals"
        );
    }
}

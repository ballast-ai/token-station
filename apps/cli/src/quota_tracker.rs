//! The host-side quota state the gateway keeps between requests, composed from
//! the accounting primitives: a per-account [`AccountLedger`] (consumption),
//! one [`InflightLeases`] table (concurrency), and a bounded conversation →
//! last-account map (prompt-cache affinity).
//!
//! It answers the two questions the quota-first router needs at route time —
//! "what is this account's quota picture" ([`QuotaTracker::quota_state`], with
//! in-flight load already folded into headroom) and "which account did this
//! conversation last use" ([`QuotaTracker::last_account`]) — and takes the two
//! updates the request flow produces afterward: a lease on dispatch, and a
//! consumption record plus lease release on settle.
//!
//! Accounts are keyed by upstream: a plan (a Claude Pro key, a prepaid
//! `DeepSeek` balance) covers every model reached through that one connection, so
//! two candidate models on the same upstream share its window and its rate.

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};
use token_station_protocol::Usage;
use token_station_router_core::{QuotaState, UpstreamModel};

use crate::quota_ledger::{AccountLedger, RATE_PRESSURE_PERMILLE, SlidingWindow};
use crate::quota_lease::{DEFAULT_LEASE_MS, InflightLeases, LeaseId, apply_inflight_penalty};

/// How many conversations' last-account affinity to remember before evicting the
/// oldest. Affinity is best-effort — a forgotten conversation just re-picks by
/// quota — so a modest bound keeps memory flat under churn.
pub const DEFAULT_SESSION_CAPACITY: usize = 4096;

/// What a plan's windows are counted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanUnit {
    /// The window limit is a token allowance; consumption is input + output.
    #[default]
    Tokens,
    /// The window limit is a request count; each exchange consumes one.
    Requests,
}

/// One reset window of a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaWindowSpec {
    /// The window's period in milliseconds (5h, a day, a week…).
    pub len_ms: u64,
    /// The allowance per window, in the plan's [`PlanUnit`].
    pub limit: u64,
}

/// A user-declared (or provider-discovered) plan for one account.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaPlan {
    /// The reset windows. Empty ⇒ the account is non-windowed (metered).
    #[serde(default)]
    pub windows: Vec<QuotaWindowSpec>,
    /// Requests-per-minute ceiling, if known, for the instantaneous rate window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_per_min: Option<u64>,
    /// The unit the windows count in.
    #[serde(default)]
    pub unit: PlanUnit,
}

struct TrackedAccount {
    ledger: AccountLedger,
    unit: PlanUnit,
    rate_limit_per_min: Option<u64>,
}

impl TrackedAccount {
    fn from_plan(plan: &QuotaPlan) -> Self {
        let windows = plan
            .windows
            .iter()
            .map(|spec| SlidingWindow::new(spec.len_ms, spec.limit))
            .collect();
        Self {
            ledger: AccountLedger::new(windows, plan.rate_limit_per_min),
            unit: plan.unit,
            rate_limit_per_min: plan.rate_limit_per_min,
        }
    }

    fn amount(&self, usage: &Usage) -> u64 {
        match self.unit {
            PlanUnit::Tokens => usage.input_tokens.saturating_add(usage.output_tokens),
            PlanUnit::Requests => 1,
        }
    }
}

/// The gateway's live quota state, updated per request.
pub struct QuotaTracker {
    accounts: HashMap<String, TrackedAccount>,
    leases: InflightLeases,
    sessions: HashMap<String, UpstreamModel>,
    session_order: VecDeque<String>,
    session_capacity: usize,
}

impl QuotaTracker {
    /// Build a tracker from per-account plans, keyed by upstream. Accounts with
    /// no plan are still tracked for concurrency (they get a lease and spread by
    /// in-flight load), they just have no windowed allowance to spend down.
    #[must_use]
    pub fn new(plans: HashMap<String, QuotaPlan>) -> Self {
        let accounts = plans
            .into_iter()
            .map(|(upstream, plan)| (upstream, TrackedAccount::from_plan(&plan)))
            .collect();
        Self {
            accounts,
            leases: InflightLeases::new(DEFAULT_LEASE_MS),
            sessions: HashMap::new(),
            session_order: VecDeque::new(),
            session_capacity: DEFAULT_SESSION_CAPACITY.max(1),
        }
    }

    /// The router-facing quota picture for `upstream` at `now_ms`, with this
    /// account's in-flight load already folded into its rate headroom so the
    /// router spreads concurrent requests off a filling account.
    #[must_use]
    pub fn quota_state(&self, upstream: &str, now_ms: u64) -> QuotaState {
        let (mut state, rate_limit) = match self.accounts.get(upstream) {
            Some(account) => (account.ledger.quota_state(now_ms), account.rate_limit_per_min),
            None => (QuotaState::non_windowed(), None),
        };
        let inflight = self.leases.inflight(now_ms, upstream);
        state.rate_headroom_permille =
            apply_inflight_penalty(state.rate_headroom_permille, inflight, rate_limit);
        state.rate_pressured =
            state.rate_pressured || state.rate_headroom_permille <= RATE_PRESSURE_PERMILLE;
        state
    }

    /// Take an in-flight lease on `upstream` when a request is dispatched to it.
    pub fn grant(&mut self, upstream: &str, now_ms: u64) -> LeaseId {
        self.leases.grant(now_ms, upstream)
    }

    /// Release a lease when its request settles.
    pub fn release(&mut self, lease: &LeaseId) {
        self.leases.release(lease);
    }

    /// Record one settled exchange's consumption against its account's windows.
    /// A no-op for an account with no plan (nothing windowed to count).
    pub fn record(&mut self, upstream: &str, now_ms: u64, usage: &Usage) {
        if let Some(account) = self.accounts.get_mut(upstream) {
            let amount = account.amount(usage);
            account.ledger.record(now_ms, amount);
        }
    }

    /// The account this conversation was last routed to, if remembered.
    #[must_use]
    pub fn last_account(&self, session: &str) -> Option<&UpstreamModel> {
        self.sessions.get(session)
    }

    /// Remember which account served a conversation, for prompt-cache affinity.
    /// Evicts the oldest conversation once at capacity (FIFO).
    pub fn remember(&mut self, session: &str, account: UpstreamModel) {
        if self.sessions.insert(session.to_owned(), account).is_none() {
            self.session_order.push_back(session.to_owned());
            while self.session_order.len() > self.session_capacity {
                if let Some(evicted) = self.session_order.pop_front() {
                    self.sessions.remove(&evicted);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIVE_H: u64 = 5 * 60 * 60 * 1000;

    fn token_plan(limit: u64) -> QuotaPlan {
        QuotaPlan {
            windows: vec![QuotaWindowSpec {
                len_ms: FIVE_H,
                limit,
            }],
            rate_limit_per_min: None,
            unit: PlanUnit::Tokens,
        }
    }

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            ..Usage::default()
        }
    }

    fn tracker(upstream: &str, plan: QuotaPlan) -> QuotaTracker {
        QuotaTracker::new(HashMap::from([(upstream.to_owned(), plan)]))
    }

    #[test]
    fn a_tracked_account_reports_its_window_and_depletes_on_record() {
        let mut t = tracker("claude_pro", token_plan(1000));
        let fresh = t.quota_state("claude_pro", 0);
        assert_eq!(fresh.reset.as_ref().unwrap().ms_until_reset, FIVE_H);
        assert_eq!(fresh.reset.as_ref().unwrap().remaining_permille, 1000);

        t.record("claude_pro", 0, &usage(300, 100)); // 400 tokens
        let after = t.quota_state("claude_pro", 0);
        assert_eq!(after.reset.as_ref().unwrap().remaining_permille, 600);
    }

    #[test]
    fn an_unplanned_account_is_non_windowed_but_still_spreads() {
        let t = tracker("planned", token_plan(1000));
        // No plan for this upstream ⇒ metered/non-windowed.
        let state = t.quota_state("pay_go", 0);
        assert!(state.reset.is_none());
        assert!(!state.exhausted);
    }

    #[test]
    fn in_flight_leases_lower_an_accounts_headroom() {
        let mut t = tracker("acct", token_plan(1_000_000));
        assert_eq!(t.quota_state("acct", 0).rate_headroom_permille, 1000);
        // Three in-flight requests (no known rate limit) dock 100‰ each.
        let _a = t.grant("acct", 0);
        let _b = t.grant("acct", 0);
        let _c = t.grant("acct", 0);
        assert_eq!(t.quota_state("acct", 0).rate_headroom_permille, 700);
    }

    #[test]
    fn releasing_a_lease_restores_headroom() {
        let mut t = tracker("acct", token_plan(1_000_000));
        let lease = t.grant("acct", 0);
        assert_eq!(t.quota_state("acct", 0).rate_headroom_permille, 900);
        t.release(&lease);
        assert_eq!(t.quota_state("acct", 0).rate_headroom_permille, 1000);
    }

    #[test]
    fn request_unit_plans_count_one_per_exchange() {
        let mut t = tracker(
            "msgs",
            QuotaPlan {
                windows: vec![QuotaWindowSpec {
                    len_ms: FIVE_H,
                    limit: 10,
                }],
                rate_limit_per_min: None,
                unit: PlanUnit::Requests,
            },
        );
        // A big-token exchange still counts as a single request.
        t.record("msgs", 0, &usage(5000, 5000));
        assert_eq!(
            t.quota_state("msgs", 0).reset.as_ref().unwrap().remaining_permille,
            900
        );
    }

    #[test]
    fn session_affinity_is_remembered_and_bounded() {
        let mut t = tracker("acct", token_plan(1000));
        let account = UpstreamModel::new(
            token_station_router_core::UpstreamRef::new("acct").unwrap(),
            "model",
        );
        assert!(t.last_account("conv-1").is_none());
        t.remember("conv-1", account.clone());
        assert_eq!(t.last_account("conv-1"), Some(&account));

        // Overflow the capacity and the oldest is forgotten (FIFO).
        t.session_capacity = 2;
        t.remember("conv-2", account.clone());
        t.remember("conv-3", account.clone()); // evicts conv-1
        assert!(t.last_account("conv-1").is_none());
        assert!(t.last_account("conv-3").is_some());
    }
}

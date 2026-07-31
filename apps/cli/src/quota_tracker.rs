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
use token_station_router_core::{QuotaState, ResetWindow, UpstreamModel};

use crate::quota_lease::{DEFAULT_LEASE_MS, InflightLeases, LeaseId, apply_inflight_penalty};
use crate::quota_ledger::{AccountLedger, RATE_PRESSURE_PERMILLE, SlidingWindow, WindowSnapshot};

/// How long a provider-reported (authoritative) quota reading is trusted before
/// it goes stale and the local ledger estimate takes over again. Providers stamp
/// their limits on every response, so a live account refreshes this well within
/// the window; a quiet account falls back to the estimate rather than showing an
/// old absolute number as if it were current.
pub const AUTHORITATIVE_TTL_MS: u64 = 5 * 60 * 1000;

/// Where an account's window figures came from — surfaced to the viewer so a
/// provider-reported number is never shown as if it were a local guess, and vice
/// versa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaSource {
    /// Provider-reported via response headers (no local blind spot).
    Authoritative,
    /// Locally counted from traffic through this gateway only (a lower bound on
    /// real usage — the user may spend the same key elsewhere).
    Estimated,
    /// No windowed plan and no provider reading: nothing to show but rate/cooling.
    None,
}

/// A provider-reported quota reading with the instant it was observed, so it can
/// be aged out after [`AUTHORITATIVE_TTL_MS`].
#[derive(Debug, Clone)]
struct Authoritative {
    windows: Vec<WindowSnapshot>,
    observed_at_ms: u64,
}

/// The full per-account picture for the quota viewer: every window, the rate
/// state, in-flight load, cooling, and where the figures came from.
#[derive(Debug, Clone, Serialize)]
pub struct QuotaAccountSnapshot {
    pub upstream: String,
    pub windows: Vec<QuotaWindowSnapshot>,
    pub rate_headroom_permille: u16,
    pub rate_pressured: bool,
    pub inflight: u32,
    pub exhausted: bool,
    /// Milliseconds left on an active rate/quota cooldown (from a real upstream
    /// refusal); `0` when not cooling.
    pub cooling_ms_remaining: u64,
    pub source: QuotaSource,
}

/// One window in a [`QuotaAccountSnapshot`], serialization-ready for the viewer.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct QuotaWindowSnapshot {
    pub len_ms: u64,
    pub limit: u64,
    pub used: u64,
    pub remaining_permille: u16,
    pub ms_until_reset: u64,
}

impl From<WindowSnapshot> for QuotaWindowSnapshot {
    fn from(w: WindowSnapshot) -> Self {
        Self {
            len_ms: w.len_ms,
            limit: w.limit,
            used: w.used,
            remaining_permille: w.remaining_permille,
            ms_until_reset: w.ms_until_reset,
        }
    }
}

/// The `(reset, exhausted)` the router needs, derived from a set of windows: the
/// soonest-closing window drives urgency, and any spent window binds.
fn binding_of(windows: &[WindowSnapshot]) -> (Option<ResetWindow>, bool) {
    let reset = windows
        .iter()
        .min_by_key(|w| w.ms_until_reset)
        .map(|w| ResetWindow {
            ms_until_reset: w.ms_until_reset,
            remaining_permille: w.remaining_permille,
        });
    let exhausted = windows.iter().any(|w| w.remaining_permille == 0);
    (reset, exhausted)
}

/// How many conversations' last-account affinity to remember before evicting the
/// oldest. Affinity is best-effort — a forgotten conversation just re-picks by
/// quota — so a modest bound keeps memory flat under churn.
pub const DEFAULT_SESSION_CAPACITY: usize = 4096;

/// Cooldown applied to an account when the upstream reports a quota/rate refusal
/// (429/402) but supplies no usable `Retry-After`. Long enough to stop hammering
/// a spent allowance, short enough to recover promptly once it clears.
pub const DEFAULT_QUOTA_COOLDOWN_MS: u64 = 60_000;

/// Ceiling on any single cooldown, so one upstream's oversized `Retry-After`
/// cannot park an account out of rotation for an unbounded stretch. Past this we
/// re-probe (and, if still limited, re-cool) rather than trust an arbitrary wait.
pub const MAX_QUOTA_COOLDOWN_MS: u64 = 60 * 60 * 1000;

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
    /// Per-upstream "cooling until" wall-clock (ms). Set when the upstream itself
    /// refuses for quota/rate reasons — the ground-truth signal that this
    /// allowance is spent regardless of what the local ledger estimated. While
    /// cooling, the account reports `exhausted` and drops out of rotation. Keyed
    /// by upstream so it works even for accounts with no configured plan.
    cooling_until_ms: HashMap<String, u64>,
    /// Per-upstream provider-reported quota, preferred over the local ledger
    /// while fresh (within [`AUTHORITATIVE_TTL_MS`]). This is the L2 layer: no
    /// local blind spot, because the provider counts all of the key's usage.
    authoritative: HashMap<String, Authoritative>,
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
            cooling_until_ms: HashMap::new(),
            authoritative: HashMap::new(),
        }
    }

    /// Fresh provider-reported windows for `upstream`, or `None` if there is no
    /// reading or it has aged past [`AUTHORITATIVE_TTL_MS`].
    fn fresh_authoritative(&self, upstream: &str, now_ms: u64) -> Option<&[WindowSnapshot]> {
        self.authoritative.get(upstream).and_then(|a| {
            (now_ms.saturating_sub(a.observed_at_ms) < AUTHORITATIVE_TTL_MS)
                .then_some(a.windows.as_slice())
        })
    }

    /// Whether `upstream` is cooling from a real rate/quota refusal at `now_ms`.
    fn is_cooling(&self, upstream: &str, now_ms: u64) -> bool {
        self.cooling_until_ms
            .get(upstream)
            .is_some_and(|until| now_ms < *until)
    }

    /// The router-facing quota picture for `upstream` at `now_ms`, with this
    /// account's in-flight load already folded into its rate headroom so the
    /// router spreads concurrent requests off a filling account.
    #[must_use]
    pub fn quota_state(&self, upstream: &str, now_ms: u64) -> QuotaState {
        let (mut state, rate_limit) = match self.accounts.get(upstream) {
            Some(account) => (
                account.ledger.quota_state(now_ms),
                account.rate_limit_per_min,
            ),
            None => (QuotaState::non_windowed(), None),
        };
        // A fresh provider reading wins over the local estimate for the windowed
        // picture (reset + exhausted) — it has no blind spot. Rate headroom stays
        // local (instantaneous, and folded with in-flight below).
        if let Some(windows) = self.fresh_authoritative(upstream, now_ms) {
            let (reset, exhausted) = binding_of(windows);
            state.reset = reset;
            state.exhausted = exhausted;
        }
        let inflight = self.leases.inflight(now_ms, upstream);
        state.rate_headroom_permille =
            apply_inflight_penalty(state.rate_headroom_permille, inflight, rate_limit);
        state.rate_pressured =
            state.rate_pressured || state.rate_headroom_permille <= RATE_PRESSURE_PERMILLE;
        // Ground-truth override: while the upstream is cooling from a real
        // quota/rate refusal, the account is exhausted no matter what the ledger
        // or authoritative reading said. Nothing routes to it until it elapses.
        if self.is_cooling(upstream, now_ms) {
            state.exhausted = true;
        }
        state
    }

    /// Cool an account after the upstream itself refused it for quota/rate
    /// reasons (429/402). `cooldown_ms` is the upstream's `Retry-After` when
    /// present (else [`DEFAULT_QUOTA_COOLDOWN_MS`]), capped by
    /// [`MAX_QUOTA_COOLDOWN_MS`]. Until it elapses the account reports
    /// `exhausted` and drops out of quota rotation — the L1 feedback loop that
    /// keeps routing from hammering a spent allowance.
    pub fn note_rate_limited(&mut self, upstream: &str, now_ms: u64, cooldown_ms: u64) {
        let until = now_ms.saturating_add(cooldown_ms.min(MAX_QUOTA_COOLDOWN_MS));
        self.cooling_until_ms
            .entry(upstream.to_owned())
            .and_modify(|current| *current = (*current).max(until))
            .or_insert(until);
    }

    /// Record a provider-reported (authoritative) quota reading for `upstream`,
    /// parsed from response headers. Preferred over the local estimate while
    /// fresh; an empty `windows` clears any stale reading. This is the L2 layer.
    pub fn note_authoritative(
        &mut self,
        upstream: &str,
        now_ms: u64,
        windows: Vec<WindowSnapshot>,
    ) {
        if windows.is_empty() {
            self.authoritative.remove(upstream);
            return;
        }
        self.authoritative.insert(
            upstream.to_owned(),
            Authoritative {
                windows,
                observed_at_ms: now_ms,
            },
        );
    }

    /// Full per-account picture for the quota viewer, one entry per requested
    /// upstream (so callers control which accounts appear), with figures tagged
    /// by [`QuotaSource`]. Windowed detail prefers a fresh provider reading, then
    /// the local ledger estimate; rate/in-flight/cooling are always live.
    #[must_use]
    pub fn snapshot(&self, upstreams: &[String], now_ms: u64) -> Vec<QuotaAccountSnapshot> {
        upstreams
            .iter()
            .map(|upstream| {
                let (windows, source) = if let Some(w) = self.fresh_authoritative(upstream, now_ms)
                {
                    (w.to_vec(), QuotaSource::Authoritative)
                } else if let Some(account) = self.accounts.get(upstream) {
                    let w = account.ledger.window_snapshots(now_ms);
                    let source = if w.is_empty() {
                        QuotaSource::None
                    } else {
                        QuotaSource::Estimated
                    };
                    (w, source)
                } else {
                    (Vec::new(), QuotaSource::None)
                };

                let (_, window_exhausted) = binding_of(&windows);
                let rate_limit = self
                    .accounts
                    .get(upstream)
                    .and_then(|a| a.rate_limit_per_min);
                let ledger_headroom = self
                    .accounts
                    .get(upstream)
                    .map_or(1000, |a| a.ledger.rate_headroom_permille(now_ms));
                let inflight = self.leases.inflight(now_ms, upstream);
                let rate_headroom_permille =
                    apply_inflight_penalty(ledger_headroom, inflight, rate_limit);
                let cooling = self
                    .cooling_until_ms
                    .get(upstream)
                    .map_or(0, |until| until.saturating_sub(now_ms));

                QuotaAccountSnapshot {
                    upstream: upstream.clone(),
                    windows: windows.into_iter().map(Into::into).collect(),
                    rate_headroom_permille,
                    rate_pressured: rate_headroom_permille <= RATE_PRESSURE_PERMILLE,
                    inflight,
                    exhausted: window_exhausted || self.is_cooling(upstream, now_ms),
                    cooling_ms_remaining: cooling,
                    source,
                }
            })
            .collect()
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
    use crate::quota_ledger::WindowSnapshot;

    const FIVE_H: u64 = 5 * 60 * 60 * 1000;

    fn window(remaining_permille: u16, ms_until_reset: u64) -> WindowSnapshot {
        WindowSnapshot {
            len_ms: FIVE_H,
            limit: 1000,
            used: u64::from(1000 - remaining_permille),
            remaining_permille,
            ms_until_reset,
        }
    }

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
    fn a_rate_limited_account_cools_off_until_its_retry_after_elapses() {
        let mut t = tracker("acct", token_plan(1_000_000));
        // Plenty of allowance ⇒ not exhausted on its own.
        assert!(!t.quota_state("acct", 0).exhausted);
        // The upstream returned 429 with a 30s Retry-After: cool it.
        t.note_rate_limited("acct", 0, 30_000);
        assert!(
            t.quota_state("acct", 10_000).exhausted,
            "cooling forces exhausted so the router drops it from rotation"
        );
        assert!(t.quota_state("acct", 29_999).exhausted);
        // At/after the cooldown it recovers on its own — no manual clear needed.
        assert!(!t.quota_state("acct", 30_000).exhausted);
        assert!(!t.quota_state("acct", 60_000).exhausted);
    }

    #[test]
    fn a_fresh_authoritative_reading_overrides_the_local_estimate_then_ages_out() {
        let mut t = tracker("acct", token_plan(1000));
        t.record("acct", 0, &usage(600, 0)); // 600 of 1000 used ⇒ 400‰ left
        assert_eq!(
            t.quota_state("acct", 0).reset.unwrap().remaining_permille,
            400
        );

        // The provider reports only 50‰ left (it sees the key's usage elsewhere
        // too) with a sooner reset. That wins over the local estimate.
        t.note_authoritative("acct", 0, vec![window(50, FIVE_H / 2)]);
        let fresh = t.quota_state("acct", 0);
        assert_eq!(fresh.reset.as_ref().unwrap().remaining_permille, 50);
        assert_eq!(fresh.reset.as_ref().unwrap().ms_until_reset, FIVE_H / 2);

        // Past the TTL the reading is stale and the local estimate takes back over.
        let stale = t.quota_state("acct", AUTHORITATIVE_TTL_MS);
        assert_eq!(
            stale.reset.as_ref().unwrap().remaining_permille,
            400,
            "the local estimate resumes once the provider reading ages out"
        );
    }

    #[test]
    fn snapshot_tags_each_accounts_source_and_reflects_cooling() {
        let mut t = tracker("planned", token_plan(1000));
        // A provider reading for one account, an exhausted window.
        t.note_authoritative("live", 0, vec![window(0, FIVE_H)]);
        // A cooldown from a real 429 on a third, plan-less account.
        t.note_rate_limited("cooled", 0, 30_000);

        let upstreams = ["planned", "live", "cooled", "unknown"].map(String::from);
        let snap = t.snapshot(&upstreams, 1_000);
        let by = |u: &str| snap.iter().find(|s| s.upstream == u).expect("present");

        // Locally counted plan → Estimated, one window shown.
        assert!(matches!(by("planned").source, QuotaSource::Estimated));
        assert_eq!(by("planned").windows.len(), 1);
        // Provider-reported → Authoritative, and its spent window ⇒ exhausted.
        assert!(matches!(by("live").source, QuotaSource::Authoritative));
        assert!(by("live").exhausted);
        // Plan-less but cooling → no window source, yet exhausted + cooling shown.
        assert!(matches!(by("cooled").source, QuotaSource::None));
        assert!(by("cooled").exhausted);
        assert!(by("cooled").cooling_ms_remaining > 0);
        // Unknown upstream → nothing to show, no windows.
        assert!(matches!(by("unknown").source, QuotaSource::None));
        assert!(by("unknown").windows.is_empty());
    }

    #[test]
    fn in_flight_load_saturates_headroom_so_routing_spreads_off_a_filling_account() {
        // The no-oversend guarantee: as concurrent requests pile onto one account
        // its rate headroom falls and it turns "pressured", so the next ranking
        // spills the load elsewhere instead of over-sending a single account past
        // its rate limit. With a known 10/min limit, each in-flight request
        // commits 10% of the budget.
        let plan = QuotaPlan {
            windows: vec![],
            rate_limit_per_min: Some(10),
            unit: PlanUnit::Requests,
        };
        let mut t = tracker("acct", plan);
        assert_eq!(t.quota_state("acct", 0).rate_headroom_permille, 1000);

        let ids: Vec<_> = (0..10).map(|_| t.grant("acct", 0)).collect();
        let saturated = t.quota_state("acct", 0);
        assert_eq!(
            saturated.rate_headroom_permille, 0,
            "10 in-flight saturate a 10/min budget"
        );
        assert!(
            saturated.rate_pressured,
            "a saturated account is pressured, so the router spills off it"
        );

        // Load is impending, not permanent: releasing restores full headroom.
        for id in ids {
            t.release(&id);
        }
        assert_eq!(t.quota_state("acct", 0).rate_headroom_permille, 1000);
    }

    #[test]
    fn cooling_applies_to_unplanned_accounts_and_caps_absurd_waits() {
        // An account with no configured plan can still be 429'd/402'd.
        let mut t = tracker("planned", token_plan(1000));
        t.note_rate_limited("pay_go", 0, 5_000);
        assert!(t.quota_state("pay_go", 1_000).exhausted);
        assert!(!t.quota_state("pay_go", 5_000).exhausted);
        // An oversized Retry-After is capped so an upstream can't park an account
        // out of rotation indefinitely.
        t.note_rate_limited("pay_go", 0, u64::MAX);
        assert!(t.quota_state("pay_go", MAX_QUOTA_COOLDOWN_MS - 1).exhausted);
        assert!(!t.quota_state("pay_go", MAX_QUOTA_COOLDOWN_MS).exhausted);
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
            t.quota_state("msgs", 0)
                .reset
                .as_ref()
                .unwrap()
                .remaining_permille,
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

//! Self-counted quota accounting: how much of each account's windowed allowance
//! has been spent, and how close its instantaneous rate is to the limit.
//!
//! The mechanism is borrowed from `OmniRoute`'s quota store and is the piece most
//! worth stealing: a **two-bucket sliding window with implicit reset**. Each
//! window keeps only two counts — the current bucket and the previous one — and
//! the bucket a timestamp falls in is `now / window_len`, pure arithmetic. So a
//! window "resets" with no timer and no cleanup task: when the clock crosses a
//! bucket boundary the old count ages out by itself, and a process restart
//! recovers the right picture from the persisted counts plus the current time.
//!
//! This is the *fallback* accounting, used when a provider does not report its
//! own quota. When a fetcher or a response header hands us an authoritative
//! `remaining` and `reset_at`, those are used directly (see the fetcher layer);
//! this module answers the "user told us the plan is N tokens per 5h, count it
//! ourselves" case, plus the always-local instantaneous rate window.
//!
//! Everything here takes `now_ms` as an argument and reads no clock, so it is a
//! pure function of its inputs — replayable and unit-testable, matching the
//! router core it feeds.

use token_station_router_core::{QuotaState, ResetWindow};

/// The instantaneous-rate window: how recent activity is measured for
/// load-spreading. One minute matches how providers publish requests-per-minute
/// limits.
pub const RATE_WINDOW_MS: u64 = 60_000;

/// At or below this much rate headroom (permille) an account is treated as
/// pressured, so the router spills fresh load off it before it 429s.
pub const RATE_PRESSURE_PERMILLE: u16 = 100;

/// A two-bucket sliding-window counter over a fixed period. Reset is implicit:
/// the bucket index is `now / len`, so crossing a boundary ages the old count
/// out without a timer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlidingWindow {
    len_ms: u64,
    /// The allowance for one window (tokens, or requests for the rate window).
    limit: u64,
    /// Index of the bucket `curr` counts; `now / len_ms` at the last record.
    index: u64,
    /// Count in the current bucket.
    curr: u64,
    /// Count in the immediately preceding bucket, which decays across the
    /// current one.
    prev: u64,
}

impl SlidingWindow {
    /// A fresh counter for a `len_ms`-long window with the given allowance.
    #[must_use]
    pub fn new(len_ms: u64, limit: u64) -> Self {
        Self {
            len_ms: len_ms.max(1),
            limit,
            index: 0,
            curr: 0,
            prev: 0,
        }
    }

    fn index_at(&self, now_ms: u64) -> u64 {
        now_ms / self.len_ms
    }

    /// Roll the buckets forward to `now_ms` before recording into `curr`.
    fn advance(&mut self, now_ms: u64) {
        let idx = self.index_at(now_ms);
        if idx == self.index {
            return;
        }
        // Adjacent bucket: the old current becomes previous. A gap of more than
        // one window means everything before is fully aged out.
        self.prev = if idx == self.index + 1 { self.curr } else { 0 };
        self.curr = 0;
        self.index = idx;
    }

    /// Record `amount` of consumption at `now_ms`.
    pub fn record(&mut self, now_ms: u64, amount: u64) {
        self.advance(now_ms);
        self.curr = self.curr.saturating_add(amount);
    }

    /// Effective consumption as seen at `now_ms`, without mutating: the current
    /// bucket plus the previous bucket weighted by how much of the window it
    /// still covers. Pure, so a read never disturbs the count.
    #[must_use]
    pub fn effective_used(&self, now_ms: u64) -> u64 {
        let idx = self.index_at(now_ms);
        let (curr, prev) = if idx == self.index {
            (self.curr, self.prev)
        } else if idx == self.index + 1 {
            (0, self.curr)
        } else {
            (0, 0)
        };
        let bucket_start = idx.saturating_mul(self.len_ms);
        let elapsed = now_ms.saturating_sub(bucket_start).min(self.len_ms);
        // The previous bucket's weight decays linearly as the current one fills.
        let prev_weight = self.len_ms - elapsed;
        // Bounded above by `prev`, so it always fits back into u64.
        let prev_contribution =
            u64::try_from(u128::from(prev) * u128::from(prev_weight) / u128::from(self.len_ms))
                .unwrap_or(prev);
        curr.saturating_add(prev_contribution)
    }

    /// Remaining allowance in permille (`0..=1000`). Zero limit ⇒ nothing left.
    #[must_use]
    pub fn remaining_permille(&self, now_ms: u64) -> u16 {
        if self.limit == 0 {
            return 0;
        }
        let used = self.effective_used(now_ms).min(self.limit);
        let remaining = self.limit - used;
        u16::try_from(u128::from(remaining) * 1000 / u128::from(self.limit)).unwrap_or(1000)
    }

    /// Milliseconds until this window's next bucket boundary — the point at
    /// which the oldest usage has fully aged out. The self-counted stand-in for
    /// a provider-reported reset time.
    #[must_use]
    pub fn ms_until_reset(&self, now_ms: u64) -> u64 {
        let idx = self.index_at(now_ms);
        idx.saturating_add(1).saturating_mul(self.len_ms) - now_ms
    }

    /// Full display detail for one window as of `now_ms` — richer than the
    /// router-facing [`QuotaState`], which keeps only the single binding window.
    #[must_use]
    pub fn snapshot(&self, now_ms: u64) -> WindowSnapshot {
        WindowSnapshot {
            len_ms: self.len_ms,
            limit: self.limit,
            used: self.effective_used(now_ms).min(self.limit),
            remaining_permille: self.remaining_permille(now_ms),
            ms_until_reset: self.ms_until_reset(now_ms),
        }
    }
}

/// One window's full picture for the quota viewer: its period, allowance, how
/// much is used, and when it resets. Pure display detail; the router ranks on
/// [`QuotaState`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSnapshot {
    pub len_ms: u64,
    pub limit: u64,
    pub used: u64,
    pub remaining_permille: u16,
    pub ms_until_reset: u64,
}

/// One account's self-counted quota: its (possibly several) reset windows and
/// its instantaneous rate window. Produces the [`QuotaState`] the router ranks
/// on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountLedger {
    /// The plan's reset windows (e.g. a 5-hour and a weekly one). Empty ⇒ the
    /// account is non-windowed (pay-as-you-go / unmeasured).
    windows: Vec<SlidingWindow>,
    /// The always-local instantaneous rate window. `None` ⇒ rate is not tracked
    /// and the account is treated as having full headroom.
    rate: Option<SlidingWindow>,
}

impl AccountLedger {
    /// A ledger with the given reset windows and an optional per-minute rate
    /// limit (in requests, or whatever unit the caller records).
    #[must_use]
    pub fn new(windows: Vec<SlidingWindow>, rate_limit_per_min: Option<u64>) -> Self {
        Self {
            windows,
            rate: rate_limit_per_min.map(|limit| SlidingWindow::new(RATE_WINDOW_MS, limit)),
        }
    }

    /// A non-windowed account (pay-as-you-go / unmeasured): no reset windows,
    /// optionally still rate-tracked.
    #[must_use]
    pub fn non_windowed(rate_limit_per_min: Option<u64>) -> Self {
        Self::new(Vec::new(), rate_limit_per_min)
    }

    /// Record one exchange: `amount` against every reset window (tokens or
    /// requests, matching the windows' `limit` unit) and one unit against the
    /// rate window.
    pub fn record(&mut self, now_ms: u64, amount: u64) {
        for window in &mut self.windows {
            window.record(now_ms, amount);
        }
        if let Some(rate) = &mut self.rate {
            rate.record(now_ms, 1);
        }
    }

    /// The router-facing quota picture as of `now_ms`.
    ///
    /// - `reset` is the **soonest-closing** window (its time-to-reset drives
    ///   urgency), carrying that window's remaining for display.
    /// - `exhausted` is true if **any** window's allowance is spent, since the
    ///   tightest window binds.
    /// - rate headroom / pressure come from the instantaneous rate window.
    #[must_use]
    pub fn quota_state(&self, now_ms: u64) -> QuotaState {
        let reset = self
            .windows
            .iter()
            .min_by_key(|window| window.ms_until_reset(now_ms))
            .map(|window| ResetWindow {
                ms_until_reset: window.ms_until_reset(now_ms),
                remaining_permille: window.remaining_permille(now_ms),
            });

        let exhausted = self
            .windows
            .iter()
            .any(|window| window.remaining_permille(now_ms) == 0);

        let rate_headroom_permille = self
            .rate
            .as_ref()
            .map_or(1000, |rate| rate.remaining_permille(now_ms));
        let rate_pressured = rate_headroom_permille <= RATE_PRESSURE_PERMILLE;

        QuotaState {
            reset,
            rate_headroom_permille,
            rate_pressured,
            exhausted,
        }
    }

    /// Full per-window detail for the viewer, in the plan's declared order. Empty
    /// for a non-windowed (pay-as-you-go / unmeasured) account.
    #[must_use]
    pub fn window_snapshots(&self, now_ms: u64) -> Vec<WindowSnapshot> {
        self.windows
            .iter()
            .map(|window| window.snapshot(now_ms))
            .collect()
    }

    /// Instantaneous rate headroom (permille) from the local rate window, before
    /// any in-flight penalty. Full headroom when the account is not rate-tracked.
    #[must_use]
    pub fn rate_headroom_permille(&self, now_ms: u64) -> u16 {
        self.rate
            .as_ref()
            .map_or(1000, |rate| rate.remaining_permille(now_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIVE_H: u64 = 5 * 60 * 60 * 1000;

    #[test]
    fn a_fresh_window_is_fully_available() {
        let w = SlidingWindow::new(FIVE_H, 1000);
        assert_eq!(w.remaining_permille(0), 1000);
        assert_eq!(w.effective_used(0), 0);
    }

    #[test]
    fn recording_depletes_the_window_proportionally() {
        let mut w = SlidingWindow::new(FIVE_H, 1000);
        w.record(1_000, 400);
        assert_eq!(w.effective_used(1_000), 400);
        assert_eq!(w.remaining_permille(1_000), 600);
    }

    #[test]
    fn crossing_a_bucket_boundary_ages_the_old_count_out_with_no_timer() {
        let mut w = SlidingWindow::new(FIVE_H, 1000);
        // Spend it all inside the first bucket.
        w.record(1_000, 1000);
        assert_eq!(w.remaining_permille(1_000), 0);
        // At the very start of the next bucket, the previous bucket still fully
        // weighs in (nothing of the new bucket has elapsed).
        let next_start = FIVE_H;
        assert_eq!(w.remaining_permille(next_start), 0);
        // Halfway through the next bucket, half the old usage has aged out.
        assert_eq!(w.remaining_permille(next_start + FIVE_H / 2), 500);
        // A full window later, the old bucket is entirely gone: fully available.
        assert_eq!(w.remaining_permille(next_start + FIVE_H), 1000);
    }

    #[test]
    fn a_gap_of_more_than_one_window_forgets_everything() {
        let mut w = SlidingWindow::new(FIVE_H, 1000);
        w.record(0, 1000);
        // Two windows later, both buckets are stale.
        assert_eq!(w.remaining_permille(FIVE_H * 3), 1000);
    }

    #[test]
    fn ms_until_reset_counts_down_to_the_next_boundary() {
        let w = SlidingWindow::new(FIVE_H, 1000);
        assert_eq!(w.ms_until_reset(0), FIVE_H);
        assert_eq!(w.ms_until_reset(FIVE_H - 1), 1);
        assert_eq!(w.ms_until_reset(FIVE_H), FIVE_H);
    }

    #[test]
    fn a_read_never_mutates_the_count() {
        let mut w = SlidingWindow::new(FIVE_H, 1000);
        w.record(1_000, 500);
        // Peeking far in the future must not roll the stored buckets.
        let _ = w.effective_used(FIVE_H * 10);
        assert_eq!(w.effective_used(1_000), 500);
    }

    #[test]
    fn the_ledger_reports_the_soonest_closing_window_and_binds_on_the_tightest() {
        // A 5h window (fresh) and a weekly window that is spent.
        let weekly = 7 * 24 * 60 * 60 * 1000;
        let mut ledger = AccountLedger::new(
            vec![
                SlidingWindow::new(FIVE_H, 1000),
                SlidingWindow::new(weekly, 500),
            ],
            None,
        );
        ledger.record(0, 500); // spends the weekly window, dents the 5h one
        let state = ledger.quota_state(0);
        // Soonest reset is the 5h window.
        assert_eq!(state.reset.as_ref().unwrap().ms_until_reset, FIVE_H);
        // But the weekly window is spent, so the account is exhausted.
        assert!(state.exhausted);
    }

    #[test]
    fn a_non_windowed_ledger_has_no_reset_and_is_not_exhausted() {
        let ledger = AccountLedger::non_windowed(None);
        let state = ledger.quota_state(0);
        assert!(state.reset.is_none());
        assert!(!state.exhausted);
        assert_eq!(state.rate_headroom_permille, 1000);
    }

    #[test]
    fn hammering_the_rate_window_marks_the_account_pressured() {
        // 10 requests/min budget.
        let mut ledger = AccountLedger::new(vec![SlidingWindow::new(FIVE_H, 1_000_000)], Some(10));
        for i in 0..10 {
            ledger.record(i, 1);
        }
        let state = ledger.quota_state(10);
        assert!(state.rate_pressured, "rate budget spent ⇒ pressured");
        assert!(!state.exhausted, "but the plan window still has plenty");
    }
}

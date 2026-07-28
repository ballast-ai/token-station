//! Quota-first routing: pick the account whose window is closing *soonest*,
//! keep a conversation on the account it already warmed (prompt cache), and
//! shed load off accounts that are pressing their instantaneous rate limit.
//!
//! This is the supply-side answer to a different question than the tiered
//! router: not "which model fits this request" but "given a model already
//! chosen, which of my *quota-bearing* accounts should serve it so that
//! prepaid / free-tier allowances are spent before they refresh and are lost."
//!
//! # Why this shape (and not the obvious one)
//!
//! The naive score is `urgency × remaining` — prefer the account that is both
//! about-to-reset and has the most left. It thrashes: consuming the leader
//! makes it no longer the leader, so every request flips accounts and shreds
//! the provider-side prompt cache. It also concentrates all load on one
//! account, which is the fastest way to trip that account's requests-per-minute
//! limit.
//!
//! So `remaining` is **not** a scoring term here — it is only a gate (an
//! exhausted window cannot serve). Waste-avoidance comes entirely from *how
//! soon the window resets*: the sooner a window closes, the sooner its unused
//! allowance is lost, so it gets priority. Urgency is the **absolute** time to
//! reset — a window closing in 20 minutes outranks one closing in 4 hours,
//! whatever their lengths — and for two windows closing at (nearly) the same
//! time the total wasted at reset is identical regardless of which we drain, so
//! we are free to spread across them by the secondary score.
//!
//! Ranking is therefore two levels:
//!
//! 1. **reset bucket** — absolute time-to-reset, quantized by a tie band so
//!    near-equal resets compare equal; the sooner bucket always wins.
//!    Non-windowed accounts (pay-as-you-go / unmeasured) sink to the bottom
//!    bucket, so windowed allowances are spent first and metered spend is the
//!    last resort — which is also the cheapest order.
//! 2. **secondary score**, inside a bucket — in descending weight:
//!    - `affinity`  — stay on the account this conversation last used (cache
//!      continuity), *unless* it is pressured.
//!    - `headroom`  — send fresh / spilled load to the freest account.
//!    - `pressure`  — an account at its rate limit is pushed down hard enough
//!      to overcome its own affinity bonus, so a warmed-but-throttled account
//!      spills to a free one.
//!    - insertion order — a stable, deterministic final tiebreak.
//!
//! Everything the ranking reads is host-measured and handed in via
//! [`QuotaState`] (reset clock, headroom) or the per-request `last_used` flag.
//! This module performs no I/O and no clock reads, so a ranking is a pure
//! function of its inputs — replayable and unit-testable, matching the router's
//! invariants.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

/// Two resets within this many milliseconds of each other are treated as
/// simultaneous, so the secondary score (not a sub-second clock difference)
/// decides between them.
pub const DEFAULT_RESET_TIE_BAND_MS: u64 = 60_000;

/// The binding reset window of an account: the tightest of its windows (a plan
/// with both a 5-hour and a weekly window is constrained by whichever is closer
/// to resetting / more depleted). The host collapses multiple windows into this
/// one before handing it to the router.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetWindow {
    /// Milliseconds until this window refills. Smaller ⇒ more urgent.
    pub ms_until_reset: u64,
    /// Fraction of this window's allowance still available, in permille
    /// (`0..=1000`). Not a ranking input — kept for the exhaustion gate and for
    /// the quota viewer UI.
    pub remaining_permille: u16,
}

/// One account's quota picture, entirely host-measured. Attached to a routing
/// candidate the same way `health` is: the router owns none of it.
///
/// [`Default`] is a non-windowed, fully-open account — the neutral value a
/// candidate carries in tiered mode (where quota is ignored) and the safe
/// starting point before the host has measured anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaState {
    /// The binding reset window, or `None` for a non-windowed account
    /// (pay-as-you-go, or one whose quota could not be measured). `None` sinks
    /// beneath every windowed account and is used only as a last resort —
    /// which for pay-as-you-go is also the cheapest order (spend perishable
    /// allowances first, metered last).
    pub reset: Option<ResetWindow>,
    /// Instantaneous rate headroom over the last rolling minute, in permille
    /// (`1000` = the account's requests/tokens-per-minute budget is untouched,
    /// `0` = saturated). Drives load-spreading.
    pub rate_headroom_permille: u16,
    /// The account is at or over its instantaneous rate limit right now (a fresh
    /// 429 / in-flight leases have filled it). Load is pushed off it even if a
    /// conversation had warmed it.
    pub rate_pressured: bool,
    /// A window allowance is spent (`remaining == 0`). The account cannot serve
    /// until it resets and is dropped before ranking.
    pub exhausted: bool,
}

impl QuotaState {
    /// A fully-open windowed account: full headroom, not pressured, not
    /// exhausted. Convenience for tests and for the "just learned its limits"
    /// path.
    #[must_use]
    pub fn open(ms_until_reset: u64) -> Self {
        Self {
            reset: Some(ResetWindow {
                ms_until_reset,
                remaining_permille: 1000,
            }),
            rate_headroom_permille: 1000,
            rate_pressured: false,
            exhausted: false,
        }
    }

    /// A non-windowed account (pay-as-you-go / unmeasured): no reset clock, so
    /// it sinks below every windowed account.
    #[must_use]
    pub fn non_windowed() -> Self {
        Self {
            reset: None,
            rate_headroom_permille: 1000,
            rate_pressured: false,
            exhausted: false,
        }
    }
}

impl Default for QuotaState {
    fn default() -> Self {
        Self::non_windowed()
    }
}

/// Tunable weights for the secondary score. Defaults are chosen so the ordering
/// *intent* holds without hand-tuning: `pressure` exceeds `affinity`, so a
/// warmed-but-throttled account yields to a free one (spillover), while an
/// un-pressured warmed account keeps its edge (`affinity` > `headroom`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuotaWeights {
    /// Flat bonus for the account this conversation last used.
    pub affinity: u32,
    /// Per permille of instantaneous rate headroom.
    pub headroom: u32,
    /// Flat penalty for an account at its instantaneous rate limit.
    pub pressure: u32,
}

impl Default for QuotaWeights {
    fn default() -> Self {
        Self {
            affinity: 300_000,
            headroom: 200_000,
            pressure: 500_000,
        }
    }
}

/// Configuration for quota-first routing, persisted in [`crate::RouterConfig`].
/// All fields default, so a config that merely flips the mode on gets sensible
/// behavior without spelling any of this out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuotaConfig {
    /// Secondary-score weights.
    pub weights: QuotaWeights,
    /// Reset tie band — see [`DEFAULT_RESET_TIE_BAND_MS`].
    pub reset_tie_band_ms: u64,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            weights: QuotaWeights::default(),
            reset_tie_band_ms: DEFAULT_RESET_TIE_BAND_MS,
        }
    }
}

/// Where a candidate lands in the quota-first order. Comparison is "better is
/// greater": the preferred account is the [`Ord::max`]. A sooner reset bucket
/// always beats a later one; within a bucket the higher secondary score wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaRank {
    /// Absolute time-to-reset quantized by the tie band; lower is sooner is
    /// better. Non-windowed accounts get `u64::MAX`.
    bucket: u64,
    /// Secondary score inside the bucket; higher is better.
    score: i64,
}

impl Ord for QuotaRank {
    fn cmp(&self, other: &Self) -> Ordering {
        // Lower bucket is better, so it must compare *greater*: reverse it.
        other
            .bucket
            .cmp(&self.bucket)
            .then_with(|| self.score.cmp(&other.score))
    }
}

impl PartialOrd for QuotaRank {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Rank one candidate account for quota-first routing. Returns `None` when the
/// account cannot serve (a window allowance is spent) so the caller drops it;
/// otherwise a [`QuotaRank`] where greater is preferred.
///
/// `is_last_used` is the per-request affinity signal: `true` when this account
/// is the one the current conversation was last routed to. `insertion_index` is
/// the account's position in the operator's configured order, used only as a
/// deterministic tiebreak (earlier-added wins an exact tie). `tie_band_ms`
/// quantizes the reset clock — see [`DEFAULT_RESET_TIE_BAND_MS`].
#[must_use]
pub fn quota_rank(
    state: &QuotaState,
    is_last_used: bool,
    insertion_index: u32,
    weights: &QuotaWeights,
    tie_band_ms: u64,
) -> Option<QuotaRank> {
    if state.exhausted {
        return None;
    }

    let bucket = match &state.reset {
        Some(window) => window.ms_until_reset / tie_band_ms.max(1),
        None => u64::MAX,
    };

    let affinity = if is_last_used {
        i64::from(weights.affinity)
    } else {
        0
    };
    let headroom = i64::from(weights.headroom) * i64::from(state.rate_headroom_permille) / 1000;
    let pressure = if state.rate_pressured {
        i64::from(weights.pressure)
    } else {
        0
    };
    // Insertion order is only ever a tiebreak, so it is subtracted raw: it can
    // never bridge a difference in any weighted term inside a bucket.
    let score = affinity + headroom - pressure - i64::from(insertion_index);

    Some(QuotaRank { bucket, score })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIVE_H: u64 = 5 * 60 * 60 * 1000;
    const W: QuotaWeights = QuotaWeights {
        affinity: 300_000,
        headroom: 200_000,
        pressure: 500_000,
    };
    const TIE: u64 = DEFAULT_RESET_TIE_BAND_MS;

    fn rank(state: &QuotaState, last_used: bool, index: u32) -> QuotaRank {
        quota_rank(state, last_used, index, &W, TIE).expect("routable")
    }

    #[test]
    fn the_soonest_resetting_window_outranks_everything_else() {
        // A is closing in 20 min; B is a full 5 hours away.
        let a = QuotaState::open(20 * 60 * 1000);
        let b = QuotaState::open(FIVE_H);
        // Even if B is the warmed, un-pressured, full-headroom account, A wins:
        // reset bucket dominates the secondary score entirely.
        assert!(rank(&a, false, 5) > rank(&b, true, 0));
    }

    #[test]
    fn urgency_is_absolute_not_proportional_to_window_length() {
        // The bug the first cut had: 30 min from reset must be equally urgent
        // whether the window is 5 hours or a month long — it is the absolute
        // time to reset that decides, because the waste lands at that instant.
        let half_hour = 30 * 60 * 1000;
        let short_window = QuotaState::open(half_hour); // was a 5h window
        let long_window = QuotaState::open(half_hour); // was a monthly window
        assert_eq!(
            rank(&short_window, false, 0).bucket,
            rank(&long_window, false, 0).bucket,
        );
    }

    #[test]
    fn resets_within_the_tie_band_share_a_bucket() {
        let a = QuotaState::open(20 * 60 * 1000);
        let b = QuotaState::open(20 * 60 * 1000 + TIE / 2);
        assert_eq!(rank(&a, false, 0).bucket, rank(&b, false, 0).bucket);
        // A second past the band is a different (later) bucket.
        let c = QuotaState::open(20 * 60 * 1000 + TIE * 2);
        assert!(rank(&c, false, 0).bucket > rank(&a, false, 0).bucket);
    }

    #[test]
    fn a_warmed_account_keeps_its_edge_while_it_is_not_pressured() {
        // Same reset ⇒ same bucket; affinity should decide.
        let warmed = QuotaState::open(FIVE_H / 2);
        let fresh = QuotaState::open(FIVE_H / 2);
        assert!(rank(&warmed, true, 0) > rank(&fresh, false, 1));
    }

    #[test]
    fn a_pressured_warmed_account_spills_to_a_free_one() {
        // The warmed account is at its rate limit; the fresh one is wide open.
        let warmed_pressured = QuotaState {
            reset: Some(ResetWindow {
                ms_until_reset: FIVE_H / 2,
                remaining_permille: 1000,
            }),
            rate_headroom_permille: 0,
            rate_pressured: true,
            exhausted: false,
        };
        let fresh_free = QuotaState::open(FIVE_H / 2);
        assert!(
            rank(&fresh_free, false, 1) > rank(&warmed_pressured, true, 0),
            "a throttled warmed account must yield to a free one"
        );
    }

    #[test]
    fn an_exhausted_window_is_unroutable() {
        let spent = QuotaState {
            reset: Some(ResetWindow {
                ms_until_reset: 60_000,
                remaining_permille: 0,
            }),
            rate_headroom_permille: 1000,
            rate_pressured: false,
            exhausted: true,
        };
        assert_eq!(quota_rank(&spent, true, 0, &W, TIE), None);
    }

    #[test]
    fn a_windowed_account_always_outranks_a_non_windowed_one() {
        // Even a windowed account a full period from resetting must beat
        // pay-as-you-go, which costs money now and never wastes — the correct
        // last resort.
        let windowed = QuotaState::open(FIVE_H);
        let metered = QuotaState::non_windowed();
        assert!(rank(&windowed, false, 1) > rank(&metered, true, 0));
    }

    #[test]
    fn insertion_order_breaks_an_exact_tie_and_nothing_more() {
        let a = QuotaState::open(FIVE_H / 2);
        let b = QuotaState::open(FIVE_H / 2);
        // Identical state, neither warmed: earlier insertion index wins.
        assert!(rank(&a, false, 0) > rank(&b, false, 1));
        // But the gap is a single point — it can never overturn a real signal
        // (e.g. the later-added but warmed account still wins).
        assert!(rank(&b, true, 1) > rank(&a, false, 0));
    }
}

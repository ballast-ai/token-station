//! Active quota fetchers: for providers that do *not* stamp their remaining
//! allowance on every response (so the L2 header path can't see it), poll a
//! dedicated balance endpoint instead. Currently `OpenRouter`'s `GET /api/v1/key`.
//!
//! Only the response *parsing* lives here — pure and unit-testable. The polling
//! loop and the authenticated GET live in the gateway (they need egress + the
//! keychain), so this module reads no clock and touches no network.
//!
//! # A balance is not a window
//!
//! `OpenRouter` credits are a prepaid balance, not an allowance that resets. In
//! quota-first terms that means the *lowest* urgency — a balance that never
//! resets wastes nothing by waiting, so it should be drained only after every
//! time-limited allowance. We model it as a window whose reset is
//! [`u64::MAX`] (effectively never), which sinks it to the bottom of the
//! quota ranking while still gating exhaustion when the balance hits zero.

use serde_json::Value;

use crate::quota_ledger::WindowSnapshot;

/// A never-resetting balance sorts last: its "time to reset" is effectively
/// infinite, so the quota ranker only reaches for it once windowed allowances
/// are spent. The viewer renders a reset this far out as "no scheduled reset".
pub const NEVER_RESETS_MS: u64 = u64::MAX;

/// Credits are dollars-with-cents; scale to milli-units so the integer
/// `WindowSnapshot` keeps three decimals of precision.
const CREDIT_SCALE: f64 = 1000.0;

/// Parse an `OpenRouter` `GET /api/v1/key` body into a balance window, or `None`
/// when the account is unlimited (no `limit`) or the body is unusable.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn parse_openrouter_key(body: &str) -> Option<WindowSnapshot> {
    let value: Value = serde_json::from_str(body).ok()?;
    let data = value.get("data").unwrap_or(&value);

    // No `limit` ⇒ unlimited credits ⇒ no bounded remaining to show. `is_nan`
    // guards against a garbage value slipping past the `<= 0` check.
    let limit = data.get("limit").and_then(Value::as_f64)?;
    if limit.is_nan() || limit <= 0.0 {
        return None;
    }
    // Prefer the provider's own remaining; else derive it from usage.
    let remaining = data
        .get("limit_remaining")
        .and_then(Value::as_f64)
        .or_else(|| {
            data.get("usage")
                .and_then(Value::as_f64)
                .map(|used| limit - used)
        })
        .unwrap_or(limit)
        .clamp(0.0, limit);

    let remaining_permille = ((remaining / limit) * 1000.0).round().clamp(0.0, 1000.0) as u16;
    Some(WindowSnapshot {
        len_ms: 0,
        limit: (limit * CREDIT_SCALE).round() as u64,
        used: ((limit - remaining) * CREDIT_SCALE).round() as u64,
        remaining_permille,
        ms_until_reset: NEVER_RESETS_MS,
    })
}

/// Whether an upstream's base URL is `OpenRouter`, i.e. its `/key` balance
/// endpoint should be polled. Host-based so a self-hosted proxy of a different
/// provider is not mistaken for it.
#[must_use]
pub fn is_openrouter(base_url: &str) -> bool {
    base_url.contains("openrouter.ai")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_typical_openrouter_balance() {
        let body = r#"{"data":{"label":"sk-...","limit":10.0,"usage":3.5,"limit_remaining":6.5,"is_free_tier":false}}"#;
        let window = parse_openrouter_key(body).expect("a bounded balance");
        assert_eq!(window.remaining_permille, 650);
        assert_eq!(window.ms_until_reset, NEVER_RESETS_MS);
        assert_eq!(window.limit, 10_000); // 10.0 credits in milli-units
        assert_eq!(window.used, 3_500);
    }

    #[test]
    fn derives_remaining_from_usage_when_not_given() {
        let body = r#"{"data":{"limit":20.0,"usage":15.0}}"#;
        let window = parse_openrouter_key(body).expect("bounded");
        assert_eq!(window.remaining_permille, 250);
    }

    #[test]
    fn an_exhausted_balance_reports_zero() {
        let body = r#"{"data":{"limit":5.0,"usage":5.0,"limit_remaining":0.0}}"#;
        let window = parse_openrouter_key(body).expect("bounded");
        assert_eq!(window.remaining_permille, 0);
    }

    #[test]
    fn an_unlimited_or_unusable_account_yields_nothing() {
        // Unlimited: no `limit`.
        assert!(parse_openrouter_key(r#"{"data":{"limit":null,"usage":3.0}}"#).is_none());
        assert!(parse_openrouter_key(r#"{"data":{"usage":3.0}}"#).is_none());
        // Garbage.
        assert!(parse_openrouter_key("not json").is_none());
    }

    #[test]
    fn openrouter_is_detected_by_host() {
        assert!(is_openrouter("https://openrouter.ai/api/v1"));
        assert!(!is_openrouter("https://api.deepseek.com/v1"));
    }
}

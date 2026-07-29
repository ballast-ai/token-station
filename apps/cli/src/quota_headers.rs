//! Parse a provider's own quota / rate-limit response headers into windows.
//!
//! This is the L2 authoritative layer: providers stamp their remaining allowance
//! and reset time on every response, and unlike the local ledger those figures
//! count *all* of the key's usage — no blind spot. The gateway harvests them off
//! a successful response and hands them to the quota tracker.
//!
//! Only the token-quota families that are actually documented are read, and only
//! when limit, remaining, and reset all parse; anything unrecognized is ignored
//! rather than guessed. Pure and clock-injected (`now_ms`), so it is unit-
//! testable against fixed header maps.

use std::collections::BTreeMap;

use crate::quota_ledger::WindowSnapshot;

/// `(limit, remaining, reset)` header names for one token-quota family. Keys are
/// lowercase to match the gateway's normalized header map.
const FAMILIES: &[(&str, &str, &str)] = &[
    // OpenAI-compatible token rate limit.
    (
        "x-ratelimit-limit-tokens",
        "x-ratelimit-remaining-tokens",
        "x-ratelimit-reset-tokens",
    ),
    // Anthropic per-window token limit.
    (
        "anthropic-ratelimit-tokens-limit",
        "anthropic-ratelimit-tokens-remaining",
        "anthropic-ratelimit-tokens-reset",
    ),
    // Anthropic unified subscription window (e.g. the 5-hour plan window).
    (
        "anthropic-ratelimit-unified-limit",
        "anthropic-ratelimit-unified-remaining",
        "anthropic-ratelimit-unified-reset",
    ),
];

/// Every recognized token-quota window the headers report, in family order.
/// Empty when the provider stamped none we understand.
#[must_use]
pub fn parse_quota_windows(headers: &BTreeMap<String, String>, now_ms: u64) -> Vec<WindowSnapshot> {
    FAMILIES
        .iter()
        .filter_map(|(limit_key, remaining_key, reset_key)| {
            window_from(headers, limit_key, remaining_key, reset_key, now_ms)
        })
        .collect()
}

fn window_from(
    headers: &BTreeMap<String, String>,
    limit_key: &str,
    remaining_key: &str,
    reset_key: &str,
    now_ms: u64,
) -> Option<WindowSnapshot> {
    let limit = get_u64(headers, limit_key)?;
    if limit == 0 {
        return None;
    }
    let remaining = get_u64(headers, remaining_key)?.min(limit);
    let ms_until_reset = headers
        .get(reset_key)
        .and_then(|value| parse_reset_ms(value, now_ms))
        .unwrap_or(0);
    let used = limit - remaining;
    let remaining_permille =
        u16::try_from(u128::from(remaining) * 1000 / u128::from(limit)).unwrap_or(1000);
    Some(WindowSnapshot {
        // These headers report remaining + reset, not the window period; the
        // viewer shows remaining and time-to-reset, which need no length.
        len_ms: 0,
        limit,
        used,
        remaining_permille,
        ms_until_reset,
    })
}

fn get_u64(headers: &BTreeMap<String, String>, key: &str) -> Option<u64> {
    headers.get(key)?.trim().parse::<u64>().ok()
}

/// Milliseconds until a reset expressed as any of the forms providers use:
/// an absolute RFC3339 timestamp (Anthropic), a Go-style duration such as
/// `6m0s` / `1s` / `88ms` (OpenAI), or a bare integer number of seconds.
fn parse_reset_ms(value: &str, now_ms: u64) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    // Absolute timestamp → delta from now.
    if value.contains('T') && value.contains('-') {
        return parse_rfc3339_ms(value).map(|epoch_ms| epoch_ms.saturating_sub(now_ms));
    }
    // Bare integer → seconds.
    if value.bytes().all(|b| b.is_ascii_digit()) {
        return value.parse::<u64>().ok().map(|s| s.saturating_mul(1000));
    }
    parse_duration_ms(value)
}

/// A Go-style duration (`1h`, `45s`, `6m0s`, `88ms`, `1.5s`) → milliseconds.
// The accumulator is clamped non-negative and finite before the cast, and a
// reset duration far beyond u64 milliseconds is not a real value we need to keep
// exactly — truncation to whole milliseconds is the intended behavior.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn parse_duration_ms(value: &str) -> Option<u64> {
    let bytes = value.as_bytes();
    let mut i = 0;
    let mut total_ms = 0.0_f64;
    let mut saw_component = false;
    while i < bytes.len() {
        let num_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        if i == num_start {
            return None;
        }
        let num: f64 = value.get(num_start..i)?.parse().ok()?;
        let unit_start = i;
        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        let per_unit_ms = match value.get(unit_start..i)? {
            "ms" => 1.0,
            "s" => 1_000.0,
            "m" => 60_000.0,
            "h" => 3_600_000.0,
            _ => return None,
        };
        total_ms += num * per_unit_ms;
        saw_component = true;
    }
    saw_component.then(|| total_ms.max(0.0) as u64)
}

/// An RFC3339 timestamp (`2026-07-29T12:34:56Z`, offsets and fractions ignored,
/// treated as UTC) → epoch milliseconds. Compact and crate-free.
fn parse_rfc3339_ms(value: &str) -> Option<u64> {
    if value.len() < 19 {
        return None;
    }
    let year: i64 = value.get(0..4)?.parse().ok()?;
    let month: i64 = value.get(5..7)?.parse().ok()?;
    let day: i64 = value.get(8..10)?.parse().ok()?;
    let hour: i64 = value.get(11..13)?.parse().ok()?;
    let minute: i64 = value.get(14..16)?.parse().ok()?;
    let second: i64 = value.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let secs = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(secs).ok()?.checked_mul(1_000)
}

/// Days since the Unix epoch for a civil (proleptic Gregorian) date. Howard
/// Hinnant's `days_from_civil`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn openai_token_headers_with_a_go_duration_reset() {
        let h = headers(&[
            ("x-ratelimit-limit-tokens", "1000"),
            ("x-ratelimit-remaining-tokens", "250"),
            ("x-ratelimit-reset-tokens", "6m0s"),
        ]);
        let windows = parse_quota_windows(&h, 0);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].limit, 1000);
        assert_eq!(windows[0].used, 750);
        assert_eq!(windows[0].remaining_permille, 250);
        assert_eq!(windows[0].ms_until_reset, 360_000);
    }

    #[test]
    fn anthropic_unified_window_with_an_rfc3339_reset() {
        // 1970-01-01T00:01:00Z is 60_000 ms after the epoch.
        let h = headers(&[
            ("anthropic-ratelimit-unified-limit", "500"),
            ("anthropic-ratelimit-unified-remaining", "0"),
            ("anthropic-ratelimit-unified-reset", "1970-01-01T00:01:00Z"),
        ]);
        let windows = parse_quota_windows(&h, 0);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].remaining_permille, 0, "spent unified window");
        assert_eq!(windows[0].ms_until_reset, 60_000);
    }

    #[test]
    fn reset_parses_bare_seconds_and_millis_and_absolute_delta() {
        assert_eq!(parse_reset_ms("30", 0), Some(30_000));
        assert_eq!(parse_reset_ms("88ms", 0), Some(88));
        assert_eq!(parse_reset_ms("1h2m3s", 0), Some(3_723_000));
        // An absolute timestamp already in the past yields zero, not underflow.
        assert_eq!(parse_reset_ms("1970-01-01T00:00:00Z", 10_000), Some(0));
    }

    #[test]
    fn unrecognized_or_incomplete_families_are_ignored() {
        // Remaining without a limit is not a usable window.
        let h = headers(&[("x-ratelimit-remaining-tokens", "100")]);
        assert!(parse_quota_windows(&h, 0).is_empty());
        // A zero limit is not a window (avoids divide-by-zero and false 0‰).
        let z = headers(&[
            ("x-ratelimit-limit-tokens", "0"),
            ("x-ratelimit-remaining-tokens", "0"),
        ]);
        assert!(parse_quota_windows(&z, 0).is_empty());
    }

    #[test]
    fn both_families_present_yield_two_windows() {
        let h = headers(&[
            ("x-ratelimit-limit-tokens", "1000"),
            ("x-ratelimit-remaining-tokens", "900"),
            ("x-ratelimit-reset-tokens", "10s"),
            ("anthropic-ratelimit-tokens-limit", "2000"),
            ("anthropic-ratelimit-tokens-remaining", "2000"),
            ("anthropic-ratelimit-tokens-reset", "5"),
        ]);
        let windows = parse_quota_windows(&h, 0);
        assert_eq!(windows.len(), 2);
    }
}

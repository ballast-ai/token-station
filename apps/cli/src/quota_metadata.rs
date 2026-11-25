//! Read a provider's own quota / rate-limit facts into windows.
//!
//! This is the L2 authoritative layer: providers stamp their remaining allowance
//! and reset time on every response, and unlike the local ledger those figures
//! count *all* of the key's usage — no blind spot. The gateway harvests them off
//! a successful response and hands them to the quota tracker.
//!
//! The input is South's typed `ProviderQuotaMetadataV1`. A South component call
//! returns it directly. A response read from real headers gets it from the
//! canonical header names, with [`quota_metadata_from_headers`].
//!
//! Only the token-quota families that are actually documented are read, and only
//! when limit, remaining, and reset all parse; anything unrecognized is ignored
//! rather than guessed. Pure and clock-injected (`now_ms`), so it is unit-
//! testable against fixed metadata.

use std::collections::BTreeMap;

use south_contracts::{
    MAX_PROVIDER_QUOTA_METADATA_VALUE_BYTES, ProviderQuotaMetadataFieldV1 as Field,
    ProviderQuotaMetadataV1,
};

use crate::quota_ledger::WindowSnapshot;

/// `(limit, remaining, reset)` fields for one token-quota family.
const FAMILIES: &[(Field, Field, Field)] = &[
    // OpenAI-compatible token rate limit.
    (
        Field::XRateLimitLimitTokens,
        Field::XRateLimitRemainingTokens,
        Field::XRateLimitResetTokens,
    ),
    // Anthropic per-window token limit.
    (
        Field::AnthropicRateLimitTokensLimit,
        Field::AnthropicRateLimitTokensRemaining,
        Field::AnthropicRateLimitTokensReset,
    ),
    // Anthropic unified subscription window (e.g. the 5-hour plan window).
    (
        Field::AnthropicRateLimitUnifiedLimit,
        Field::AnthropicRateLimitUnifiedRemaining,
        Field::AnthropicRateLimitUnifiedReset,
    ),
];

/// Reads the quota fields from real response headers, under their canonical
/// names: South's canonical quota declaration. Keys are lowercase, as in the
/// gateway's normalized header map.
///
/// A value South would refuse (over its length bound, or not header text) is
/// dropped, as South's transport drops it.
#[must_use]
pub fn quota_metadata_from_headers(headers: &BTreeMap<String, String>) -> ProviderQuotaMetadataV1 {
    let fields = Field::ALL.into_iter().filter_map(|field| {
        let value = headers.get(field.as_header_name())?;
        (value.len() <= MAX_PROVIDER_QUOTA_METADATA_VALUE_BYTES
            && value
                .bytes()
                .all(|byte| byte == b'\t' || (b' '..=b'~').contains(&byte)))
        .then(|| (field, value.clone()))
    });
    // Each field is read once and each value was checked, so South accepts it.
    ProviderQuotaMetadataV1::try_from_iter(fields).unwrap_or_default()
}

/// Every recognized token-quota window the metadata reports, in family order.
/// Empty when the provider reported none we understand.
#[must_use]
pub fn parse_quota_windows(metadata: &ProviderQuotaMetadataV1, now_ms: u64) -> Vec<WindowSnapshot> {
    parse_quota_reports(metadata, now_ms)
        .into_iter()
        .map(|(_, window)| window)
        .collect()
}

/// Keep each family's identity so a partial response updates only that family
/// instead of deleting independent exhaustion gates. The identity is the limit
/// field's canonical header name.
#[must_use]
pub fn parse_quota_reports(
    metadata: &ProviderQuotaMetadataV1,
    now_ms: u64,
) -> Vec<(String, WindowSnapshot)> {
    FAMILIES
        .iter()
        .filter_map(|&(limit, remaining, reset)| {
            window_from(metadata, limit, remaining, reset, now_ms)
                .map(|window| (limit.as_header_name().to_owned(), window))
        })
        .collect()
}

fn window_from(
    metadata: &ProviderQuotaMetadataV1,
    limit_field: Field,
    remaining_field: Field,
    reset_field: Field,
    now_ms: u64,
) -> Option<WindowSnapshot> {
    let limit = get_u64(metadata, limit_field)?;
    if limit == 0 {
        return None;
    }
    let remaining = get_u64(metadata, remaining_field)?.min(limit);
    let ms_until_reset = metadata
        .value(reset_field)
        .and_then(|value| parse_reset_ms(value, now_ms))?;
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

fn get_u64(metadata: &ProviderQuotaMetadataV1, field: Field) -> Option<u64> {
    metadata.value(field)?.trim().parse::<u64>().ok()
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

/// An RFC3339 timestamp with its stated offset and fractional seconds.
fn parse_rfc3339_ms(value: &str) -> Option<u64> {
    let timestamp =
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()?;
    u64::try_from(timestamp.unix_timestamp_nanos() / 1_000_000).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Metadata as a real response with these headers carries it.
    fn headers(pairs: &[(&str, &str)]) -> ProviderQuotaMetadataV1 {
        quota_metadata_from_headers(
            &pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn real_headers_become_metadata_under_their_canonical_names_only() {
        let long = "9".repeat(MAX_PROVIDER_QUOTA_METADATA_VALUE_BYTES + 1);
        let metadata = headers(&[
            ("x-ratelimit-limit-tokens", "1000"),
            ("x-ratelimit-remaining-tokens", long.as_str()),
            ("x-ratelimit-reset-tokens", "6m0s"),
            ("anthropic-ratelimit-tokens-limit", "2000"),
            ("x-codex-primary-used-percent", "50"),
            ("content-type", "application/json"),
        ]);
        assert_eq!(metadata.present_field_count(), 3);
        assert_eq!(metadata.value(Field::XRateLimitLimitTokens), Some("1000"));
        // Over South's bound: dropped, as South's transport drops it.
        assert_eq!(metadata.value(Field::XRateLimitRemainingTokens), None);
        assert_eq!(metadata.value(Field::XRateLimitResetTokens), Some("6m0s"));
        assert_eq!(
            metadata.value(Field::AnthropicRateLimitTokensLimit),
            Some("2000")
        );
        // The family without a remaining value is not a window.
        assert!(parse_quota_windows(&metadata, 0).is_empty());
        assert_eq!(
            quota_metadata_from_headers(&BTreeMap::new()),
            ProviderQuotaMetadataV1::default()
        );
    }

    /// The parser reads fields. A South response carries them with no header
    /// map at all.
    #[test]
    fn metadata_without_any_header_is_read() {
        let metadata = ProviderQuotaMetadataV1::try_from_iter([
            (Field::AnthropicRateLimitUnifiedLimit, "500".to_owned()),
            (Field::AnthropicRateLimitUnifiedRemaining, "125".to_owned()),
            (
                Field::AnthropicRateLimitUnifiedReset,
                "1970-01-01T00:01:00Z".to_owned(),
            ),
        ])
        .unwrap();
        let reports = parse_quota_reports(&metadata, 0);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].0, "anthropic-ratelimit-unified-limit");
        assert_eq!(reports[0].1.remaining_permille, 250);
        assert_eq!(reports[0].1.ms_until_reset, 60_000);
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

    #[test]
    fn provider_reset_honors_offsets_fractions_and_real_calendar_dates() {
        let h = headers(&[
            ("anthropic-ratelimit-tokens-limit", "1000"),
            ("anthropic-ratelimit-tokens-remaining", "250"),
            (
                "anthropic-ratelimit-tokens-reset",
                "1970-01-01T08:01:00.125+08:00",
            ),
        ]);
        assert_eq!(parse_quota_windows(&h, 0)[0].ms_until_reset, 60_125);
        for reset in [
            "2030-02-30T00:00:00Z",
            "2030-01-01T00:00:00",
            "2030-01-01T00:00:00+25:00",
        ] {
            let h = headers(&[
                ("anthropic-ratelimit-tokens-limit", "1000"),
                ("anthropic-ratelimit-tokens-remaining", "250"),
                ("anthropic-ratelimit-tokens-reset", reset),
            ]);
            assert!(parse_quota_windows(&h, 0).is_empty(), "{reset}");
        }
    }

    #[test]
    fn quota_reports_keep_distinct_header_family_identifiers() {
        let h = headers(&[
            ("x-ratelimit-limit-tokens", "1000"),
            ("x-ratelimit-remaining-tokens", "0"),
            ("x-ratelimit-reset-tokens", "60"),
            ("anthropic-ratelimit-tokens-limit", "2000"),
            ("anthropic-ratelimit-tokens-remaining", "500"),
            ("anthropic-ratelimit-tokens-reset", "120"),
        ]);
        let reports = parse_quota_reports(&h, 0);
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].0, "x-ratelimit-limit-tokens");
        assert_eq!(reports[1].0, "anthropic-ratelimit-tokens-limit");
        assert_eq!(reports[0].1.remaining_permille, 0);
        assert_eq!(reports[1].1.remaining_permille, 250);
    }
}

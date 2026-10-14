//! Active quota fetchers: for providers that do *not* stamp their remaining
//! allowance on every response (so the L2 header path can't see it), poll a
//! dedicated read-only endpoint. Recognized sources include `OpenRouter` balances
//! and Kimi Code, `MiniMax`, and Zhipu/Z.ai plan windows.
//!
//! Only the response *parsing* lives here — pure and unit-testable. The polling
//! loop and the authenticated GET live in the gateway (they need egress + the
//! secrets store), so this module reads no clock and touches no network.
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

    // No limit is unknown/unlimited. A zero limit is an explicit exhausted cap.
    let limit = data.get("limit").and_then(Value::as_f64)?;
    if !limit.is_finite() || limit < 0.0 {
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
        })?;
    if !remaining.is_finite() || remaining > limit {
        return None;
    }
    let remaining = remaining.max(0.0);

    let remaining_permille = if limit == 0.0 {
        0
    } else {
        let scaled = ((remaining / limit) * 1000.0).round() as u16;
        if remaining > 0.0 { scaled.max(1) } else { 0 }
    };
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
    let Ok(url) = url::Url::parse(base_url) else {
        return false;
    };
    trusted_url(&url)
        && url.host_str() == Some("openrouter.ai")
        && url.path().trim_end_matches('/') == "/api/v1"
}

fn trusted_url(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuotaProvider {
    OpenRouter,
    KimiCode,
    MiniMax,
    Zhipu,
}

/// A recognized read-only endpoint. Its origin is fixed by the provider contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaEndpoint {
    pub url: String,
    provider: QuotaProvider,
}

impl QuotaEndpoint {
    /// The `MiniMax` general pool counts the text model family. Other pools must
    /// retain their model scope instead of being attached to the whole upstream.
    #[must_use]
    pub fn accepts_models<'a>(&self, models: impl IntoIterator<Item = &'a str>) -> bool {
        if self.provider != QuotaProvider::MiniMax {
            return true;
        }
        let mut models = models.into_iter().peekable();
        models.peek().is_some()
            && models.all(|model| model.to_ascii_lowercase().starts_with("minimax-m"))
    }

    #[must_use]
    pub fn parse_response(
        &self,
        status: u16,
        body: &str,
        now_ms: u64,
    ) -> Option<Vec<WindowSnapshot>> {
        (200..300)
            .contains(&status)
            .then(|| self.parse(body, now_ms))
            .flatten()
    }
    #[must_use]
    pub fn authorization(&self, key: &str) -> String {
        if self.provider == QuotaProvider::Zhipu {
            key.to_owned()
        } else {
            format!("Bearer {key}")
        }
    }

    /// Normalize complete provider reports into percentage windows. A failed report
    /// returns `None`, so the caller retains the previous reading.
    #[must_use]
    pub fn parse(&self, body: &str, now_ms: u64) -> Option<Vec<WindowSnapshot>> {
        match self.provider {
            QuotaProvider::OpenRouter => parse_openrouter_key(body).map(|w| vec![w]),
            QuotaProvider::KimiCode => parse_kimi(body, now_ms),
            QuotaProvider::MiniMax => parse_minimax(body, now_ms),
            QuotaProvider::Zhipu => parse_zhipu(body, now_ms),
        }
    }
}

fn number(value: &Value) -> Option<f64> {
    let n = match value {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.parse().ok()?,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

fn timestamp_ms(value: &Value) -> Option<u64> {
    let timestamp = time::OffsetDateTime::parse(
        value.as_str()?,
        &time::format_description::well_known::Rfc3339,
    )
    .ok()?;
    u64::try_from(timestamp.unix_timestamp_nanos() / 1_000_000).ok()
}

// Provider plan points and percentages are not token counts. Use a normalized
// 1000-point allowance while retaining the provider's window duration and clock.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn percent_window(
    remaining: f64,
    len_ms: u64,
    reset_ms: u64,
    now_ms: u64,
) -> Option<WindowSnapshot> {
    if !remaining.is_finite() || !(0.0..=100.0).contains(&remaining) || len_ms == 0 {
        return None;
    }
    let until = reset_ms.checked_sub(now_ms).filter(|n| *n > 0)?;
    let scaled = (remaining * 10.0).floor() as u16;
    let remaining_permille = if remaining > 0.0 { scaled.max(1) } else { 0 };
    Some(WindowSnapshot {
        len_ms,
        limit: 1000,
        used: u64::from(1000 - remaining_permille),
        remaining_permille,
        ms_until_reset: until,
    })
}

fn kimi_window(detail: &Value, len_ms: u64, now_ms: u64) -> Option<WindowSnapshot> {
    let limit = number(detail.get("limit")?)?;
    if limit <= 0.0 {
        return None;
    }
    let remaining = if let Some(left) = detail.get("remaining") {
        number(left)?
    } else {
        limit - number(detail.get("used")?)?
    };
    if remaining < 0.0 || remaining > limit {
        return None;
    }
    let reset = detail
        .get("resetTime")
        .or_else(|| detail.get("resetAt"))
        .or_else(|| detail.get("reset_at"))
        .or_else(|| detail.get("reset_time"))?;
    percent_window(
        remaining / limit * 100.0,
        len_ms,
        timestamp_ms(reset)?,
        now_ms,
    )
}

fn parse_kimi(body: &str, now_ms: u64) -> Option<Vec<WindowSnapshot>> {
    let root: Value = serde_json::from_str(body).ok()?;
    let mut windows = Vec::new();
    {
        for row in root.get("limits")?.as_array()? {
            let window = row.get("window")?;
            let duration = number(window.get("duration")?)?;
            if duration <= 0.0 || duration.fract() != 0.0 || duration > 1_000_000.0 {
                return None;
            }
            let unit_ms = match window.get("timeUnit")?.as_str()? {
                "TIME_UNIT_MINUTE" => 60_000,
                "TIME_UNIT_HOUR" => 3_600_000,
                "TIME_UNIT_DAY" => 86_400_000,
                _ => return None,
            };
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let len_ms = (duration as u64).checked_mul(unit_ms)?;
            windows.push(kimi_window(
                row.get("detail").unwrap_or(window),
                len_ms,
                now_ms,
            )?);
        }
    }
    // The whole-budget report is required. Accepting only shorter windows
    // would discard a weekly exhaustion gate from the account's report.
    windows.push(kimi_window(root.get("usage")?, 604_800_000, now_ms)?);
    Some(windows)
}

fn minimax_timestamp(value: &Value) -> Option<u64> {
    let n = value.as_u64()?;
    if n < 1_000_000_000_000 {
        n.checked_mul(1000)
    } else {
        Some(n)
    }
}

fn parse_minimax(body: &str, now_ms: u64) -> Option<Vec<WindowSnapshot>> {
    let root: Value = serde_json::from_str(body).ok()?;
    if root.get("base_resp")?.get("status_code")?.as_u64()? != 0 {
        return None;
    }
    let mut general = root.get("model_remains")?.as_array()?.iter().filter(|row| {
        row.get("model_name")
            .and_then(Value::as_str)
            .is_some_and(|name| name.eq_ignore_ascii_case("general"))
    });
    let row = general.next()?;
    if general.next().is_some() {
        return None;
    }
    let mut windows = Vec::new();
    for (status_key, remaining_key, start_key, end_key, weekly) in [
        (
            "current_interval_status",
            "current_interval_remaining_percent",
            "start_time",
            "end_time",
            false,
        ),
        (
            "current_weekly_status",
            "current_weekly_remaining_percent",
            "weekly_start_time",
            "weekly_end_time",
            true,
        ),
    ] {
        let status = row.get(status_key)?.as_u64()?;
        if status == 3 {
            continue; // Explicit unlimited windows do not gate the general pool.
        }
        let remaining = match status {
            1 => number(row.get(remaining_key)?)?,
            2 => 0.0, // The explicit exhausted status is stronger than its percentage.
            _ => return None,
        };
        let reset = minimax_timestamp(row.get(end_key)?)?;
        let len_ms = match row.get(start_key) {
            Some(start) => reset.checked_sub(minimax_timestamp(start)?)?,
            None if weekly => 604_800_000,
            None => return None,
        };
        windows.push(percent_window(remaining, len_ms, reset, now_ms)?);
    }
    (!windows.is_empty()).then_some(windows)
}

fn parse_zhipu(body: &str, now_ms: u64) -> Option<Vec<WindowSnapshot>> {
    let root: Value = serde_json::from_str(body).ok()?;
    if !root.get("success")?.as_bool()? {
        return None;
    }
    let limits = root.get("data")?.get("limits")?.as_array()?;
    let mut windows = Vec::new();
    for row in limits {
        match row.get("type")?.as_str()? {
            "TIME_LIMIT" => continue, // MCP calls do not block model requests.
            "TOKENS_LIMIT" => {}
            _ => return None,
        }
        // This host tracks whole-upstream windows. It cannot safely discard
        // a new provider field that narrows a quota to only some models.
        if ["model", "model_name", "modelName", "pool"]
            .iter()
            .any(|field| row.get(field).is_some())
        {
            return None;
        }
        let len_ms = match row.get("unit")?.as_u64()? {
            3 => match row.get("number") {
                Some(Value::Null) | None => 18_000_000, // Team reports omit the five-hour number.
                Some(n) => match n.as_u64()? {
                    0 => 18_000_000,
                    n @ 1..=24 => n.checked_mul(3_600_000)?,
                    _ => return None,
                },
            },
            // Both 1 and 7 are reported for the same one-week budget.
            6 => match row.get("number")?.as_u64()? {
                1 | 7 => 604_800_000,
                _ => return None,
            },
            _ => return None,
        };
        let used = number(row.get("percentage")?)?;
        let reset = row.get("nextResetTime")?.as_u64()?;
        windows.push(percent_window(100.0 - used, len_ms, reset, now_ms)?);
    }
    (!windows.is_empty()).then_some(windows)
}

/// Recognize official API-key quota endpoints without following arbitrary base paths.
#[must_use]
pub fn quota_endpoint(base_url: &str) -> Option<QuotaEndpoint> {
    let url = url::Url::parse(base_url).ok()?;
    if !trusted_url(&url) {
        return None;
    }
    let host = url.host_str()?;
    let path = url.path().trim_end_matches('/');
    let (provider, quota_path) = match (host, path) {
        ("openrouter.ai", "/api/v1") => (QuotaProvider::OpenRouter, "/api/v1/key"),
        ("api.kimi.com" | "api.kimi.ai", "/coding" | "/coding/v1") => {
            (QuotaProvider::KimiCode, "/coding/v1/usages")
        }
        ("api.minimaxi.com" | "api.minimax.io", "/v1" | "/anthropic" | "/anthropic/v1") => {
            (QuotaProvider::MiniMax, "/v1/token_plan/remains")
        }
        ("open.bigmodel.cn" | "api.z.ai", "/api/coding/paas/v4") => {
            (QuotaProvider::Zhipu, "/api/monitor/usage/quota/limit")
        }
        _ => return None,
    };
    Some(QuotaEndpoint {
        url: format!("https://{host}{quota_path}"),
        provider,
    })
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

    #[test]
    fn unknown_openrouter_balance_and_untrusted_origins_are_not_accepted() {
        // Synthetic contract fixtures. These do not verify a live account.
        assert!(parse_openrouter_key(r#"{"data":{"limit":10}}"#).is_none());
        assert!(parse_openrouter_key(r#"{"data":{"limit":10,"usage":-1}}"#).is_none());
        for base in [
            "https://openrouter.ai.evil.example/api/v1",
            "https://evil.example/openrouter.ai/api/v1",
            "http://openrouter.ai/api/v1",
            "https://openrouter.ai:444/api/v1",
            "https://key@openrouter.ai/api/v1",
            "https://openrouter.ai/private-proxy",
        ] {
            assert!(!is_openrouter(base), "{base}");
        }
    }

    #[test]
    fn plan_endpoints_use_exact_official_origins_and_auth_contracts() {
        let kimi = quota_endpoint("https://api.kimi.com/coding/v1").expect("coding key");
        assert_eq!(kimi.url, "https://api.kimi.com/coding/v1/usages");
        assert_eq!(kimi.authorization("test-key"), "Bearer test-key");
        let minimax = quota_endpoint("https://api.minimax.io/v1").expect("plan vendor");
        assert_eq!(minimax.url, "https://api.minimax.io/v1/token_plan/remains");
        let zhipu =
            quota_endpoint("https://open.bigmodel.cn/api/coding/paas/v4").expect("coding key");
        assert_eq!(
            zhipu.url,
            "https://open.bigmodel.cn/api/monitor/usage/quota/limit"
        );
        assert_eq!(zhipu.authorization("test-key"), "test-key");
        for base in [
            "https://api.kimi.com/v1",
            "https://api.kimi.com/coding/v1/proxy",
            "https://api.kimi.com.evil.example/coding/v1",
            "https://localhost/coding/v1",
            "https://api.minimax.io/v1?forward=1",
            "http://api.minimaxi.com/v1",
            "https://api.z.ai/api/paas/v4",
            "https://open.bigmodel.cn/api/coding/paas/v4#x",
        ] {
            assert!(quota_endpoint(base).is_none(), "{base}");
        }
    }

    #[test]
    fn kimi_windows_keep_both_limits_and_absolute_reset_clocks() {
        // Synthetic, credential-free fixtures based on the pinned upstream wire contract.
        let endpoint = quota_endpoint("https://api.kimi.com/coding/v1").unwrap();
        let body = r#"{"usage":{"limit":"100","used":"40","resetTime":"2030-01-08T00:00:00Z"},"limits":[{"window":{"duration":300,"timeUnit":"TIME_UNIT_MINUTE"},"detail":{"limit":100,"remaining":75,"resetTime":"2030-01-01T13:00:00.500+08:00"}}]}"#;
        let windows = endpoint
            .parse(body, 1_893_456_000_000)
            .expect("valid windows");
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].len_ms, 18_000_000);
        assert_eq!(windows[0].remaining_permille, 750);
        assert_eq!(windows[0].ms_until_reset, 18_000_500);
        assert_eq!(windows[1].len_ms, 604_800_000);
        assert_eq!(windows[1].remaining_permille, 600);
        assert_eq!(windows[1].ms_until_reset, 604_800_000);
        assert!(
            endpoint
                .parse(
                    r#"{"usage":{"limit":100,"resetTime":"2030-01-08T00:00:00Z"}}"#,
                    0
                )
                .is_none()
        );
    }

    #[test]
    fn minimax_general_windows_do_not_inherit_video_pool_exhaustion() {
        // Synthetic fixture. Only the unique general bucket gates text models.
        let endpoint = quota_endpoint("https://api.minimaxi.com/v1").unwrap();
        let body = r#"{"base_resp":{"status_code":0},"model_remains":[{"model_name":"general","start_time":1893456000,"end_time":1893474000,"current_interval_remaining_percent":75,"current_interval_status":1,"weekly_start_time":1893456000000,"weekly_end_time":1894060800000,"current_weekly_remaining_percent":80,"current_weekly_status":2},{"model_name":"video","current_interval_status":2,"current_weekly_status":2}]}"#;
        let windows = endpoint
            .parse(body, 1_893_456_000_000)
            .expect("general pool");
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].remaining_permille, 750);
        assert_eq!(windows[0].ms_until_reset, 18_000_000);
        assert_eq!(
            windows[1].remaining_permille, 0,
            "status overrides advertised remaining"
        );
        assert_eq!(windows[1].len_ms, 604_800_000);
        assert!(
            endpoint
                .parse(
                    r#"{"base_resp":{"status_code":1004},"model_remains":[]}"#,
                    0
                )
                .is_none()
        );
        assert!(endpoint.parse(r#"{"base_resp":{"status_code":0},"model_remains":[{"model_name":"general"},{"model_name":"general"}]}"#, 0).is_none());
    }

    #[test]
    fn zhipu_token_windows_exclude_mcp_counts_and_require_complete_reports() {
        let endpoint = quota_endpoint("https://api.z.ai/api/coding/paas/v4").unwrap();
        let body = r#"{"success":true,"data":{"limits":[{"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":12,"nextResetTime":1893474000000},{"type":"TOKENS_LIMIT","unit":6,"number":1,"percentage":100,"nextResetTime":1894060800000},{"type":"TIME_LIMIT","unit":5,"number":1,"percentage":100}]}}"#;
        let windows = endpoint
            .parse(body, 1_893_456_000_000)
            .expect("token allowance");
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].remaining_permille, 880);
        assert_eq!(windows[1].remaining_permille, 0);
        assert_eq!(windows[1].ms_until_reset, 604_800_000);
        assert!(endpoint.parse(r#"{"success":true,"data":{"limits":[{"type":"TOKENS_LIMIT","unit":3,"number":5,"nextResetTime":1893474000000}]}}"#, 0).is_none());
        assert!(
            endpoint
                .parse(r#"{"success":false,"data":{"limits":[]}}"#, 0)
                .is_none()
        );
    }

    #[test]
    fn zero_cap_and_overdrawn_balances_remain_exhausted() {
        for body in [
            r#"{"data":{"limit":0,"usage":0,"limit_remaining":0}}"#,
            r#"{"data":{"limit":10,"usage":11}}"#,
            r#"{"data":{"limit":10,"limit_remaining":-1}}"#,
        ] {
            let w = parse_openrouter_key(body).expect("known exhausted balance");
            assert_eq!(w.remaining_permille, 0);
            assert_eq!(w.ms_until_reset, NEVER_RESETS_MS);
        }
    }

    #[test]
    fn polling_rejects_error_responses_and_models_outside_the_general_pool() {
        let endpoint = quota_endpoint("https://openrouter.ai/api/v1").unwrap();
        let body = r#"{"data":{"limit":10,"usage":3}}"#;
        assert!(endpoint.parse_response(200, body, 0).is_some());
        for status in [301, 401, 429, 500] {
            assert!(endpoint.parse_response(status, body, 0).is_none());
        }
        let mini = quota_endpoint("https://api.minimax.io/v1").unwrap();
        assert!(mini.accepts_models(["MiniMax-M2.5", "MiniMax-M3"]));
        assert!(!mini.accepts_models(["MiniMax-M3", "video-01"]));
        assert!(!mini.accepts_models(["image-01"]));
        assert!(!mini.accepts_models([]));
    }

    #[test]
    fn tiny_positive_provider_allowances_are_not_marked_exhausted() {
        let or = parse_openrouter_key(r#"{"data":{"limit":10,"limit_remaining":0.0001}}"#).unwrap();
        assert_eq!(or.remaining_permille, 1);
        let kimi = quota_endpoint("https://api.kimi.com/coding/v1").unwrap();
        let body = r#"{"usage":{"limit":100,"remaining":0.001,"resetTime":"2030-01-08T00:00:00Z"},"limits":[]}"#;
        assert_eq!(kimi.parse(body, 0).unwrap()[0].remaining_permille, 1);
    }

    #[test]
    fn malformed_or_partial_plan_reports_do_not_replace_a_complete_reading() {
        let kimi = quota_endpoint("https://api.kimi.com/coding/v1").unwrap();
        for body in [
            r#"{"usage":{"limit":100,"used":10,"resetTime":"2030-01-08T00:00:00Z"}}"#,
            r#"{"usage":{"limit":"NaN","used":1,"resetTime":"2030-01-08T00:00:00Z"},"limits":[]}"#,
            r#"{"usage":{"limit":100,"used":101,"resetTime":"2030-01-08T00:00:00Z"},"limits":[]}"#,
            r#"{"usage":{"limit":100,"used":-1,"resetTime":"2030-01-08T00:00:00Z"},"limits":[]}"#,
            r#"{"usage":{"limit":100,"used":10,"resetTime":"2030-02-30T00:00:00Z"},"limits":[]}"#,
            r#"{"usage":{"limit":100,"used":10,"resetTime":"2030-01-08T00:00:00Z"},"limits":[{"window":{"duration":300,"timeUnit":"TIME_UNIT_MINUTE"},"detail":{"limit":100,"remaining":70}}]}"#,
        ] {
            assert!(kimi.parse(body, 0).is_none(), "{body}");
        }
        let zhipu = quota_endpoint("https://api.z.ai/api/coding/paas/v4").unwrap();
        for body in [
            r#"{"success":true,"data":{"limits":[{"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":-1,"nextResetTime":1893474000000}]}}"#,
            r#"{"success":true,"data":{"limits":[{"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":101,"nextResetTime":1893474000000}]}}"#,
            r#"{"success":true,"data":{"limits":[{"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":20,"nextResetTime":1893474000000,"model_name":"glm-special"}]}}"#,
        ] {
            assert!(zhipu.parse(body, 0).is_none(), "{body}");
        }
    }
}

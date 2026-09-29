//! Content-free accounting observations from provider response envelopes.

use serde_json::{Value, value::RawValue};
use token_station_metrics::{CostKind, RequestRecord, UsageObservation};
use token_station_protocol::Usage;

fn usage_object(body: &Value) -> Option<&Value> {
    body.get("usage")
        .or_else(|| body.get("message")?.get("usage"))
        .or_else(|| body.get("response")?.get("usage"))
        .or_else(|| body.get("usageMetadata"))
        .filter(|value| value.is_object())
}

fn observe(body: &Value) -> UsageObservation {
    let Some(raw) = usage_object(body) else {
        return UsageObservation::default();
    };
    let count = |key: &str| raw.get(key).and_then(Value::as_u64);
    let detail = |group: &str, key: &str| raw.get(group)?.get(key)?.as_u64();
    let cache_read = count("cache_read_input_tokens")
        .or_else(|| detail("prompt_tokens_details", "cached_tokens"))
        .or_else(|| detail("input_tokens_details", "cached_tokens"))
        .or_else(|| count("prompt_cache_hit_tokens"))
        .or_else(|| count("cachedContentTokenCount"));
    let cache_write = count("cache_creation_input_tokens")
        .or_else(|| detail("prompt_tokens_details", "cache_write_tokens"))
        .or_else(|| detail("input_tokens_details", "cache_write_tokens"));
    let anthropic = raw.get("prompt_tokens").is_none()
        && (raw.get("cache_creation_input_tokens").is_some()
            || raw.get("cache_read_input_tokens").is_some()
            || body.get("message").is_some()
            || body.get("type").and_then(Value::as_str) == Some("message_delta"));
    let invalid_count = |key: &str| {
        raw.get(key)
            .is_some_and(|v| !v.is_null() && v.as_u64().is_none())
    };
    let invalid_detail = |group: &str, keys: &[&str]| {
        raw.get(group).is_some_and(|value| {
            !value.is_null()
                && (!value.is_object()
                    || keys.iter().any(|key| {
                        value
                            .get(key)
                            .is_some_and(|v| !v.is_null() && v.as_u64().is_none())
                    }))
        })
    };
    let incomplete = [
        "prompt_tokens",
        "input_tokens",
        "promptTokenCount",
        "completion_tokens",
        "output_tokens",
        "candidatesTokenCount",
        "thoughtsTokenCount",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
        "prompt_cache_hit_tokens",
        "cachedContentTokenCount",
    ]
    .iter()
    .any(|key| invalid_count(key))
        || invalid_detail(
            "prompt_tokens_details",
            &["cached_tokens", "cache_write_tokens"],
        )
        || invalid_detail(
            "input_tokens_details",
            &["cached_tokens", "cache_write_tokens"],
        )
        || invalid_detail(
            "cache_creation",
            &["ephemeral_5m_input_tokens", "ephemeral_1h_input_tokens"],
        )
        || invalid_detail("completion_tokens_details", &["reasoning_tokens"])
        || invalid_detail("output_tokens_details", &["reasoning_tokens"]);
    let input = count("prompt_tokens")
        .or_else(|| count("input_tokens"))
        .or_else(|| count("promptTokenCount"));
    let input = if anthropic {
        input.and_then(|value| {
            value
                .checked_add(cache_read.unwrap_or(0))?
                .checked_add(cache_write.unwrap_or(0))
        })
    } else {
        input
    };
    let output = count("completion_tokens")
        .or_else(|| count("output_tokens"))
        .or_else(|| {
            count("candidatesTokenCount")
                .and_then(|value| value.checked_add(count("thoughtsTokenCount").unwrap_or(0)))
        });
    UsageObservation {
        incomplete,
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        cache_write_5m_tokens: detail("cache_creation", "ephemeral_5m_input_tokens"),
        cache_write_1h_tokens: detail("cache_creation", "ephemeral_1h_input_tokens"),
        reasoning_tokens: detail("completion_tokens_details", "reasoning_tokens")
            .or_else(|| detail("output_tokens_details", "reasoning_tokens"))
            .or_else(|| count("thoughtsTokenCount")),
    }
}

/// Parse decimal USD into microdollars with checked integer arithmetic.
fn usd_micros(value: &RawValue) -> Option<i64> {
    let raw = value.get();
    let decoded;
    let text = if raw.starts_with('"') {
        decoded = serde_json::from_str::<String>(raw).ok()?;
        decoded.as_str()
    } else {
        raw
    };
    if text.len() > 64 {
        return None;
    }
    let (mantissa, exponent) = text
        .split_once(['e', 'E'])
        .map_or(Some((text, 0i32)), |(m, e)| {
            Some((m, e.parse::<i32>().ok()?))
        })?;
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let digits = format!("{whole}{fraction}").parse::<u128>().ok()?;
    let scale = 6i32
        .checked_add(exponent)?
        .checked_sub(i32::try_from(fraction.len()).ok()?)?;
    let micros = if scale >= 0 {
        digits.checked_mul(10u128.checked_pow(u32::try_from(scale).ok()?)?)?
    } else {
        let divisor = 10u128.checked_pow(scale.unsigned_abs())?;
        digits.checked_add(divisor / 2)? / divisor
    };
    i64::try_from(micros).ok()
}

/// Borrow only the accounting envelope. Raw decimal money never passes through `f64`.
#[derive(serde::Deserialize)]
struct RawCostEnvelope<'a> {
    #[serde(borrow)]
    usage: Option<&'a RawValue>,
    #[serde(borrow)]
    message: Option<&'a RawValue>,
    #[serde(borrow)]
    response: Option<&'a RawValue>,
}

#[derive(serde::Deserialize)]
struct RawCostUsage<'a> {
    #[serde(borrow)]
    cost: Option<&'a RawValue>,
}

/// A per-attempt transient parser. No body bytes leave this object.
#[derive(Default, PartialEq, Eq)]
enum BodyMode {
    #[default]
    Disabled,
    Json,
    Stream,
    Abandoned,
}

#[derive(Default, PartialEq, Eq)]
enum UsageStage {
    #[default]
    Unseen,
    Preliminary,
    Reported,
}

#[derive(Default)]
pub(crate) struct AccountingTap {
    observation: UsageObservation,
    stage: UsageStage,
    actual_usd: Option<i64>,
    trusted_usd: bool,
    mode: BodyMode,
    json: Vec<u8>,
    decoder: crate::sse::SseFrameDecoder,
}

impl AccountingTap {
    pub(crate) fn new(endpoint: &str) -> Self {
        let trusted_usd = url::Url::parse(endpoint).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str() == Some("openrouter.ai")
                && url.port_or_known_default() == Some(443)
                && url.username().is_empty()
                && url.password().is_none()
        });
        Self {
            trusted_usd,
            ..Self::default()
        }
    }

    pub(crate) fn head(&mut self, status: u16, content_type: &str) {
        self.mode = if !(200..300).contains(&status) {
            BodyMode::Disabled
        } else if content_type
            .split(';')
            .next()
            .is_some_and(|s| s.trim().eq_ignore_ascii_case("text/event-stream"))
        {
            BodyMode::Stream
        } else {
            BodyMode::Json
        };
    }

    fn parse_event(&mut self, bytes: &[u8]) {
        let Ok(body) = serde_json::from_slice::<Value>(bytes) else {
            return;
        };
        let cost = if self.trusted_usd {
            serde_json::from_slice::<RawCostEnvelope<'_>>(bytes)
                .ok()
                .and_then(|envelope| {
                    fn nested_usage(raw: &RawValue) -> Option<&RawValue> {
                        serde_json::from_str::<RawCostEnvelope<'_>>(raw.get())
                            .ok()
                            .and_then(|nested| nested.usage)
                    }
                    envelope
                        .usage
                        .or_else(|| envelope.message.and_then(nested_usage))
                        .or_else(|| envelope.response.and_then(nested_usage))
                })
                .and_then(|raw| serde_json::from_str::<RawCostUsage<'_>>(raw.get()).ok())
                .and_then(|usage| usage.cost)
                .and_then(usd_micros)
        } else {
            None
        };
        self.event(&body, cost);
    }

    fn event(&mut self, body: &Value, cost: Option<i64>) {
        if usage_object(body).is_none() {
            return;
        }
        let observation = observe(body);
        match body.get("type").and_then(Value::as_str) {
            Some("message_start") => self.stage = UsageStage::Preliminary,
            Some("message_delta") if observation.output_tokens.is_none() => {}
            _ => self.stage = UsageStage::Reported,
        }
        let raw = usage_object(body).expect("usage object was checked");
        let native_input = raw.get("prompt_tokens").is_none()
            && (body
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with("message_"))
                || raw.get("cache_read_input_tokens").is_some()
                || raw.get("cache_creation_input_tokens").is_some());
        let fresh_input = native_input
            .then(|| {
                raw.get("input_tokens").and_then(Value::as_u64).or_else(|| {
                    self.observation
                        .input_tokens?
                        .checked_sub(self.observation.cache_read_tokens.unwrap_or(0))?
                        .checked_sub(self.observation.cache_write_tokens.unwrap_or(0))
                })
            })
            .flatten();
        self.observation.absorb(observation);
        if let Some(fresh_input) = fresh_input {
            self.observation.input_tokens = fresh_input
                .checked_add(self.observation.cache_read_tokens.unwrap_or(0))
                .and_then(|input| {
                    input.checked_add(self.observation.cache_write_tokens.unwrap_or(0))
                });
        }
        if self.trusted_usd
            && let Some(cost) = cost
        {
            self.actual_usd = Some(cost);
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) {
        if matches!(self.mode, BodyMode::Disabled | BodyMode::Abandoned) {
            return;
        }
        if self.mode == BodyMode::Stream {
            match self.decoder.push(bytes) {
                Ok(frames) => {
                    for frame in frames {
                        let data = frame
                            .lines()
                            .filter_map(|line| line.strip_prefix("data:"))
                            .map(|line| line.strip_prefix(' ').unwrap_or(line))
                            .collect::<Vec<_>>()
                            .join("\n");
                        self.parse_event(data.as_bytes());
                    }
                }
                Err(_) => self.mode = BodyMode::Abandoned,
            }
        } else if self.json.len().saturating_add(bytes.len()) <= 32 * 1024 * 1024 {
            self.json.extend_from_slice(bytes);
        } else {
            self.json.clear();
            self.mode = BodyMode::Abandoned;
        }
    }

    pub(crate) fn finish(mut self, record: &mut RequestRecord) {
        if self.mode == BodyMode::Abandoned {
            // Keep the adapter's final values, but do not certify an incomplete observation.
            record.usage_observation = Some(UsageObservation {
                incomplete: true,
                ..UsageObservation::default()
            });
            if let Some(cost) = self.actual_usd {
                record.cost_kind = CostKind::Actual;
                record.cost_micros = Some(cost);
                record.price_version = None;
            }
            return;
        }
        if self.mode == BodyMode::Json {
            let bytes = std::mem::take(&mut self.json);
            self.parse_event(&bytes);
        }
        if self.stage != UsageStage::Unseen {
            self.observation.incomplete |= self.stage == UsageStage::Preliminary;
            record.usage_observation = Some(self.observation);
            if self.observation == UsageObservation::default() {
                record.usage = None;
            } else {
                self.observation
                    .apply(record.usage.get_or_insert_with(Usage::default));
            }
        } else if record.usage == Some(Usage::default()) {
            // Adapter defaults are not evidence that the provider reported zero.
            record.usage = None;
        }
        if let Some(cost) = self.actual_usd {
            record.cost_kind = CostKind::Actual;
            record.cost_micros = Some(cost);
            record.price_version = None;
        }
    }
}

/// Require complete observed usage and consistent token subsets before estimating.
/// Historical receipts without metadata retain their existing numeric semantics.
pub(crate) fn can_estimate(usage: &Usage, observation: Option<UsageObservation>) -> bool {
    let mut usage = *usage;
    if let Some(observation) = observation {
        if observation.incomplete
            || observation.input_tokens.is_none()
            || observation.output_tokens.is_none()
        {
            return false;
        }
        // Read TTL detail from metadata even when a consumer loaded only SQL columns.
        observation.apply(&mut usage);
    }
    usage
        .cache_read_tokens
        .checked_add(usage.cache_write_tokens)
        .is_some_and(|cache| cache <= usage.input_tokens)
        && usage
            .cache_write_5m_tokens
            .checked_add(usage.cache_write_1h_tokens)
            .is_some_and(|tiers| tiers <= usage.cache_write_tokens)
        && usage.reasoning_tokens <= usage.output_tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn anthropic_start_without_final_usage_is_only_a_partial_observation() {
        let mut tap = AccountingTap::new("https://api.anthropic.com/v1");
        tap.head(200, "text/event-stream");
        tap.push(b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":100,\"output_tokens\":1}}}\n\n");
        tap.push(b"data: {\"type\":\"message_stop\"}\n\n");
        let mut record = RequestRecord::begin(0, "anthropic");
        tap.finish(&mut record);
        assert!(record.usage_observation.unwrap().incomplete);
        assert_eq!(record.usage.unwrap().input_tokens, 100);
        assert!(!can_estimate(
            &record.usage.unwrap(),
            record.usage_observation
        ));
    }

    #[test]
    fn explicit_zero_and_absent_cache_fields_remain_distinct() {
        let absent = observe(&json!({"usage":{"prompt_tokens":10,"completion_tokens":2}}));
        let zero = observe(&json!({"usage":{"prompt_tokens":10,"completion_tokens":2,
            "prompt_tokens_details":{"cached_tokens":0,"cache_write_tokens":0}}}));
        assert_eq!(absent.cache_write_tokens, None);
        assert_eq!(zero.cache_write_tokens, Some(0));
        assert_eq!(zero.cache_read_tokens, Some(0));
    }

    #[test]
    fn anthropic_input_is_inclusive_and_tiers_are_preserved() {
        let value = observe(
            &json!({"message":{"usage":{"input_tokens":10,"output_tokens":0,
            "cache_read_input_tokens":20,"cache_creation_input_tokens":30,
            "cache_creation":{"ephemeral_5m_input_tokens":10,"ephemeral_1h_input_tokens":20}}}}),
        );
        assert_eq!(value.input_tokens, Some(60));
        assert_eq!(value.cache_write_1h_tokens, Some(20));
    }

    #[test]
    fn compatible_prompt_totals_do_not_add_anthropic_cache_extensions_again() {
        let value = observe(&json!({"usage":{"prompt_tokens":60,"completion_tokens":2,
            "cache_read_input_tokens":20,"cache_creation_input_tokens":30}}));
        assert_eq!(value.input_tokens, Some(60));
        assert_eq!(value.cache_read_tokens, Some(20));
        assert_eq!(value.cache_write_tokens, Some(30));
    }

    #[test]
    fn malformed_counts_preserve_known_fields_but_invalidate_complete_observations() {
        let value = observe(&json!({"usage":{"input_tokens":10,"output_tokens":2,
            "cache_read_input_tokens":"bad","cache_creation_input_tokens":0}}));
        assert!(value.incomplete);
        assert_eq!(value.output_tokens, Some(2));
        let value = observe(&json!({"usage":{"prompt_tokens":10,"completion_tokens":2,
            "prompt_tokens_details":{"cached_tokens":-1}}}));
        assert!(value.incomplete);
    }

    #[test]
    fn responses_extensions_and_reported_zero_usage_survive() {
        let value = observe(
            &json!({"response":{"usage":{"input_tokens":0,"output_tokens":0,
            "input_tokens_details":{"cache_write_tokens":0}}}}),
        );
        assert_eq!(value.input_tokens, Some(0));
        assert_eq!(value.cache_write_tokens, Some(0));
    }

    #[test]
    fn raw_money_preserves_numeric_and_string_precision_in_json_and_sse() {
        for (cost, expected) in [
            ("0.999999499999999999", 999_999),
            (r#""0.999999499999999999""#, 999_999),
            ("9.99999499999999999e-1", 999_999),
            (r#""9.99999499999999999e-1""#, 999_999),
            ("9007199254.740991", 9_007_199_254_740_991),
            ("9223372036854.775807", i64::MAX),
            ("0", 0),
            (r#""0""#, 0),
            ("0.0000005", 1),
            (r#""0.000000\u0035""#, 1),
        ] {
            for wrapper in ["root", "message", "response"] {
                for streaming in [false, true] {
                    let record =
                        raw_money_receipt(cost, wrapper, streaming, "https://openrouter.ai/api/v1");
                    assert_eq!(
                        record.cost_micros,
                        Some(expected),
                        "{cost}, {wrapper}, stream={streaming}"
                    );
                    assert_eq!(record.cost_kind, CostKind::Actual);
                    assert_eq!(record.price_version, None);
                }
            }
        }
    }

    #[test]
    fn raw_money_rejects_invalid_and_overflowing_amounts() {
        for cost in [
            "-1",
            r#""-1""#,
            "true",
            "null",
            "{}",
            "[]",
            r#""NaN""#,
            r#""Infinity""#,
            r#""bad""#,
            "1e100",
            r#""1e100""#,
            "9223372036854.775808",
            r#""9223372036854.775808""#,
        ] {
            let record = raw_money_receipt(cost, "root", false, "https://openrouter.ai/api/v1");
            assert_eq!(record.cost_micros, None, "{cost}");
            assert_eq!(record.cost_kind, CostKind::Unknown);
        }
    }

    #[test]
    fn raw_money_requires_the_official_https_endpoint() {
        for endpoint in [
            "https://other.test/v1",
            "http://openrouter.ai/api/v1",
            "https://openrouter.ai.evil.test/api/v1",
            "https://openrouter.ai:444/api/v1",
        ] {
            for cost in ["0.999999499999999999", r#""0.999999499999999999""#] {
                let record = raw_money_receipt(cost, "response", true, endpoint);
                assert_eq!(record.cost_micros, None);
                assert_eq!(record.cost_kind, CostKind::Unknown);
                assert!(record.usage_observation.is_some());
            }
        }
    }

    fn raw_money_receipt(
        cost: &str,
        wrapper: &str,
        streaming: bool,
        endpoint: &str,
    ) -> RequestRecord {
        let usage =
            format!(r#"{{"usage":{{"prompt_tokens":1,"completion_tokens":1,"cost":{cost}}}}}"#);
        let body = match wrapper {
            "message" => format!(r#"{{"message":{usage}}}"#),
            "response" => format!(r#"{{"response":{usage}}}"#),
            _ => usage,
        };
        let mut tap = AccountingTap::new(endpoint);
        tap.head(
            200,
            if streaming {
                "text/event-stream"
            } else {
                "application/json"
            },
        );
        let wire = if streaming {
            format!("data: {body}\n\n")
        } else {
            body
        };
        for chunk in wire.as_bytes().chunks(7) {
            tap.push(chunk);
        }
        let mut record = RequestRecord::begin(0, "openai");
        tap.finish(&mut record);
        record
    }

    #[test]
    fn monetary_values_are_checked_and_rounded_without_float_casts() {
        let parse = |text| usd_micros(serde_json::from_str::<&RawValue>(text).unwrap());
        assert_eq!(parse(r#""0.151120""#), Some(151_120));
        assert_eq!(parse("0"), Some(0));
        assert_eq!(parse(r#""0.0000006""#), Some(1));
        assert_eq!(parse(r#""1e-6""#), Some(1));
        for value in ["-1", r#""NaN""#, r#""1e100""#, "true"] {
            assert_eq!(parse(value), None);
        }
    }

    #[test]
    fn split_sse_snapshots_preserve_presence_without_counting_duplicates_twice() {
        let mut tap = AccountingTap::new("https://api.anthropic.com/v1");
        tap.head(200, "text/event-stream; charset=utf-8");
        let frames = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":0,\"cache_read_input_tokens\":20,\"cache_creation_input_tokens\":30}}}\n\n",
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":7}}\n\n",
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":7}}\n\n"
        );
        for byte in frames.bytes() {
            tap.push(&[byte]);
        }
        let mut record = RequestRecord::begin(0, "anthropic");
        tap.finish(&mut record);
        let usage = record.usage.unwrap();
        assert_eq!(
            (
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_write_tokens
            ),
            (60, 7, 30)
        );
    }

    #[test]
    fn sparse_anthropic_updates_reconcile_input_with_previous_cache_snapshot() {
        let mut tap = AccountingTap::new("https://api.anthropic.com/v1");
        tap.head(200, "text/event-stream");
        tap.push(b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":0,\"cache_read_input_tokens\":20,\"cache_creation_input_tokens\":30}}}\n\n");
        tap.push(b"data: {\"type\":\"message_delta\",\"usage\":{\"input_tokens\":40,\"output_tokens\":7}}\n\n");
        let mut record = RequestRecord::begin(0, "anthropic");
        tap.finish(&mut record);
        assert_eq!(record.usage.unwrap().input_tokens, 90);
        assert!(can_estimate(
            &record.usage.unwrap(),
            record.usage_observation
        ));
    }

    #[test]
    fn only_the_official_usd_contract_accepts_account_charges() {
        for (endpoint, accepted) in [
            ("https://openrouter.ai/api/v1", true),
            ("https://openrouter.ai.evil.test/api/v1", false),
            ("http://openrouter.ai/api/v1", false),
            ("https://proxy.test/openrouter.ai", false),
        ] {
            let mut tap = AccountingTap::new(endpoint);
            tap.head(200, "application/json");
            tap.push(br#"{"usage":{"prompt_tokens":1,"completion_tokens":1,"cost":0.000001,"cost_details":{"upstream_inference_cost":99}}}"#);
            let mut record = RequestRecord::begin(0, "openai");
            tap.finish(&mut record);
            assert_eq!(record.cost_micros, accepted.then_some(1));
            assert_eq!(record.cost_kind == CostKind::Actual, accepted);
        }
    }

    #[test]
    fn observations_do_not_require_plaintext_logging_and_reset_between_attempts() {
        let ctx = crate::request_context::RequestContext::detached(
            std::time::Duration::from_secs(10),
            std::time::Duration::from_secs(5),
        );
        ctx.begin_accounting("https://openrouter.ai/api/v1");
        ctx.capture_upstream_response_head(200, &std::collections::BTreeMap::new());
        ctx.append_upstream_response_body(
            br#"{"usage":{"prompt_tokens":10,"completion_tokens":2,"cost":1}}"#,
        );
        // A new attempt discards the first attempt's uncommitted evidence.
        ctx.begin_accounting("https://other.test/v1");
        ctx.capture_upstream_response_head(200, &std::collections::BTreeMap::new());
        ctx.append_upstream_response_body(br#"{"usage":{"prompt_tokens":3,"completion_tokens":0,"prompt_tokens_details":{"cache_write_tokens":0}}}"#);
        let mut record = RequestRecord::begin(0, "openai");
        ctx.finish_accounting(&mut record);
        assert_eq!(record.usage.unwrap().input_tokens, 3);
        assert_eq!(record.cost_micros, None);
        assert_eq!(
            record.usage_observation.unwrap().cache_write_tokens,
            Some(0)
        );
    }

    #[test]
    fn error_bodies_and_empty_adapter_defaults_do_not_become_free_usage() {
        let mut tap = AccountingTap::new("https://openrouter.ai/api/v1");
        tap.head(429, "application/json");
        tap.push(br#"{"usage":{"prompt_tokens":100,"completion_tokens":10,"cost":99}}"#);
        let mut record = RequestRecord::begin(0, "openai");
        record.usage = Some(Usage::default());
        tap.finish(&mut record);
        assert_eq!(record.usage, None);
        assert_eq!(record.cost_micros, None);
    }

    #[test]
    fn oversized_stream_frames_are_bounded_and_do_not_create_cost() {
        let mut tap = AccountingTap::new("https://openrouter.ai/api/v1");
        tap.head(200, "text/event-stream");
        for _ in 0..129 {
            tap.push(&[b'x'; 8192]);
        }
        let mut record = RequestRecord::begin(0, "openai");
        tap.finish(&mut record);
        assert!(record.usage.is_none());
        assert!(record.cost_micros.is_none());
    }

    #[test]
    fn abandoning_later_frames_preserves_an_already_verified_account_charge() {
        let mut tap = AccountingTap::new("https://openrouter.ai/api/v1");
        tap.head(200, "text/event-stream");
        tap.push(
            b"data: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2,\"cost\":0.01}}\n\n",
        );
        for _ in 0..129 {
            tap.push(&[b'x'; 8192]);
        }
        let mut record = RequestRecord::begin(0, "openai");
        tap.finish(&mut record);
        assert_eq!(record.cost_kind, CostKind::Actual);
        assert_eq!(record.cost_micros, Some(10_000));
        assert!(record.usage_observation.unwrap().incomplete);
    }

    #[test]
    fn abandoned_observations_do_not_replace_final_adapter_usage_with_old_snapshots() {
        let mut tap = AccountingTap::new("https://api.anthropic.com/v1");
        tap.head(200, "text/event-stream");
        tap.push(b"data: {\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\n");
        for _ in 0..129 {
            tap.push(&[b'x'; 8192]);
        }
        let mut record = RequestRecord::begin(0, "anthropic");
        record.usage = Some(Usage {
            input_tokens: 10,
            output_tokens: 7,
            ..Usage::default()
        });
        tap.finish(&mut record);
        assert_eq!(record.usage.unwrap().output_tokens, 7);
        assert!(record.usage_observation.unwrap().incomplete);
        assert!(record.cost_micros.is_none());
    }
}

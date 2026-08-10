use serde::{Deserialize, Serialize};

/// serde helper: additive tier fields stay off the wire at zero so the
/// serialized shape of tier-less providers is byte-identical to v0.2.0.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Token accounting for one exchange, in the host's canonical vocabulary.
///
/// Providers report usage under many names; a `provider-adapter` maps them onto
/// these fields so cost estimation and the local metrics store see one shape.
/// Fields default to zero because most providers report only a subset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    /// Prompt tokens served from a provider-side cache, usually billed cheaper.
    #[serde(default)]
    pub cache_read_tokens: u64,
    /// Prompt tokens written into a provider-side cache, sometimes billed extra.
    ///
    /// When the provider splits cache writes into TTL tiers (Anthropic's
    /// `cache_creation.ephemeral_5m_input_tokens` / `ephemeral_1h_input_tokens`,
    /// billed at different multipliers), this stays the **total** and the two
    /// tier fields below carry the detail — mirroring how Anthropic reports
    /// `cache_creation_input_tokens` alongside its sub-buckets. Providers
    /// without a tier split leave the tier fields at zero.
    #[serde(default)]
    pub cache_write_tokens: u64,
    /// Subset of [`Self::cache_write_tokens`] written into a 5-minute-TTL
    /// cache tier (Anthropic bills this at 1.25× input). Zero when the
    /// provider reports no tier split. Zero is not serialized, so the wire
    /// shape of providers without a tier split is unchanged.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_write_5m_tokens: u64,
    /// Subset of [`Self::cache_write_tokens`] written into a 1-hour-TTL
    /// cache tier (Anthropic bills this at 2× input). Zero when the
    /// provider reports no tier split. Zero is not serialized (same as the
    /// 5m tier field).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_write_1h_tokens: u64,
    /// Tokens spent on hidden reasoning, billed as output by most providers.
    #[serde(default)]
    pub reasoning_tokens: u64,
}

impl Usage {
    /// Total tokens that crossed the wire in both directions.
    ///
    /// Cache and reasoning counts are not added here: `reasoning_tokens` is
    /// already a subset of `output_tokens` for every provider modelled so far,
    /// and cache counts partition `input_tokens` rather than extend it. Pricing
    /// needs the individual fields, not this sum.
    #[must_use]
    pub const fn total(self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Fold a later usage report into this one, field-wise, with
    /// last-nonzero-wins semantics.
    ///
    /// This is the contract for streams that report usage **more than once**:
    /// Anthropic's `message_start` carries the input-side buckets up front and
    /// `message_delta` carries the final output count at the end, so a stream
    /// may legitimately emit [`crate::StreamEvent::Usage`] twice. Consumers
    /// accumulate with this method; a zero field in a later report never
    /// erases an earlier nonzero value.
    pub fn absorb(&mut self, later: Usage) {
        fn keep(slot: &mut u64, later: u64) {
            if later != 0 {
                *slot = later;
            }
        }
        keep(&mut self.input_tokens, later.input_tokens);
        keep(&mut self.output_tokens, later.output_tokens);
        keep(&mut self.cache_read_tokens, later.cache_read_tokens);
        keep(&mut self.cache_write_tokens, later.cache_write_tokens);
        keep(&mut self.cache_write_5m_tokens, later.cache_write_5m_tokens);
        keep(&mut self.cache_write_1h_tokens, later.cache_write_1h_tokens);
        keep(&mut self.reasoning_tokens, later.reasoning_tokens);
    }
}

#[cfg(test)]
mod tests {
    use super::Usage;

    #[test]
    fn total_counts_only_wire_tokens() {
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 90,
            cache_write_tokens: 10,
            reasoning_tokens: 15,
            ..Usage::default()
        };

        assert_eq!(usage.total(), 120);
    }

    #[test]
    fn absent_fields_default_to_zero() {
        let usage: Usage = serde_json::from_str(r#"{"input_tokens":7}"#).expect("valid usage");

        assert_eq!(usage.input_tokens, 7);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.reasoning_tokens, 0);
    }

    #[test]
    fn cache_write_tiers_default_to_zero_and_deserialize_when_present() {
        let bare: Usage = serde_json::from_str(r#"{"cache_write_tokens":30}"#).expect("valid");
        assert_eq!(bare.cache_write_tokens, 30);
        assert_eq!(bare.cache_write_5m_tokens, 0);
        assert_eq!(bare.cache_write_1h_tokens, 0);

        let tiered: Usage = serde_json::from_str(
            r#"{"cache_write_tokens":30,"cache_write_5m_tokens":10,"cache_write_1h_tokens":20}"#,
        )
        .expect("valid");
        assert_eq!(tiered.cache_write_5m_tokens, 10);
        assert_eq!(tiered.cache_write_1h_tokens, 20);
    }

    #[test]
    fn absorb_is_last_nonzero_wins_per_field() {
        // Anthropic streaming reports usage twice: message_start carries input
        // buckets, and the final message_delta carries output counts. Absorbing
        // both must produce one complete usage record.
        let mut acc = Usage::default();
        acc.absorb(Usage {
            input_tokens: 1000,
            cache_write_tokens: 300,
            cache_write_5m_tokens: 100,
            cache_write_1h_tokens: 200,
            cache_read_tokens: 500,
            output_tokens: 1,
            ..Usage::default()
        });
        acc.absorb(Usage {
            output_tokens: 400,
            ..Usage::default()
        });

        assert_eq!(acc.input_tokens, 1000);
        assert_eq!(acc.cache_write_5m_tokens, 100);
        assert_eq!(acc.cache_write_1h_tokens, 200);
        assert_eq!(acc.cache_read_tokens, 500);
        assert_eq!(acc.output_tokens, 400, "后报非零覆盖前报占位值");
        assert_eq!(acc.cache_write_tokens, 300, "后报零不得抹掉前报非零");
    }
}

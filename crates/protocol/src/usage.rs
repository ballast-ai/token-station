use serde::{Deserialize, Serialize};

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
    #[serde(default)]
    pub cache_write_tokens: u64,
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
}

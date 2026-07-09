#![doc = "Local metrics primitives for usage and routing observability."]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl TokenUsage {
    #[must_use]
    pub fn total(self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

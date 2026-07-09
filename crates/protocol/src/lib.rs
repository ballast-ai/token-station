#![doc = "Canonical protocol types shared by token-station components."]

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentHint {
    kind: String,
}

impl AgentHint {
    #[must_use]
    pub fn new(kind: impl Into<String>) -> Self {
        Self { kind: kind.into() }
    }

    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatRequest {
    model: String,
}

impl ChatRequest {
    #[must_use]
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
        }
    }

    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentHint, ChatRequest};

    #[test]
    fn keeps_basic_protocol_fields() {
        assert_eq!(AgentHint::new("code-review").kind(), "code-review");
        assert_eq!(ChatRequest::new("auto").model(), "auto");
    }
}

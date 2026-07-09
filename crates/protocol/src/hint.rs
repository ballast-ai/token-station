use serde::{Deserialize, Serialize};

use crate::Extensions;

/// What an [`AgentHint`] describes.
///
/// Closed on purpose. A hint kind the host does not know is a version mismatch,
/// not something to guess at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HintKind {
    /// Where the caller is in its own workflow, e.g. `planning`, `edit`,
    /// `summarize`. The strongest routing signal an agent can give, because the
    /// agent knows its step type and the host cannot infer it.
    StepType,
    /// What the work is about, e.g. `code-review`, `translation`.
    TaskType,
    /// A soft preference, e.g. `latency`, `cost`, `quality`.
    Preference,
    /// A capability the caller needs, e.g. `vision`, `json_schema`.
    Capability,
}

/// A routing feature supplied by the calling agent, IDE or CLI.
///
/// A hint is *input to* routing, never a routing decision. Nothing here selects
/// a provider, a credential or a budget: those belong to the host, and an
/// adapter that wants to influence them has to go through the router's rules
/// like any other caller. `value` is free-form because the vocabulary of step
/// and task names belongs to the agent ecosystem, not to this ABI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentHint {
    pub kind: HintKind,
    pub value: String,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}

impl AgentHint {
    #[must_use]
    pub fn new(kind: HintKind, value: impl Into<String>) -> Self {
        Self {
            kind,
            value: value.into(),
            extensions: Extensions::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentHint, HintKind};

    #[test]
    fn serializes_kind_in_snake_case() {
        let hint = AgentHint::new(HintKind::StepType, "planning");
        let json = serde_json::to_string(&hint).expect("serializable hint");

        assert_eq!(json, r#"{"kind":"step_type","value":"planning"}"#);
    }

    #[test]
    fn rejects_unknown_kind_as_version_mismatch() {
        let parsed: Result<AgentHint, _> =
            serde_json::from_str(r#"{"kind":"budget","value":"cheap"}"#);

        assert!(parsed.is_err());
    }

    #[test]
    fn captures_unknown_fields_as_extensions() {
        let hint: AgentHint =
            serde_json::from_str(r#"{"kind":"preference","value":"latency","weight":0.7}"#)
                .expect("valid hint");

        assert_eq!(hint.extensions["weight"], serde_json::json!(0.7));
    }
}

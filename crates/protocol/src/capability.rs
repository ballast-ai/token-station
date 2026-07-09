use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::Extensions;

/// What one model can do, as reported by its `provider-adapter`.
///
/// The router uses this to drop candidates that cannot serve a request: a
/// request carrying tools must not be routed to a model without `tool`. Absent
/// capabilities are treated as unsupported, so a conservative adapter degrades
/// availability rather than correctness.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelCapability {
    pub model: String,
    #[serde(default)]
    pub tool: bool,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub json_schema: bool,
    /// Maximum combined input and output tokens. Zero means unknown, which the
    /// router treats as "do not route long-context requests here".
    #[serde(default)]
    pub context_window: u32,
    /// Sampling parameter names this model honours, e.g. `temperature`.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub supported_parameters: BTreeSet<String>,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}

#[cfg(test)]
mod tests {
    use super::ModelCapability;

    #[test]
    fn absent_capabilities_are_unsupported() {
        let capability: ModelCapability =
            serde_json::from_str(r#"{"model":"local-llama"}"#).expect("valid capability");

        assert!(!capability.tool);
        assert!(!capability.vision);
        assert!(!capability.json_schema);
        assert_eq!(capability.context_window, 0);
    }

    #[test]
    fn supported_parameters_serialize_in_a_stable_order() {
        let capability = ModelCapability {
            model: "gpt-5.5".to_owned(),
            supported_parameters: ["top_p", "temperature", "stop"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            ..ModelCapability::default()
        };
        let json = serde_json::to_value(&capability).expect("serializable capability");

        assert_eq!(
            json["supported_parameters"],
            serde_json::json!(["stop", "temperature", "top_p"])
        );
    }
}

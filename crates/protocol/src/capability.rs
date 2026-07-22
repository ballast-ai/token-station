use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::Extensions;

/// Evidence strength for one model capability.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Verified,
    Declared,
    Unsupported,
    #[default]
    Unknown,
}

impl CapabilityState {
    /// Only positive evidence routes traffic. Unknown fails closed.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Verified | Self::Declared)
    }
}

/// What one model can do, as reported by its `provider-adapter`.
///
/// The router uses this to drop candidates that cannot serve a request: a
/// request carrying tools must not be routed without positive evidence. Absent
/// capabilities fail closed. The legacy bools remain in the v1 ABI; optional
/// evidence fields add the four-state truth without breaking older plugins.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelCapability {
    pub model: String,
    #[serde(default)]
    pub tool: bool,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub json_schema: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_state: Option<CapabilityState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision_state: Option<CapabilityState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_schema_state: Option<CapabilityState>,
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

impl ModelCapability {
    #[must_use]
    pub fn tool_state(&self) -> CapabilityState {
        self.tool_state.unwrap_or(if self.tool {
            CapabilityState::Declared
        } else {
            CapabilityState::Unsupported
        })
    }

    #[must_use]
    pub fn vision_state(&self) -> CapabilityState {
        self.vision_state.unwrap_or(if self.vision {
            CapabilityState::Declared
        } else {
            CapabilityState::Unsupported
        })
    }

    #[must_use]
    pub fn json_schema_state(&self) -> CapabilityState {
        self.json_schema_state.unwrap_or(if self.json_schema {
            CapabilityState::Declared
        } else {
            CapabilityState::Unsupported
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{CapabilityState, ModelCapability};

    #[test]
    fn absent_legacy_capabilities_are_unsupported_and_fail_closed() {
        let capability: ModelCapability =
            serde_json::from_str(r#"{"model":"local-llama"}"#).expect("valid capability");

        assert_eq!(capability.tool_state(), CapabilityState::Unsupported);
        assert_eq!(capability.vision_state(), CapabilityState::Unsupported);
        assert_eq!(capability.json_schema_state(), CapabilityState::Unsupported);
        assert!(!capability.tool_state().is_supported());
        assert_eq!(capability.context_window, 0);
    }

    #[test]
    fn legacy_bools_and_explicit_unknown_coexist() {
        let capability: ModelCapability = serde_json::from_str(
            r#"{"model":"legacy","tool":true,"vision":false,"json_schema_state":"unknown"}"#,
        )
        .expect("legacy capability parses");
        assert_eq!(capability.tool_state(), CapabilityState::Declared);
        assert_eq!(capability.vision_state(), CapabilityState::Unsupported);
        assert_eq!(capability.json_schema_state(), CapabilityState::Unknown);
        assert_eq!(
            serde_json::to_value(&capability).unwrap()["tool"],
            serde_json::json!(true)
        );
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

//! ChatGPT plan Responses translation on the South-owned v2 ABI.
//!
//! This component has no network or credential access. The Host supplies a
//! registration-bound credential through the declared slot. Sampling limits
//! that the plan endpoint does not accept are deliberately not sent.
//!
//! The Responses codec itself lives in the shared `plan-responses` crate. This
//! crate adds the codex-plan dialect and the JSON response body.

use plan_responses::{OpaqueReasoning, Profile, ResponseMetadata, ResponsesCodec};
use serde_json::{json, Value};
use south_component_conformance::{ComponentResultV1, ProviderComponentV1, StreamParserV1};
use south_provider_api::{ComponentMetadataV1, PROVIDER_WORLD};
use std::collections::BTreeMap;
use token_station_protocol::{
    ChatRequest, ChatResponse, ErrorCode, ErrorEnvelope, HttpRequestDescriptor, HttpResponseParts,
    ModelCapability, ProviderConfig, SafeHeaders,
};

pub use plan_responses::TOOL_NAMESPACE;

mod codex;
#[cfg(target_arch = "wasm32")]
mod shell;

pub struct ChatGptPlan;

/// The ChatGPT plan reads opaque reasoning and `response.metadata` past.
const CODEC: ResponsesCodec = ResponsesCodec {
    profile: Profile {
        label: "The ChatGPT plan",
        opaque_reasoning: OpaqueReasoning::Ignore,
        response_metadata: ResponseMetadata::Ignore,
    },
};

fn capability(message: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::Capability, 400, message)
}

fn protocol(message: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::ProviderProtocolError, 502, message)
}

impl ProviderComponentV1 for ChatGptPlan {
    fn metadata(&self) -> ComponentMetadataV1 {
        ComponentMetadataV1 {
            name: "provider-chatgpt-plan".into(),
            version: "2.2.0".into(),
            api_version: PROVIDER_WORLD.into(),
        }
    }
    fn model_capabilities(
        &self,
        config: &ProviderConfig,
    ) -> ComponentResultV1<Vec<ModelCapability>> {
        Ok(config.models.clone())
    }
    fn build_http_request(
        &self,
        r: &ChatRequest,
        config: &ProviderConfig,
    ) -> ComponentResultV1<HttpRequestDescriptor> {
        if config.provider != "codex-plan" {
            return CODEC.build_http_request(r, config);
        }
        codex::check_config(config)?;
        let mut descriptor = CODEC.request(r, config, codex::headers(config)?)?;
        descriptor.body = descriptor.body.take().map(codex::body).transpose()?;
        Ok(descriptor)
    }
    fn parse_response(&self, parts: &HttpResponseParts) -> ComponentResultV1<ChatResponse> {
        if !(200..300).contains(&parts.status) {
            return Err(self.map_provider_error(parts)?);
        }
        // A JSON response object is the same terminal state as
        // `response.completed`, so it goes through that event's checks too.
        match serde_json::from_str::<Value>(&parts.body) {
            Ok(response) if response.is_object() => CODEC.parse_response(&HttpResponseParts {
                body: format!(
                    "data: {}\n\n",
                    json!({"type":"response.completed","response":response})
                ),
                ..parts.clone()
            }),
            _ => CODEC.parse_response(parts),
        }
    }
    fn map_provider_error(&self, parts: &HttpResponseParts) -> ComponentResultV1<ErrorEnvelope> {
        CODEC.map_provider_error(parts)
    }
    fn stream_parser(&self) -> Box<dyn StreamParserV1> {
        CODEC.stream_parser()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn vendored_wit_matches_the_pinned_south_contract() {
        assert_eq!(
            include_str!("../wit/provider-adapter.wit"),
            south_provider_api::ADAPTER_WIT
        );
    }
}

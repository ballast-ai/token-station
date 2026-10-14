//! Claude Code subscription codec. The host owns process I/O and local login references.
//! This component cannot access the network, the filesystem, or login credentials.

#[cfg(any(target_arch = "wasm32", test))]
fn build_http_request(chat_request: &str, provider_config: &str) -> Result<String, String> {
    use serde_json::{json, Value};
    let capability = |message: &str| {
        json!({"code":"capability","http_status":400,"message":message}).to_string()
    };
    let request: Value = serde_json::from_str(chat_request)
        .map_err(|_| capability("Invalid Claude subscription request."))?;
    let mut config: Value = serde_json::from_str(provider_config)
        .map_err(|_| capability("Invalid Claude subscription configuration."))?;
    if config["provider"] != "claude-plan" || config["base_url"] != "http://127.0.0.1:9/claude-plan"
    {
        return Err(capability(
            "Claude subscriptions require the fixed host process endpoint.",
        ));
    }
    if config["auth"] != "local_subscription_token" {
        return Err(json!({"code":"auth","http_status":401,"message":"Claude subscriptions require a registered local login reference."}).to_string());
    }
    if matches!(
        request
            .pointer("/response_format/type")
            .and_then(Value::as_str),
        Some("json_object" | "json_schema")
    ) {
        return Err(capability(
            "Claude subscriptions cannot preserve this structured response format.",
        ));
    }
    // Reject content before the reference flattens system and tool-result blocks.
    if let Some(messages) = request["messages"].as_array() {
        for message in messages {
            if let Some(parts) = message["content"].as_array() {
                if parts
                    .iter()
                    .any(|part| part["type"] != "text" || !part["text"].is_string())
                {
                    return Err(capability("Claude subscriptions cannot preserve images, files, or opaque conversation state."));
                }
            }
        }
    }
    for field in [
        "thinking",
        "anthropic_thinking",
        "reasoning_effort",
        "context_management",
        "previous_response_id",
        "conversation",
        "additional_tools",
        "native_tools",
        "hosted_tools",
        "web_search",
        "anthropic_server_tools",
    ] {
        if request
            .get(field)
            .is_some_and(|value| !value.is_null() && value != &json!(false) && value != &json!([]))
        {
            return Err(capability(
                "Claude subscriptions cannot preserve the requested provider state or hosted tool.",
            ));
        }
    }
    // The Anthropic reference supplies the wire format. Host-only authentication
    // replaces the reference secret slot before the descriptor leaves this component.
    config["provider"] = json!("anthropic");
    config["base_url"] = json!("http://127.0.0.1:9/claude-plan/v1");
    config["auth"] = json!("provider_api_key");
    let built = south_component_conformance::abi::build_http_request_json(
        &south_component_conformance::reference_anthropic::AnthropicReferenceV1,
        chat_request,
        &config.to_string(),
    )?;
    let mut descriptor: Value = serde_json::from_str(&built)
        .map_err(|_| capability("Invalid Claude host transport descriptor."))?;
    if descriptor["url"] != "http://127.0.0.1:9/claude-plan/v1/messages" {
        return Err(capability(
            "Claude subscriptions cannot use an external transport endpoint.",
        ));
    }
    preserve_tool_error_flags(&request, &mut descriptor);
    descriptor["auth"] = json!({"scheme":"bearer","secret":"local_subscription_token"});
    Ok(descriptor.to_string())
}

#[cfg(any(target_arch = "wasm32", test))]
fn preserve_tool_error_flags(request: &serde_json::Value, descriptor: &mut serde_json::Value) {
    use serde_json::json;
    // The reference drops the Agent's tool-error flag. Keep it on its result.
    if let Some(messages) = request["messages"].as_array() {
        for message in messages {
            let Some(is_error) = message["anthropic_tool_result_is_error"].as_bool() else {
                continue;
            };
            if let Some(turns) = descriptor["body"]["messages"].as_array_mut() {
                for turn in turns {
                    if let Some(parts) = turn["content"].as_array_mut() {
                        for part in parts {
                            if part["type"] == "tool_result"
                                && part["tool_use_id"] == message["tool_call_id"]
                            {
                                part["is_error"] = json!(is_error);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod shell {
    use std::sync::Mutex;

    wit_bindgen::generate!({
        path: "wit",
        world: "provider-adapter-v2",
    });

    use exports::token_station::adapter::provider_adapter::{
        AdapterHealth, AdapterMetadata, Guest,
    };
    use south_component_conformance::abi;
    use south_component_conformance::reference_anthropic::AnthropicReferenceV1;
    use token_station::adapter::common::HealthStatus;

    /// The stream this instance is holding. Instance state on purpose: the host
    /// instantiates one component per stream.
    static STREAM: Mutex<Option<abi::StreamAbiV1>> = Mutex::new(None);

    struct Anthropic;

    impl Guest for Anthropic {
        fn metadata() -> AdapterMetadata {
            AdapterMetadata {
                name: "provider-claude-plan".into(),
                version: "1.0.0".into(),
                api_version: "provider-adapter-v2".into(),
            }
        }

        fn healthcheck() -> AdapterHealth {
            AdapterHealth {
                status: HealthStatus::Ready,
                detail: None,
            }
        }

        fn model_capabilities(provider_config: String) -> Result<String, String> {
            abi::model_capabilities_json(&AnthropicReferenceV1, &provider_config)
        }

        fn build_http_request(
            chat_request: String,
            provider_config: String,
        ) -> Result<String, String> {
            super::build_http_request(&chat_request, &provider_config)
        }

        fn parse_response(response_parts: String) -> Result<String, String> {
            abi::parse_response_json(&AnthropicReferenceV1, &response_parts)
        }

        fn parse_stream_chunk(chunk: Vec<u8>) -> Result<String, String> {
            let mut stream = STREAM.lock().expect("single-threaded guest");
            stream
                .get_or_insert_with(|| abi::StreamAbiV1::new(&AnthropicReferenceV1))
                .parse_chunk_json(&chunk)
        }

        fn map_provider_error(response_parts: String) -> Result<String, String> {
            abi::map_provider_error_json(&AnthropicReferenceV1, &response_parts)
        }
    }

    export!(Anthropic);
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};
    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../fixtures/provider.request.local.input.json"
        ))
        .unwrap()
    }
    #[test]
    fn host_process_descriptor_uses_only_local_reference_auth() {
        let f = fixture();
        let output = super::build_http_request(
            &f["chat_request"].to_string(),
            &f["provider_config"].to_string(),
        )
        .unwrap();
        let output: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["url"], "http://127.0.0.1:9/claude-plan/v1/messages");
        assert_eq!(
            output["auth"],
            json!({"scheme":"bearer","secret":"local_subscription_token"})
        );
        assert_eq!(output["body"]["messages"][0]["content"], "Hello");
        let expected: Value = serde_json::from_str(include_str!(
            "../fixtures/provider.request.local.expected.json"
        ))
        .unwrap();
        assert_eq!(output, expected);
    }
    #[test]
    fn substituted_network_and_credentials_are_rejected() {
        let f = fixture();
        for change in [
            json!({"base_url":"https://api.anthropic.com"}),
            json!({"auth":"provider_api_key"}),
            json!({"provider":"anthropic"}),
        ] {
            let mut config = f["provider_config"].clone();
            for (key, value) in change.as_object().unwrap() {
                config[key] = value.clone();
            }
            assert!(
                super::build_http_request(&f["chat_request"].to_string(), &config.to_string())
                    .is_err()
            );
        }
    }
    #[test]
    fn response_format_has_an_explicit_capability_error() {
        let f = fixture();
        let mut request = f["chat_request"].clone();
        request["response_format"] =
            json!({"type":"json_schema","json_schema":{"name":"test","schema":{"type":"object"}}});
        let error =
            super::build_http_request(&request.to_string(), &f["provider_config"].to_string())
                .unwrap_err();
        assert_eq!(
            serde_json::from_str::<Value>(&error).unwrap()["code"],
            "capability"
        );
    }
    #[test]
    fn tool_result_and_system_opaque_parts_fail_before_flattening() {
        let f = fixture();
        for role in ["system", "tool"] {
            let mut request = f["chat_request"].clone();
            request["messages"] = json!([{"role":role,"tool_call_id":"call-test","content":[{"type":"redacted_thinking","data":"opaque"}]}]);
            let error =
                super::build_http_request(&request.to_string(), &f["provider_config"].to_string())
                    .unwrap_err();
            assert_eq!(
                serde_json::from_str::<Value>(&error).unwrap()["code"],
                "capability"
            );
        }
    }
    #[test]
    fn caller_tool_error_flags_survive_the_messages_codec() {
        let f = fixture();
        let mut request = f["chat_request"].clone();
        request["messages"] = json!([
            {"role":"assistant","tool_calls":[{"id":"call-test","name":"lookup","arguments":"{}"}]},
            {"role":"tool","tool_call_id":"call-test","content":"Caller error","anthropic_tool_result_is_error":true}
        ]);
        let output =
            super::build_http_request(&request.to_string(), &f["provider_config"].to_string())
                .unwrap();
        let output: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(
            output["body"]["messages"][1]["content"][0]["is_error"],
            true
        );
        for state in ["anthropic_thinking", "reasoning_effort"] {
            request[state] = json!("opaque");
            assert!(super::build_http_request(
                &request.to_string(),
                &f["provider_config"].to_string()
            )
            .is_err());
            request.as_object_mut().unwrap().remove(state);
        }
    }
    #[test]
    fn south_abi_matches_the_pinned_contract() {
        assert_eq!(
            include_str!("../wit/provider-adapter.wit"),
            south_provider_api::ADAPTER_WIT
        );
    }
}

#[cfg(test)]
mod native_conformance;

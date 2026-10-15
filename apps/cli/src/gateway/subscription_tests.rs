use super::{assemble_upstreams, south_component_support};
use crate::config::ClientConfig;
use crate::local_subscriptions::Kind;
use crate::plugins::{PluginRegistry, Receipts};
use crate::{
    config::EgressMode,
    south_provider_call::{
        CommunityCallPolicyV1, IneligibleV1, PrepareProviderCallErrorV1, RequestBodyModeV1,
        prepare_provider_call_v1, prepare_provider_stream_v1,
    },
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use token_station_protocol::ChatRequest;

#[test]
fn real_gateway_assembly_projects_all_local_subscription_metadata_into_wasm() {
    for (kind, endpoint, metadata, suffix) in subscription_cases() {
        let mut value: Value = serde_json::from_str(crate::EXAMPLE_CONFIG).unwrap();
        value["upstreams"]["openai_personal"] = json!({
            "provider":kind.provider(), "base_url":endpoint,
            "auth":{"slot":"local_subscription_token","local_subscription":{"kind":kind,"id":"fixture-reference"}},
            "subscription_identity":metadata, "models":[{"model":"fixture-model","tool":true}]
        });
        value["plugins"]["providers"][kind.provider()] = json!(kind.package());
        let mut config: ClientConfig = serde_json::from_value(value).unwrap();
        config.plugins.dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins-dist");
        let registry = PluginRegistry::discover(
            &config.plugins,
            &Receipts::load(
                &std::env::temp_dir()
                    .join(format!("ts-subscription-assembly-{}", std::process::id())),
            )
            .unwrap(),
        )
        .unwrap();
        let support = south_component_support(&config, &registry).unwrap();
        let (upstreams, catalog) = assemble_upstreams(&config, &support).unwrap();
        let upstream = &upstreams["openai_personal"];
        let request: ChatRequest = serde_json::from_value(json!({"model":"fixture-model","messages":[{"role":"user","content":"fixture request"}]})).unwrap();
        let descriptor = upstream
            .plugin
            .build_http_request(&request, &upstream.config)
            .unwrap_or_else(|error| panic!("{}: {}", kind.provider(), error.message));
        assert!(
            descriptor.url.ends_with(suffix),
            "{}: {}",
            kind.provider(),
            descriptor.url
        );
        upstream.config.authorize(&descriptor).unwrap();
        if matches!(kind, Kind::GeminiCli | Kind::Antigravity) {
            for stream in [false, true] {
                let mut request = request.clone();
                request.stream = stream;
                let descriptor = upstream
                    .plugin
                    .build_http_request(&request, &upstream.config)
                    .unwrap();
                let policy = CommunityCallPolicyV1::new(
                    EgressMode::Direct,
                    if stream {
                        RequestBodyModeV1::Streaming
                    } else {
                        RequestBodyModeV1::Buffered
                    },
                    BTreeSet::from(["bearer".to_owned()]),
                );
                let auth = config.upstreams["openai_personal"].auth.as_ref().unwrap();
                let prepared = if stream {
                    prepare_provider_stream_v1(&policy, &upstream.config, auth, &descriptor)
                } else {
                    prepare_provider_call_v1(&policy, &upstream.config, auth, &descriptor)
                };
                assert!(
                    matches!(
                        prepared,
                        Err(PrepareProviderCallErrorV1::Ineligible(
                            IneligibleV1::RootRpcPath
                        ))
                    ),
                    "{} stream={stream}: {prepared:?}",
                    kind.provider()
                );
            }
        }
        assert!(
            catalog
                .iter()
                .any(|(_, capability)| capability.model == "fixture-model"
                    && capability.extensions.get("subscription_billing") == Some(&json!(true)))
        );
    }
}

fn subscription_cases() -> [(Kind, &'static str, Value, &'static str); 4] {
    [
        (
            Kind::Claude,
            "http://127.0.0.1:9/claude-plan",
            json!({"claude_transport":"claude-code-process","claude_history_mode":"structured-replay"}),
            "/v1/messages",
        ),
        (
            Kind::Copilot,
            "https://api.githubcopilot.com",
            json!({"copilot_source_kind":"editor","copilot_model_endpoints":{"fixture-model":["/chat/completions"]}}),
            "/chat/completions",
        ),
        (
            Kind::GeminiCli,
            "https://cloudcode-pa.googleapis.com",
            json!({"google_project":"fixture-project","google_client_version":"0.61.0"}),
            "/v1internal:generateContent",
        ),
        (
            Kind::Antigravity,
            "https://daily-cloudcode-pa.googleapis.com",
            json!({"google_project":"fixture-project","google_client_version":"2.9.1"}),
            "/v1internal:generateContent",
        ),
    ]
}

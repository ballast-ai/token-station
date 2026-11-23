use serde_json::{Value, json};
use token_station_cli::config::{ClientConfig, is_subscription_provider};

fn config(kind: &str, provider: &str, endpoint: &str) -> Value {
    let mut value: Value = serde_json::from_str(token_station_cli::EXAMPLE_CONFIG).unwrap();
    let upstream = &mut value["upstreams"]["openai_personal"];
    upstream["provider"] = json!(provider);
    upstream["base_url"] = json!(endpoint);
    upstream["auth"] = json!({"slot":"local_subscription_token","local_subscription":{"kind":kind,"id":"bound-reference"}});
    upstream["subscription_identity"] = json!({});
    value
}

#[test]
fn all_local_channel_contracts_preserve_subscription_billing() {
    for (kind, provider, endpoint) in [
        ("claude", "claude-plan", "http://127.0.0.1:9/claude-plan"),
        ("copilot", "copilot-plan", "https://api.githubcopilot.com"),
        (
            "gemini-cli",
            "gemini-cli-plan",
            "https://cloudcode-pa.googleapis.com",
        ),
        (
            "antigravity",
            "antigravity-plan",
            "https://daily-cloudcode-pa.googleapis.com",
        ),
    ] {
        let parsed: ClientConfig =
            serde_json::from_value(config(kind, provider, endpoint)).unwrap();
        parsed.validate().unwrap();
        assert!(is_subscription_provider(provider));
        let saved = serde_json::to_value(parsed).unwrap();
        assert_eq!(
            saved["upstreams"]["openai_personal"]["auth"]["local_subscription"]["kind"],
            kind
        );
        assert!(
            saved["upstreams"]["openai_personal"]["auth"]
                .get("store")
                .is_none()
        );
    }
}

#[test]
fn account_references_cannot_move_to_other_origins_dialects_or_slots() {
    for patch in [
        json!({"base_url":"https://cloudcode-pa.googleapis.com.evil.test"}),
        json!({"base_url":"http://127.0.0.1:9000"}),
        json!({"provider":"openai-compatible"}),
        json!({"provider":"antigravity-plan"}),
        json!({"auth":{"slot":"provider_api_key","local_subscription":{"kind":"gemini-cli","id":"reference"}}}),
        json!({"auth":{"slot":"local_subscription_token","local_subscription":{"kind":"copilot","id":"reference"}}}),
        json!({"auth":{"slot":"local_subscription_token","store":true,"local_subscription":{"kind":"gemini-cli","id":"reference"}}}),
        json!({"auth":{"slot":"local_subscription_token","local_subscription":{"kind":"gemini-cli","id":"../reference"}}}),
        json!({"subscription_identity":{"access_token":"fixture-token"}}),
        json!({"local":true}),
        json!({"quota_plan":{}}),
        json!({"api_dialect":"responses-native"}),
    ] {
        let mut value = config(
            "gemini-cli",
            "gemini-cli-plan",
            "https://cloudcode-pa.googleapis.com",
        );
        for (key, replacement) in patch.as_object().unwrap() {
            value["upstreams"]["openai_personal"][key] = replacement.clone();
        }
        let parsed: ClientConfig = serde_json::from_value(value).unwrap();
        assert!(parsed.validate().is_err(), "{patch}");
    }
}

#[test]
fn copilot_rejects_port_path_and_unrelated_token_destinations() {
    for endpoint in [
        "https://api.githubcopilot.com:444",
        "https://api.githubcopilot.com/other",
        "https://api.github.com",
        "https://api.githubcopilot.com.evil.test",
    ] {
        let parsed: ClientConfig =
            serde_json::from_value(config("copilot", "copilot-plan", endpoint)).unwrap();
        assert!(parsed.validate().is_err(), "{endpoint}");
    }
}

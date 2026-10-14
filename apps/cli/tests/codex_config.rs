use serde_json::{Value, json};
use token_station_cli::config::ClientConfig;

fn config() -> Value {
    let mut value: Value = serde_json::from_str(token_station_cli::EXAMPLE_CONFIG).unwrap();
    let upstream = &mut value["upstreams"]["openai_personal"];
    upstream["provider"] = json!("codex-plan");
    upstream["base_url"] = json!("https://chatgpt.com/backend-api/codex");
    upstream["auth"] = json!({"slot":"codex_access_token", "codex_account":"local-codex-1"});
    upstream["codex_identity"] = json!({"account_id":"account-1","client_version":"0.162.0"});
    value
}

#[test]
fn a_local_codex_reference_is_a_separate_subscription_connection() {
    let parsed: ClientConfig = serde_json::from_value(config()).unwrap();
    parsed.validate().unwrap();
    let saved = serde_json::to_value(parsed).unwrap();
    assert_eq!(
        saved["upstreams"]["openai_personal"]["auth"]["codex_account"],
        "local-codex-1"
    );
    assert!(
        saved["upstreams"]["openai_personal"]["auth"]
            .get("chatgpt_account")
            .is_none()
    );
}

#[test]
fn codex_credentials_cannot_be_bound_to_another_endpoint_or_dialect() {
    for (field, replacement) in [
        ("base_url", json!("https://example.com/backend-api/codex")),
        ("base_url", json!("http://127.0.0.1:9999/backend-api/codex")),
        ("provider", json!("chatgpt-plan")),
        ("provider", json!("openai-compatible")),
    ] {
        let mut value = config();
        value["upstreams"]["openai_personal"][field] = replacement;
        let parsed: ClientConfig = serde_json::from_value(value).unwrap();
        assert!(parsed.validate().is_err(), "{field}");
    }
}

#[test]
fn codex_rejects_ambiguous_credentials_and_unsupported_transport_modes() {
    for patch in [
        json!({"auth":{"slot":"codex_access_token","codex_account":"local-codex-1","store":true}}),
        json!({"auth":{"slot":"provider_api_key","codex_account":"local-codex-1"}}),
        json!({"auth":{"slot":"codex_access_token","codex_account":"../other"}}),
        json!({"auth":{"slot":"codex_access_token","store":true}}),
        json!({"codex_identity":null}),
        json!({"codex_identity":{"account_id":"account.with.dot","client_version":"0.162.0"}}),
        json!({"codex_identity":{"account_id":"account-1","client_version":"invalid"}}),
        json!({"codex_identity":{"account_id":"bad\nheader","client_version":"0.162.0"}}),
        json!({"quota_plan":{}}),
        json!({"api_dialect":"responses-native"}),
    ] {
        let mut value = config();
        for (key, value_patch) in patch.as_object().unwrap() {
            value["upstreams"]["openai_personal"][key] = value_patch.clone();
        }
        let parsed: ClientConfig = serde_json::from_value(value).unwrap();
        assert!(parsed.validate().is_err(), "{patch}");
    }
}

#[test]
fn codex_loopback_fixtures_require_an_explicit_store_and_slot() {
    for auth in [
        json!(null),
        json!({"slot":"codex_access_token","env":"CODEX_FIXTURE"}),
        json!({"slot":"codex_access_token","file":"fixture.txt"}),
        json!({"slot":"wrong","store":true}),
    ] {
        let mut value = config();
        value["upstreams"]["openai_personal"]["base_url"] = json!("http://127.0.0.1:9999/v1");
        value["upstreams"]["openai_personal"]["auth"] = auth;
        let parsed: ClientConfig = serde_json::from_value(value).unwrap();
        assert!(parsed.validate().is_err());
    }
}

#[test]
#[ignore = "Reads the explicitly authorized local Codex catalog without model inference"]
fn live_local_codex_catalog_contains_selectable_models() {
    assert_eq!(
        std::env::var("TOKEN_STATION_CODEX_LIVE").as_deref(),
        Ok("1")
    );
    let data = std::env::var("TOKEN_STATION_CHATGPT_LIVE_DATA_DIR").unwrap();
    let manager = token_station_cli::codex_accounts::AccountManager::new(data);
    let account = manager
        .connect()
        .expect("Local Codex connection must be ready");
    let models = manager
        .models(&account.id)
        .expect("Codex catalog must be available");
    assert!(!models.is_empty());
    for model in models {
        eprintln!("available model: {}", model.id);
    }
}

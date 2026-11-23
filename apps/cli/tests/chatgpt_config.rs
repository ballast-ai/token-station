use serde_json::{Value, json};
use token_station_cli::config::ClientConfig;

fn config() -> Value {
    let mut value: Value = serde_json::from_str(token_station_cli::EXAMPLE_CONFIG).unwrap();
    let upstream = &mut value["upstreams"]["openai_personal"];
    upstream["provider"] = json!("chatgpt-plan");
    upstream["base_url"] = json!("https://api.openai.com/v1");
    upstream["auth"] = json!({"slot":"chatgpt_access_token", "chatgpt_account":"registration-1"});
    value
}

#[test]
fn accepts_an_isolated_chatgpt_registration_reference() {
    let parsed: ClientConfig = serde_json::from_value(config()).unwrap();
    parsed.validate().unwrap();
}

#[test]
fn rejects_chatgpt_credentials_at_an_arbitrary_endpoint() {
    let mut value = config();
    value["upstreams"]["openai_personal"]["base_url"] = json!("https://example.com/v1");
    let parsed: ClientConfig = serde_json::from_value(value).unwrap();
    assert!(parsed.validate().unwrap_err().contains("ChatGPT"));
}

#[test]
fn rejects_a_subscription_quota_rotation_plan() {
    let mut value = config();
    value["upstreams"]["openai_personal"]["quota_plan"] = json!({});
    let parsed: ClientConfig = serde_json::from_value(value).unwrap();
    assert!(parsed.validate().unwrap_err().contains("ChatGPT"));
}

#[test]
fn rejects_ambiguous_chatgpt_credential_sources() {
    let mut value = config();
    value["upstreams"]["openai_personal"]["auth"]["store"] = json!(true);
    let parsed: ClientConfig = serde_json::from_value(value).unwrap();
    assert!(parsed.validate().is_err());
}

#[test]
fn an_unrelated_signed_out_account_does_not_block_a_probe() {
    let mut value = config();
    value["upstreams"]["mock"] = json!({"provider":"openai-compatible", "base_url":"https://example.com/v1", "models":[{"model":"mock"}]});
    let parsed: ClientConfig = serde_json::from_value(value).unwrap();
    let result = token_station_cli::secrets::SecretStore::snapshot_for_upstream(
        &parsed,
        &std::env::temp_dir(),
        "mock",
    );
    assert!(result.is_ok());
}

//! Select local Codex reuse only for an explicitly requested live acceptance.
use serde_json::{Value, json};
use std::path::Path;

pub fn use_codex() -> bool {
    std::env::var("TOKEN_STATION_CODEX_LIVE").as_deref() == Ok("1")
}

pub fn configure(mut config: Value, upstream: &str, data: &Path) -> Value {
    if !use_codex() {
        return config;
    }
    let manager = token_station_cli::codex_accounts::AccountManager::new(data);
    let account = manager
        .connect()
        .expect("The selected local Codex session must be available");
    config["plugins"]["providers"]["codex-plan"] = json!("provider-chatgpt-plan-v2");
    let row = &mut config["upstreams"][upstream];
    row["provider"] = json!("codex-plan");
    row["base_url"] = json!("https://chatgpt.com/backend-api/codex");
    row["auth"] = json!({"slot":"codex_access_token","codex_account":account.id});
    row["codex_identity"] =
        json!({"account_id":account.account_id,"client_version":account.client_version});
    config
}

//! Desktop commands expose local Codex references, never credential material.
use crate::*;
use token_station_cli::chatgpt_accounts::PlanModel;
use token_station_cli::codex_accounts::{
    AccountManager, AccountView, API_BASE as CODEX_BASE, SLOT as CODEX_SLOT,
};

fn manager(state: &State<'_, AppStateManaged>) -> Result<AccountManager, String> {
    let inner = state.0.lock().map_err(|_| "App state is unavailable")?;
    Ok(AccountManager::new(inner.data_dir()))
}
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|_| "Local Codex operation failed".to_owned())?
}
#[tauri::command]
pub(crate) async fn codex_accounts_list(
    state: State<'_, AppStateManaged>,
) -> Result<Vec<AccountView>, String> {
    let manager = manager(&state)?;
    blocking(move || manager.list()).await
}
#[tauri::command]
pub(crate) async fn codex_account_discover(
    state: State<'_, AppStateManaged>,
) -> Result<AccountView, String> {
    let manager = manager(&state)?;
    blocking(move || manager.discover()).await
}
#[tauri::command]
pub(crate) async fn codex_account_models(
    state: State<'_, AppStateManaged>,
    account_id: String,
) -> Result<Vec<PlanModel>, String> {
    let manager = manager(&state)?;
    blocking(move || manager.models(&account_id)).await
}
#[tauri::command]
pub(crate) async fn codex_account_disconnect(
    state: State<'_, AppStateManaged>,
    account_id: String,
) -> Result<(), String> {
    let manager = manager(&state)?;
    blocking(move || manager.disconnect(&account_id)).await
}

fn upstream_document(account: &AccountView, models: &[PlanModel]) -> Result<Value, String> {
    let safe_identity = |value: &str| {
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    };
    let version = &account.client_version;
    let parts = version
        .split(['-', '+'])
        .next()
        .unwrap_or_default()
        .split('.')
        .collect::<Vec<_>>();
    let valid_version = !version.is_empty()
        && version.len() <= 64
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
        && parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    if account.state != "ready"
        || !safe_identity(&account.id)
        || !safe_identity(&account.account_id)
        || !valid_version
    {
        return Err("A valid local Codex account reference is required".into());
    }
    if models.iter().any(|model| model.id.trim() != model.id) {
        return Err("Codex returned an invalid model identifier".into());
    }
    let ids = normalize_provider_model_ids(models.iter().map(|model| model.id.clone()).collect())?;
    let capabilities = ids.into_iter().map(|id| {
        let model = models.iter().find(|model| model.id == id).expect("normalized catalog model");
        json!({"model":id,"tool":true,"tool_state":"declared",
            "vision":model.supports_images == Some(true),
            "vision_state":match model.supports_images { Some(true)=>"declared",Some(false)=>"unsupported",None=>"unknown" },
            "json_schema":true,"json_schema_state":"declared",
            "context_window":model.context_window.and_then(|value|u32::try_from(value).ok()).unwrap_or(0)})
    }).collect::<Vec<_>>();
    Ok(json!({"provider":"codex-plan","base_url":CODEX_BASE,
        "auth":{"slot":CODEX_SLOT,"codex_account":account.id},
        "codex_identity":{"account_id":account.account_id,"client_version":account.client_version},
        "provider_call":DEFAULT_PROVIDER_CALL,"models":capabilities}))
}

#[tauri::command]
pub(crate) async fn codex_connect_provider(
    state: State<'_, AppStateManaged>,
    account_id: Option<String>,
) -> Result<String, String> {
    let data_dir = state
        .0
        .lock()
        .map_err(|_| "App state is unavailable")?
        .data_dir();
    let account_manager = AccountManager::new(&data_dir);
    let (account, models) = blocking(move || {
        let id = match account_id {
            Some(id) => id,
            None => account_manager.connect()?.id,
        };
        let models = account_manager.models(&id)?;
        let account = account_manager
            .list()?
            .into_iter()
            .find(|account| account.id == id && account.state == "ready")
            .ok_or_else(|| "Check the local Codex login before reconnecting".to_owned())?;
        Ok((account, models))
    })
    .await?;
    let upstream = upstream_document(&account, &models)?;
    let name = format!("codex_{}", account.id);
    let mut inner = state.0.lock().map_err(|_| "App state is unavailable")?;
    if inner.data_dir() != data_dir {
        return Err("The data directory changed. Retry the connection".into());
    }
    if provider_tombstones::contains(&data_dir, &name)? {
        return Err("Restore the deleted Codex provider before reconnecting".into());
    }
    inner.edit_validated_draft(|inner| {
        if let Some(existing) = inner.draft["upstreams"].get(&name) {
            if existing["provider"] != "codex-plan"
                || existing["auth"]["codex_account"] != account.id
            {
                return Err("The provider name is already in use".into());
            }
        }
        inner.draft["plugins"]["providers"]["codex-plan"] = json!("provider-chatgpt-plan");
        inner.draft["upstreams"][&name] = upstream;
        Ok(())
    })?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn account() -> AccountView {
        AccountView {
            id: "local-1".into(),
            label: "Local Codex".into(),
            email: None,
            state: "ready".into(),
            account_id: "account-1".into(),
            client_version: "1.0.0".into(),
        }
    }
    fn models() -> Vec<PlanModel> {
        vec![PlanModel {
            id: "live-model".into(),
            name: "Live model".into(),
            context_window: None,
            supports_images: None,
        }]
    }
    #[test]
    fn local_reference_contains_only_bound_identity_and_observed_catalog() {
        let document = upstream_document(&account(), &models()).unwrap();
        let parsed: token_station_cli::config::UpstreamConfig =
            serde_json::from_value(document.clone()).unwrap();
        assert_eq!(parsed.provider, "codex-plan");
        assert_eq!(document["base_url"], CODEX_BASE);
        assert_eq!(document["provider"], "codex-plan");
        assert_eq!(
            document["auth"],
            json!({"slot":CODEX_SLOT,"codex_account":"local-1"})
        );
        assert_eq!(
            document["codex_identity"],
            json!({"account_id":"account-1","client_version":"1.0.0"})
        );
        assert_eq!(document["models"][0]["context_window"], 0);
        assert_eq!(document["models"][0]["vision_state"], "unknown");
        assert!(document.get("quota_plan").is_none());
        assert!(document["models"][0].get("catalog_cost").is_none());
        assert!(document.get("extensions").is_none());
    }
    #[test]
    fn invalid_or_changed_sources_and_empty_catalogs_cannot_connect() {
        assert!(upstream_document(&account(), &[]).is_err());
        for state in [
            "missing",
            "expired",
            "account_changed",
            "api_key_only",
            "unsupported_storage",
        ] {
            let mut account = account();
            account.state = state.into();
            assert!(upstream_document(&account, &models()).is_err());
        }
        let mut invalid_account = account();
        invalid_account.id = "../source".into();
        assert!(upstream_document(&invalid_account, &models()).is_err());
        let mut invalid_identity = account();
        invalid_identity.client_version = "version\r\ninjected".into();
        assert!(upstream_document(&invalid_identity, &models()).is_err());
        let mut dotted_account = account();
        dotted_account.account_id = "account.invalid".into();
        assert!(upstream_document(&dotted_account, &models()).is_err());
        for version in [
            "release",
            "1.2",
            "1.2.3_bad",
            &format!("1.2.3-{}", "a".repeat(59)),
        ] {
            let mut invalid_version = account();
            invalid_version.client_version = version.into();
            assert!(upstream_document(&invalid_version, &models()).is_err());
        }
        let mut prerelease = account();
        prerelease.client_version = "0.162.0-alpha.17.2+build".into();
        assert!(upstream_document(&prerelease, &models()).is_ok());
        let mut invalid = models();
        invalid[0].id = " model ".into();
        assert!(upstream_document(&account(), &invalid).is_err());
    }
}

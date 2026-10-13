use crate::*;
use token_station_cli::chatgpt_accounts::{
    AccountManager, AccountView, LoginAttempt, LoginStatus, PlanModel, SignOutResult, API_BASE,
    SLOT,
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
        .map_err(|_| "ChatGPT account operation failed".to_owned())?
}

#[tauri::command]
pub(crate) async fn chatgpt_accounts_list(
    state: State<'_, AppStateManaged>,
) -> Result<Vec<AccountView>, String> {
    let manager = manager(&state)?;
    blocking(move || manager.list()).await
}

#[tauri::command]
pub(crate) async fn chatgpt_login_start(
    state: State<'_, AppStateManaged>,
    account_id: Option<String>,
) -> Result<LoginAttempt, String> {
    let manager = manager(&state)?;
    blocking(move || manager.start_login(account_id.as_deref())).await
}

#[tauri::command]
pub(crate) async fn chatgpt_login_status(
    state: State<'_, AppStateManaged>,
    attempt_id: String,
) -> Result<LoginStatus, String> {
    let manager = manager(&state)?;
    blocking(move || manager.login_status(&attempt_id)).await
}

#[tauri::command]
pub(crate) async fn chatgpt_login_cancel(
    state: State<'_, AppStateManaged>,
    attempt_id: String,
) -> Result<(), String> {
    let manager = manager(&state)?;
    blocking(move || manager.cancel_login(&attempt_id)).await
}

#[tauri::command]
pub(crate) async fn chatgpt_models(
    state: State<'_, AppStateManaged>,
    account_id: String,
) -> Result<Vec<PlanModel>, String> {
    let manager = manager(&state)?;
    blocking(move || manager.models(&account_id)).await
}

#[tauri::command]
pub(crate) async fn chatgpt_sign_out(
    state: State<'_, AppStateManaged>,
    account_id: String,
) -> Result<SignOutResult, String> {
    let manager = manager(&state)?;
    blocking(move || manager.sign_out(&account_id)).await
}

fn upstream_document(account_id: &str, models: &[PlanModel]) -> Result<Value, String> {
    if account_id.is_empty()
        || account_id.len() > 128
        || !account_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("Invalid ChatGPT account reference".to_owned());
    }
    if models.iter().any(|model| model.id.trim() != model.id) {
        return Err("ChatGPT returned an invalid model identifier".to_owned());
    }
    let ids = normalize_provider_model_ids(models.iter().map(|model| model.id.clone()).collect())?;
    let capabilities = ids.into_iter().map(|id| {
        let model = models.iter().find(|model| model.id == id).expect("normalized catalog model");
        json!({
            "model": id, "tool": true, "tool_state": "declared",
            "vision": model.supports_images == Some(true),
            "vision_state": match model.supports_images {Some(true)=>"declared",Some(false)=>"unsupported",None=>"unknown"},
            "json_schema": false, "json_schema_state": "unsupported",
            "context_window": model.context_window.and_then(|value| u32::try_from(value).ok()).unwrap_or(0),
        })
    }).collect::<Vec<_>>();
    Ok(json!({
        "provider": "chatgpt-plan", "base_url": API_BASE,
        "auth": {"slot": SLOT, "chatgpt_account": account_id},
        "provider_call": DEFAULT_PROVIDER_CALL, "models": capabilities,
    }))
}

#[tauri::command]
pub(crate) async fn chatgpt_connect_provider(
    state: State<'_, AppStateManaged>,
    account_id: String,
) -> Result<String, String> {
    let data_dir = state
        .0
        .lock()
        .map_err(|_| "App state is unavailable")?
        .data_dir();
    let account_manager = AccountManager::new(&data_dir);
    let lookup_id = account_id.clone();
    let models = blocking(move || {
        let models = account_manager.models(&lookup_id)?;
        if !account_manager
            .list()?
            .iter()
            .any(|account| account.id == lookup_id && account.state == "ready")
        {
            return Err("Sign in to an eligible ChatGPT account first".to_owned());
        }
        Ok(models)
    })
    .await?;
    let upstream = upstream_document(&account_id, &models)?;
    let name = format!("chatgpt_{account_id}");
    let mut inner = state.0.lock().map_err(|_| "App state is unavailable")?;
    if inner.data_dir() != data_dir {
        return Err("The data directory changed. Retry the connection".to_owned());
    }
    if provider_tombstones::contains(&data_dir, &name)? {
        return Err("Restore the deleted ChatGPT provider before reconnecting".to_owned());
    }
    inner.edit_validated_draft(|inner| {
        if let Some(existing) = inner.draft["upstreams"].get(&name) {
            if existing["provider"] != "chatgpt-plan"
                || existing["auth"]["chatgpt_account"] != account_id
            {
                return Err("The provider name is already in use".to_owned());
            }
        }
        inner.draft["plugins"]["providers"]["chatgpt-plan"] = json!("provider-chatgpt-plan");
        inner.draft["upstreams"][&name] = upstream;
        Ok(())
    })?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_reference_has_no_secret_or_invented_limits() {
        let document = upstream_document(
            "registration-1",
            &[PlanModel {
                id: "gpt-example".into(),
                name: "GPT Example".into(),
                context_window: None,
                supports_images: None,
            }],
        )
        .unwrap();
        assert_eq!(document["base_url"], API_BASE);
        assert_eq!(
            document["auth"],
            json!({"slot": SLOT, "chatgpt_account": "registration-1"})
        );
        assert_eq!(document["models"][0]["context_window"], 0);
        assert_eq!(document["models"][0]["vision_state"], "unknown");
        assert!(document.get("quota_plan").is_none());
        assert!(document["models"][0].get("catalog_cost").is_none());
    }

    #[test]
    fn empty_catalog_and_invalid_registration_cannot_create_providers() {
        assert!(upstream_document("registration-1", &[]).is_err());
        assert!(upstream_document("../account", &[]).is_err());
    }
    #[test]
    fn malformed_remote_model_identifiers_return_an_error() {
        assert!(upstream_document(
            "registration-1",
            &[PlanModel {
                id: " gpt-example ".into(),
                name: "Example".into(),
                context_window: None,
                supports_images: None,
            }]
        )
        .is_err());
    }
}

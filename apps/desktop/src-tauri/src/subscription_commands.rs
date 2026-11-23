//! Safe local subscription reference commands. Credentials stay in the Host.
use crate::*;
use token_station_cli::local_subscriptions::{
    AccountManager, AccountView, Connection, Kind, Model,
};

fn root(state: &State<'_, AppStateManaged>) -> Result<PathBuf, String> {
    Ok(state
        .0
        .lock()
        .map_err(|_| "App state is unavailable")?
        .data_dir())
}
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|_| "Local subscription operation failed".to_owned())?
}
#[tauri::command]
pub(crate) async fn subscription_discover(
    state: State<'_, AppStateManaged>,
    kind: Kind,
) -> Result<AccountView, String> {
    let manager = AccountManager::new(root(&state)?, kind);
    blocking(move || manager.discover()).await
}
#[tauri::command]
pub(crate) async fn subscription_accounts_list(
    state: State<'_, AppStateManaged>,
    kind: Kind,
) -> Result<Vec<AccountView>, String> {
    let manager = AccountManager::new(root(&state)?, kind);
    blocking(move || manager.list()).await
}
#[tauri::command]
pub(crate) async fn subscription_models(
    state: State<'_, AppStateManaged>,
    kind: Kind,
    account_id: String,
) -> Result<Vec<Model>, String> {
    let manager = AccountManager::new(root(&state)?, kind);
    blocking(move || manager.models(&account_id)).await
}
#[tauri::command]
pub(crate) async fn subscription_disconnect(
    state: State<'_, AppStateManaged>,
    kind: Kind,
    account_id: String,
) -> Result<(), String> {
    let manager = AccountManager::new(root(&state)?, kind);
    blocking(move || manager.disconnect(&account_id)).await
}

fn upstream_document(kind: Kind, connection: &Connection) -> Result<Value, String> {
    let id = &connection.account.id;
    if connection.account.state != "ready"
        || id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || connection
            .models
            .iter()
            .any(|model| model.id.trim() != model.id)
    {
        return Err("A valid local subscription identity and catalog are required".into());
    }
    let ids = normalize_provider_model_ids(
        connection
            .models
            .iter()
            .map(|model| model.id.clone())
            .collect(),
    )?;
    if ids.is_empty() {
        return Err("The subscription account returned no available models".into());
    }
    let models = ids.into_iter().map(|id| {
        let model = connection.models.iter().find(|model| model.id == id).expect("normalized catalog model");
        let evidence = |value: Option<bool>| match value { Some(true) => "declared", Some(false) => "unsupported", None => "unknown" };
        json!({"model":id, "tool":model.supports_tools == Some(true), "tool_state":evidence(model.supports_tools),
            "vision":model.supports_images == Some(true), "vision_state":evidence(model.supports_images),
            "context_window":model.context_window.and_then(|value| u32::try_from(value).ok()).unwrap_or(0)})
    }).collect::<Vec<_>>();
    let document = json!({"provider":kind.provider(), "base_url":connection.base_url,
        "auth":{"slot":"local_subscription_token","local_subscription":{"kind":kind,"id":id}},
        "subscription_identity":connection.metadata, "provider_call":if kind == Kind::Claude { "legacy" } else { DEFAULT_PROVIDER_CALL }, "models":models});
    let _: token_station_cli::config::UpstreamConfig = serde_json::from_value(document.clone())
        .map_err(|_| "Invalid subscription configuration")?;
    Ok(document)
}

#[tauri::command]
pub(crate) async fn subscription_connect_provider(
    state: State<'_, AppStateManaged>,
    kind: Kind,
    account_id: Option<String>,
) -> Result<String, String> {
    let data_dir = root(&state)?;
    let manager = AccountManager::new(&data_dir, kind);
    let connection = blocking(move || {
        let id = match account_id {
            Some(id) => id,
            None => manager.connect()?.id,
        };
        manager.connection(&id)
    })
    .await?;
    let document = upstream_document(kind, &connection)?;
    let name = format!(
        "{}_{}",
        kind.provider().replace('-', "_"),
        connection.account.id
    );
    let mut inner = state.0.lock().map_err(|_| "App state is unavailable")?;
    if inner.data_dir() != data_dir {
        return Err("The data directory changed. Retry the connection".into());
    }
    if provider_tombstones::contains(&data_dir, &name)? {
        return Err("Restore the deleted subscription provider before reconnecting".into());
    }
    inner.edit_validated_draft(|inner| {
        if let Some(existing) = inner.draft["upstreams"].get(&name) {
            if existing["provider"] != kind.provider()
                || existing["auth"]["local_subscription"]["id"] != connection.account.id
            {
                return Err("The provider name is already in use".into());
            }
        }
        inner.draft["plugins"]["providers"][kind.provider()] = json!(kind.package());
        inner.draft["upstreams"][&name] = document;
        Ok(())
    })?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn connection() -> Connection {
        Connection {
            account: AccountView {
                id: "bound1".into(),
                label: "Local account".into(),
                email: None,
                state: "ready".into(),
            },
            base_url: "https://cloudcode-pa.googleapis.com".into(),
            models: vec![Model {
                id: "observed-model".into(),
                name: "Observed model".into(),
                context_window: None,
                supports_images: None,
                supports_tools: None,
            }],
            metadata: BTreeMap::from([("google_project".into(), json!("fixture-project"))]),
        }
    }
    #[test]
    fn document_contains_only_opaque_reference_and_observed_capabilities() {
        let document = upstream_document(Kind::GeminiCli, &connection()).unwrap();
        assert_eq!(document["auth"]["local_subscription"]["id"], "bound1");
        assert_eq!(document["models"][0]["vision_state"], "unknown");
        assert_eq!(document["models"][0]["tool_state"], "unknown");
        assert_eq!(document["models"][0]["context_window"], 0);
        assert!(document["auth"].get("store").is_none());
        assert!(document["models"][0].get("catalog_cost").is_none());
    }
    #[test]
    fn disconnected_identity_and_empty_catalog_cannot_create_provider() {
        let mut value = connection();
        value.account.state = "account_changed".into();
        assert!(upstream_document(Kind::GeminiCli, &value).is_err());
        value.account.state = "ready".into();
        value.models.clear();
        assert!(upstream_document(Kind::GeminiCli, &value).is_err());
    }
}

//! Idempotent startup discovery without changing the user's routes or draft edits.
use crate::*;
use token_station_cli::chatgpt_accounts::PlanModel;
use token_station_cli::codex_accounts::{AccountManager, AccountView};

const RECEIPTS_FILE: &str = "codex-auto-discovery.json";
const FLIGHT: &str = "__local_codex_auto_discovery__";

fn receipts(data_dir: &Path) -> Result<BTreeSet<String>, String> {
    let path = data_dir.join(RECEIPTS_FILE);
    let bytes = match std::fs::read(path) {
        Ok(bytes) if bytes.len() <= 65536 => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        _ => return Err("Cannot read local Codex discovery history.".into()),
    };
    let ids: BTreeSet<String> = serde_json::from_slice(&bytes)
        .map_err(|_| "Cannot read local Codex discovery history.".to_owned())?;
    if ids
        .iter()
        .any(|id| id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err("Local Codex discovery history is invalid.".into());
    }
    Ok(ids)
}
fn write_receipts(data_dir: &Path, ids: &BTreeSet<String>) -> Result<(), String> {
    let bytes = serde_json::to_vec(ids)
        .map_err(|_| "Cannot save local Codex discovery history.".to_owned())?;
    agent_integration::safe_fs::write_atomic_private(&data_dir.join(RECEIPTS_FILE), &bytes)
        .map_err(|_| "Cannot save local Codex discovery history.".to_owned())
}
fn existing_codex(inner: &AppInner) -> bool {
    inner.draft["upstreams"]
        .as_object()
        .is_some_and(|upstreams| {
            upstreams.values().any(|upstream| {
                upstream["provider"] == "codex-plan"
                    || upstream["auth"]["codex_account"].is_string()
            })
        })
}
fn remember_existing(inner: &AppInner) -> Result<(), String> {
    let data_dir = inner.data_dir();
    let mut ids = receipts(&data_dir)?;
    let before = ids.clone();
    if let Some(upstreams) = inner.draft["upstreams"].as_object() {
        for upstream in upstreams.values() {
            if let Some(id) = upstream["auth"]["codex_account"]
                .as_str()
                .filter(|id| id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
            {
                ids.insert(id.to_owned());
            }
        }
    }
    if ids != before {
        write_receipts(&data_dir, &ids)?;
    }
    Ok(())
}
fn eligible(inner: &AppInner, id: &str) -> Result<bool, String> {
    if inner.load_error.is_some() || inner.config_state.is_dirty() || existing_codex(inner) {
        return Ok(false);
    }
    let data_dir = inner.data_dir();
    if receipts(&data_dir)?.contains(id) {
        return Ok(false);
    }
    let name = format!("codex_{id}");
    if inner.draft["upstreams"].get(&name).is_some() {
        return Ok(false);
    }
    if inner.draft["plugins"]["providers"]["codex-plan"]
        .as_str()
        .is_some_and(|plugin| plugin != "provider-chatgpt-plan")
    {
        return Ok(false);
    }
    for deleted in provider_tombstones::names(&data_dir)? {
        if deleted == name
            || provider_tombstones::get(&data_dir, &deleted)?
                .is_some_and(|upstream| upstream["auth"]["codex_account"].as_str() == Some(id))
        {
            return Ok(false);
        }
    }
    Ok(true)
}
fn attach_discovered(
    inner: &mut AppInner,
    account: &AccountView,
    models: &[PlanModel],
) -> Result<bool, String> {
    if !eligible(inner, &account.id)? {
        return Ok(false);
    }
    let upstream = crate::codex_commands::upstream_document(account, models)?;
    let data_dir = inner.data_dir();
    let previous_receipts = receipts(&data_dir)?;
    let mut next_receipts = previous_receipts.clone();
    next_receipts.insert(account.id.clone());
    let previous_draft = inner.draft.clone();
    let initialize_route = inner.materialize()?.home_route_is_unconfigured();
    let name = format!("codex_{}", account.id);
    let result = inner
        .edit_validated_draft(|inner| {
            inner.draft["plugins"]["providers"]["codex-plan"] = json!("provider-chatgpt-plan");
            if initialize_route {
                inner.draft["routing"]["direct_target"] =
                    json!({"upstream":name,"model":upstream["models"][0]["model"]});
            }
            inner.draft["upstreams"][&name] = upstream;
            Ok(())
        })
        .and_then(|()| inner.save_draft_without_backfill());
    if result.is_err() {
        inner.draft = previous_draft;
        // Preserve the revision watermark and invalidate in-flight provider snapshots.
        inner.observe_draft()?;
        return Err("Could not save the automatically discovered Codex provider.".into());
    }
    // The config is already durable. On a trailing receipt failure, startup sees the
    // provider and retries remember_existing before any new discovery.
    if write_receipts(&data_dir, &next_receipts).is_err() {
        eprintln!("Local Codex provider saved; discovery history will be repaired at startup.");
    }
    Ok(true)
}

/// Discover once per startup without blocking the UI or changing existing bindings.
#[tauri::command]
pub(crate) async fn codex_auto_connect_provider(
    state: State<'_, AppStateManaged>,
) -> Result<bool, String> {
    discover_and_connect(&state.0).await
}

async fn discover_and_connect(state: &Mutex<AppInner>) -> Result<bool, String> {
    let (data_dir, revision, epochs) = {
        let mut inner = state.lock().map_err(|_| "App state is unavailable")?;
        if inner.load_error.is_some() || inner.config_state.is_dirty() {
            return Ok(false);
        }
        if existing_codex(&inner) {
            remember_existing(&inner)?;
            return Ok(false);
        }
        if !inner.pending_provider_discoveries.insert(FLIGHT.into()) {
            return Ok(false);
        }
        (
            inner.data_dir(),
            inner.config_state.draft_revision(),
            inner.upstream_epochs.clone(),
        )
    };
    let _guard = ProviderDiscoveryGuard {
        inner: state,
        provider: FLIGHT.into(),
    };
    let manager = AccountManager::new(&data_dir);
    let discovered = {
        let manager = manager.clone();
        tauri::async_runtime::spawn_blocking(move || manager.discover())
            .await
            .map_err(|_| "Local Codex discovery failed.")??
    };
    if !matches!(discovered.state.as_str(), "ready" | "expired") {
        return Ok(false);
    }
    {
        let inner = state.lock().map_err(|_| "App state is unavailable")?;
        if inner.data_dir() != data_dir
            || inner.config_state.draft_revision() != revision
            || inner.upstream_epochs != epochs
            || !eligible(&inner, &discovered.id)?
        {
            return Ok(false);
        }
    }
    if !manager.auto_connect_allowed(&discovered.id)? {
        return Ok(false);
    }
    let source_id = discovered.id;
    let expected_id = source_id.clone();
    let (account, models) = tauri::async_runtime::spawn_blocking(move || {
        let account = manager.connect_automatically()?;
        if account.id != expected_id {
            return Err("The local Codex account changed during discovery.".to_owned());
        }
        let models = manager.models(&account.id)?;
        let current = manager.account(&account.id)?;
        if current.state != "ready" {
            return Err("The local Codex login changed during discovery.".to_owned());
        }
        Ok((current, models))
    })
    .await
    .map_err(|_| "Local Codex discovery failed.")??;
    let mut inner = state.lock().map_err(|_| "App state is unavailable")?;
    if inner.data_dir() != data_dir
        || inner.config_state.draft_revision() != revision
        || inner.upstream_epochs != epochs
    {
        return Ok(false);
    }
    // Recheck the on-disk source and registration after the asynchronous model fetch.
    let current = AccountManager::new(&data_dir).account(&source_id)?;
    if current.id != account.id || current.state != "ready" {
        return Ok(false);
    }
    attach_discovered(&mut inner, &current, &models)
}

#[cfg(test)]
mod tests {
    use super::*;
    static NEXT_FIXTURE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    struct Fixture {
        root: PathBuf,
        inner: AppInner,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "ts-codex-auto-{}-{}-{}",
                NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let draft = template(&root.join("data"), &root.join("plugins"));
            let inner = AppInner::new(root.join("token-station.json"), draft, None);
            Self { root, inner }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn account() -> AccountView {
        AccountView {
            id: "1234567890abcdef1234567890abcdef".into(),
            label: "Fixture".into(),
            email: None,
            state: "ready".into(),
            account_id: "fixture-account".into(),
            client_version: "0.162.0".into(),
        }
    }
    fn models() -> Vec<PlanModel> {
        vec![PlanModel {
            id: "fixture-model".into(),
            name: "Fixture model".into(),
            context_window: Some(128000),
            supports_images: Some(true),
        }]
    }
    #[test]
    fn fresh_install_adds_and_saves_one_provider_with_the_first_setup_target() {
        let mut f = Fixture::new();
        let router = f.inner.draft["router"].clone();
        assert!(attach_discovered(&mut f.inner, &account(), &models()).unwrap());
        assert!(!f.inner.config_state.is_dirty());
        assert_eq!(
            f.inner.draft["routing"]["direct_target"],
            json!({"upstream":format!("codex_{}",account().id),"model":"fixture-model"})
        );
        assert_eq!(f.inner.draft["router"], router);
        let saved: Value =
            serde_json::from_slice(&std::fs::read(&f.inner.config_path).unwrap()).unwrap();
        let name = format!("codex_{}", account().id);
        assert_eq!(
            saved["upstreams"][&name]["auth"]["codex_account"],
            account().id
        );
        assert_eq!(
            saved["upstreams"][&name]["models"][0]["model"],
            "fixture-model"
        );
        assert_eq!(
            saved["upstreams"][&name]["models"][0]["context_window"],
            128000
        );
        assert!(!attach_discovered(&mut f.inner, &account(), &models()).unwrap());
    }
    #[test]
    fn dirty_and_read_only_configs_are_never_written() {
        let mut f = Fixture::new();
        f.inner
            .edit_validated_draft(|inner| {
                inner.draft["server"]["listen"] = json!("127.0.0.1:19876");
                Ok(())
            })
            .unwrap();
        let before = f.inner.draft.clone();
        assert!(!attach_discovered(&mut f.inner, &account(), &models()).unwrap());
        assert_eq!(f.inner.draft, before);
        assert!(!f.inner.config_path.exists());
        let mut read_only = Fixture::new();
        read_only.inner.load_error = Some("fixture error".into());
        assert!(!attach_discovered(&mut read_only.inner, &account(), &models()).unwrap());
        assert!(!read_only.inner.config_path.exists());
    }
    #[test]
    fn deleted_or_disconnected_provider_is_not_resurrected() {
        let mut f = Fixture::new();
        assert!(attach_discovered(&mut f.inner, &account(), &models()).unwrap());
        let name = format!("codex_{}", account().id);
        f.inner
            .edit_validated_draft(|inner| {
                inner.draft["upstreams"]
                    .as_object_mut()
                    .unwrap()
                    .remove(&name);
                inner.draft["routing"]
                    .as_object_mut()
                    .unwrap()
                    .remove("direct_target");
                Ok(())
            })
            .unwrap();
        f.inner.save_draft_without_backfill().unwrap();
        assert!(!attach_discovered(&mut f.inner, &account(), &models()).unwrap());
        let mut tombstone = Fixture::new();
        provider_tombstones::archive(
            &tombstone.inner.data_dir(),
            "renamed-local",
            &json!({"provider":"codex-plan","auth":{"codex_account":account().id}}),
        )
        .unwrap();
        assert!(!attach_discovered(&mut tombstone.inner, &account(), &models()).unwrap());
    }
    #[test]
    fn another_codex_binding_is_preserved_after_local_account_switch() {
        let mut f = Fixture::new();
        assert!(attach_discovered(&mut f.inner, &account(), &models()).unwrap());
        let before = f.inner.draft.clone();
        let mut changed = account();
        changed.id = "abcdef1234567890abcdef1234567890".into();
        assert!(!attach_discovered(&mut f.inner, &changed, &models()).unwrap());
        assert_eq!(f.inner.draft, before);
    }
    #[test]
    fn invalid_catalog_does_not_suppress_a_later_successful_retry() {
        let mut f = Fixture::new();
        assert!(attach_discovered(&mut f.inner, &account(), &[]).is_err());
        assert!(attach_discovered(&mut f.inner, &account(), &models()).unwrap());
    }
    #[test]
    fn unrelated_provider_is_preserved_and_plugin_conflicts_are_not_overwritten() {
        let mut f = Fixture::new();
        f.inner.draft["upstreams"]["user_api"] = json!({"provider":"openai-compatible","base_url":"https://example.test/v1","models":[{"model":"fixture-api"}]});
        f.inner.draft["routing"]["direct_target"] =
            json!({"upstream":"user_api","model":"fixture-api"});
        f.inner.observe_draft().unwrap();
        f.inner.save_draft_without_backfill().unwrap();
        let original = f.inner.draft["upstreams"]["user_api"].clone();
        let original_route = f.inner.draft["routing"].clone();
        assert!(attach_discovered(&mut f.inner, &account(), &models()).unwrap());
        assert_eq!(f.inner.draft["upstreams"]["user_api"], original);
        assert_eq!(f.inner.draft["routing"], original_route);
        let mut conflict = Fixture::new();
        conflict.inner.draft["plugins"]["providers"]["codex-plan"] = json!("custom-plugin");
        conflict.inner.observe_draft().unwrap();
        conflict.inner.save_draft_without_backfill().unwrap();
        assert!(!attach_discovered(&mut conflict.inner, &account(), &models()).unwrap());
        assert_eq!(
            conflict.inner.draft["plugins"]["providers"]["codex-plan"],
            "custom-plugin"
        );
    }
    #[test]
    fn failed_config_save_can_retry_and_keeps_revision_watermarks() {
        let mut f = Fixture::new();
        std::fs::create_dir(&f.inner.config_path).unwrap();
        let before = f.inner.draft.clone();
        assert!(attach_discovered(&mut f.inner, &account(), &models()).is_err());
        assert_eq!(f.inner.draft, before);
        assert!(!receipts(&f.inner.data_dir())
            .unwrap()
            .contains(&account().id));
        std::fs::remove_dir(&f.inner.config_path).unwrap();
        assert!(attach_discovered(&mut f.inner, &account(), &models()).unwrap());
    }
    #[test]
    fn committed_provider_repairs_a_missing_receipt_before_any_rediscovery() {
        let mut f = Fixture::new();
        assert!(attach_discovered(&mut f.inner, &account(), &models()).unwrap());
        std::fs::remove_file(f.inner.data_dir().join(RECEIPTS_FILE)).unwrap();
        let placeholder = AppInner::new(
            f.root.join("placeholder.json"),
            template(&f.root.join("placeholder-data"), &f.root.join("plugins")),
            None,
        );
        let state = Mutex::new(std::mem::replace(&mut f.inner, placeholder));
        assert!(!tauri::async_runtime::block_on(discover_and_connect(&state)).unwrap());
        f.inner = state.into_inner().unwrap();
        assert!(receipts(&f.inner.data_dir())
            .unwrap()
            .contains(&account().id));
    }
    #[test]
    #[ignore = "Reads the existing local Codex login and fetches its live catalog without an isolated auth source."]
    fn live_fresh_install_automatically_registers_existing_local_codex() {
        assert_eq!(
            std::env::var("TOKEN_STATION_TEST_LOCAL_CODEX").as_deref(),
            Ok("1")
        );
        let mut f = Fixture::new();
        let placeholder = AppInner::new(
            f.root.join("placeholder.json"),
            template(&f.root.join("placeholder-data"), &f.root.join("plugins")),
            None,
        );
        let state = Mutex::new(std::mem::replace(&mut f.inner, placeholder));
        assert!(tauri::async_runtime::block_on(discover_and_connect(&state)).unwrap());
        assert!(!tauri::async_runtime::block_on(discover_and_connect(&state)).unwrap());
        f.inner = state.into_inner().unwrap();
        let saved: Value =
            serde_json::from_slice(&std::fs::read(&f.inner.config_path).unwrap()).unwrap();
        let providers = saved["upstreams"].as_object().unwrap();
        assert_eq!(providers.len(), 1);
        let provider = providers.values().next().unwrap();
        assert_eq!(provider["provider"], "codex-plan");
        assert!(!provider["models"].as_array().unwrap().is_empty());
        assert!(provider["auth"]["codex_account"].is_string());
        assert!(provider["auth"].get("access_token").is_none());
        assert!(!f.inner.config_state.is_dirty());
        eprintln!("Fresh local Codex discovery: one saved provider, {} live models, no duplicate on second run", provider["models"].as_array().unwrap().len());
    }
}

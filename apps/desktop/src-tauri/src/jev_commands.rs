//! Desktop controls for the optional cloud classifier. Saved keys never cross IPC.
use std::sync::Arc;

use tauri::State;
use token_station_cli::config::EgressConfig;
use token_station_cli::jev::{JevController, Status};

use crate::{AppInner, AppStateManaged};

fn controller_for(inner: &AppInner, editable: bool) -> Result<Arc<JevController>, String> {
    if editable {
        inner.ensure_editable()?;
    }
    Ok(JevController::shared(&inner.data_dir()))
}

fn edit_jev(
    inner: &AppInner,
    operation: impl FnOnce(&JevController) -> Result<Status, String>,
) -> Result<Status, String> {
    let controller = controller_for(inner, true)?;
    operation(&controller)
}

fn connection_context(inner: &AppInner) -> Result<(Arc<JevController>, EgressConfig), String> {
    let controller = controller_for(inner, true)?;
    let egress = crate::provider_commands::draft_egress_config(&inner.draft)?;
    egress.proxy_parts()?;
    Ok((controller, egress))
}

#[tauri::command]
pub(crate) fn get_jev_status(state: State<'_, AppStateManaged>) -> Result<Status, String> {
    let inner = state
        .0
        .lock()
        .map_err(|_| "Cannot read routing settings.")?;
    Ok(controller_for(&inner, false)?.status())
}

#[tauri::command]
pub(crate) fn save_jev_key(
    state: State<'_, AppStateManaged>,
    api_key: String,
) -> Result<Status, String> {
    let key = zeroize::Zeroizing::new(api_key);
    let inner = state
        .0
        .lock()
        .map_err(|_| "Cannot save routing settings.")?;
    edit_jev(&inner, |controller| controller.save_key(&key))
}

#[tauri::command]
pub(crate) fn clear_jev_key(state: State<'_, AppStateManaged>) -> Result<Status, String> {
    let inner = state
        .0
        .lock()
        .map_err(|_| "Cannot save routing settings.")?;
    edit_jev(&inner, JevController::clear_key)
}

#[tauri::command]
pub(crate) fn set_jev_enabled(
    state: State<'_, AppStateManaged>,
    enabled: bool,
) -> Result<Status, String> {
    let inner = state
        .0
        .lock()
        .map_err(|_| "Cannot save routing settings.")?;
    edit_jev(&inner, |controller| controller.set_enabled(enabled))
}

#[tauri::command]
pub(crate) async fn test_jev_connection(
    state: State<'_, AppStateManaged>,
) -> Result<Status, String> {
    let (controller, egress) = {
        let inner = state
            .0
            .lock()
            .map_err(|_| "Cannot read routing settings.")?;
        connection_context(&inner)?
    };
    tauri::async_runtime::spawn_blocking(move || controller.test_connection(&egress))
        .await
        .map_err(|_| "Jev connection test failed.".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture {
        root: PathBuf,
        inner: AppInner,
    }

    impl Fixture {
        fn new() -> Self {
            static ID: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "token-station-jev-command-{}-{}",
                std::process::id(),
                ID.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).unwrap();
            let draft = crate::config_draft::template(&root, &root.join("plugins"));
            let inner =
                AppInner::new_with_saved(root.join("config.json"), draft.clone(), draft, None);
            Self { root, inner }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn saves_and_removes_only_the_cloud_credential_without_mutating_the_route_draft() {
        let fixture = Fixture::new();
        let draft = fixture.inner.draft.clone();
        let controller = controller_for(&fixture.inner, false).unwrap();
        assert!(!controller.status().enabled);
        assert!(!controller.status().has_key);
        edit_jev(&fixture.inner, |controller| {
            controller.save_key("synthetic-jev-key")
        })
        .unwrap();
        let enabled = edit_jev(&fixture.inner, |controller| controller.set_enabled(true)).unwrap();
        assert!(enabled.enabled);
        assert!(enabled.has_key);
        let view = serde_json::to_string(&enabled).unwrap();
        assert!(!view.contains("synthetic-jev-key"));
        assert_eq!(fixture.inner.draft, draft);
        let cleared = edit_jev(&fixture.inner, JevController::clear_key).unwrap();
        assert!(!cleared.enabled);
        assert!(!cleared.has_key);
        assert_eq!(fixture.inner.draft, draft);
    }

    #[test]
    fn read_only_configuration_blocks_cloud_mutations_and_network_tests() {
        let mut fixture = Fixture::new();
        fixture.inner.load_error = Some("Invalid configuration".into());
        assert!(controller_for(&fixture.inner, false).is_ok());
        assert!(edit_jev(&fixture.inner, |controller| controller
            .save_key("not-written"))
        .is_err());
        assert!(edit_jev(&fixture.inner, |controller| controller.set_enabled(true)).is_err());
        assert!(edit_jev(&fixture.inner, JevController::clear_key).is_err());
        assert!(connection_context(&fixture.inner).is_err());
        assert!(!fixture.root.join("secrets.json").exists());
    }

    #[test]
    fn connection_test_uses_the_explicit_proxy_configuration() {
        let mut fixture = Fixture::new();
        fixture.inner.draft["egress"] = json!({
            "mode": "http", "proxy_url": "http://127.0.0.1:8911"
        });
        let (_, egress) = connection_context(&fixture.inner).unwrap();
        assert_eq!(egress.proxy_url.as_deref(), Some("http://127.0.0.1:8911"));
        fixture.inner.draft["egress"] = json!({"mode": "invalid"});
        assert!(connection_context(&fixture.inner).is_err());
    }

    #[test]
    fn cloud_settings_are_isolated_by_the_desktop_data_directory() {
        let first = Fixture::new();
        let second = Fixture::new();
        let controller = controller_for(&first.inner, false).unwrap();
        edit_jev(&first.inner, |controller| controller.save_key("first-only")).unwrap();
        edit_jev(&first.inner, |controller| controller.set_enabled(true)).unwrap();
        assert!(controller.status().enabled);
        let other = controller_for(&second.inner, false).unwrap().status();
        assert!(!other.enabled);
        assert!(!other.has_key);
    }
}

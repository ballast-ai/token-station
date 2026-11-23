//! Local classifier controls use platform and configuration admission.
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use token_station_cli::semantic::{Mode, SemanticController, Status};

use crate::{AppInner, AppStateManaged};

pub(crate) fn supported_platform() -> bool {
    platform_supported(std::env::consts::OS, std::env::consts::ARCH)
}

fn platform_supported(os: &str, arch: &str) -> bool {
    os == "macos" && arch == "aarch64"
}

fn ensure_access(inner: Option<&AppInner>, supported: bool) -> Result<(), String> {
    if !supported {
        return Err("Local SCX routing requires macOS on Apple Silicon.".into());
    }
    inner
        .ok_or_else(|| "SCX is unavailable while the application is in recovery.".to_owned())?
        .ensure_editable()
}

pub(crate) fn start_controller(inner: &AppInner) -> Option<Arc<SemanticController>> {
    start_controller_for_platform(inner, supported_platform())
}

fn start_controller_for_platform(
    inner: &AppInner,
    supported: bool,
) -> Option<Arc<SemanticController>> {
    ensure_access(Some(inner), supported).ok()?;
    let controller = SemanticController::shared(&inner.data_dir());
    if let Err(error) = controller.start_automatic_route() {
        eprintln!("SCX automatic startup failed: {error}");
    }
    Some(controller)
}

fn controller(app: &AppHandle) -> Result<Arc<SemanticController>, String> {
    let state = app.try_state::<AppStateManaged>();
    let inner = state
        .as_ref()
        .map(|state| state.0.lock().map_err(|_| "Cannot read routing settings."))
        .transpose()?;
    ensure_access(inner.as_deref(), supported_platform())?;
    let controller = app
        .try_state::<Arc<SemanticController>>()
        .map(|state| Arc::clone(state.inner()))
        .ok_or_else(|| "SCX is unavailable while the application is in recovery.".to_owned())?;
    if !Arc::ptr_eq(
        &controller,
        &SemanticController::shared(&inner.expect("access requires configuration").data_dir()),
    ) {
        return Err("Restart the App to apply the changed data directory.".into());
    }
    Ok(controller)
}

#[tauri::command]
pub(crate) fn get_semantic_status(app: AppHandle) -> Status {
    controller(&app).map_or_else(|_| Status::unavailable(), |controller| controller.status())
}

#[tauri::command]
pub(crate) async fn set_semantic_enabled(app: AppHandle, enabled: bool) -> Result<Status, String> {
    let controller = controller(&app)?;
    tauri::async_runtime::spawn_blocking(move || controller.set_enabled(enabled))
        .await
        .map_err(|_| "SCX switch change failed.".to_owned())?
}

#[tauri::command]
pub(crate) async fn set_semantic_mode(app: AppHandle, mode: Mode) -> Result<Status, String> {
    let controller = controller(&app)?;
    tauri::async_runtime::spawn_blocking(move || controller.set_mode(mode))
        .await
        .map_err(|_| "SCX mode change failed.".to_owned())?
}

#[tauri::command]
pub(crate) fn prepare_semantic_model(app: AppHandle) -> Result<Status, String> {
    controller(&app)?.prepare()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppInner;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture {
        root: PathBuf,
        inner: AppInner,
    }

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "token-station-semantic-command-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).unwrap();
            let effective = root.join("custom-data");
            let draft = crate::config_draft::template(&effective, &root.join("plugins"));
            let inner = AppInner::new_with_saved(
                root.join("com.tokenstation.desktop/token-station.json"),
                draft.clone(),
                draft,
                None,
            );
            Self { root, inner }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn local_classification_support_depends_on_the_host_platform() {
        assert!(platform_supported("macos", "aarch64"));
        for (os, arch) in [
            ("macos", "x86_64"),
            ("windows", "aarch64"),
            ("linux", "aarch64"),
            ("linux", "x86_64"),
        ] {
            assert!(!platform_supported(os, arch));
        }
    }

    #[test]
    fn standard_startup_and_gateway_share_the_effective_data_directory() {
        let fixture = Fixture::new();
        let controller = start_controller_for_platform(&fixture.inner, true).unwrap();
        assert!(controller.status().available);
        assert!(!controller.status().enabled);
        let gateway_controller = SemanticController::shared(&fixture.inner.data_dir());
        assert!(Arc::ptr_eq(&controller, &gateway_controller));
        assert!(!Arc::ptr_eq(
            &controller,
            &SemanticController::shared(&fixture.root.join("token-station-data"))
        ));
        controller.set_enabled(false).unwrap();
        assert!(fixture
            .inner
            .data_dir()
            .join("semantic-settings.json")
            .is_file());
        assert!(!fixture.root.join("token-station-data").exists());
        assert!(ensure_access(Some(&fixture.inner), true).is_ok());
    }

    #[test]
    fn unsupported_recovery_and_read_only_states_cannot_start_or_prepare() {
        let mut fixture = Fixture::new();
        assert!(ensure_access(None, true).is_err());
        assert!(ensure_access(Some(&fixture.inner), false).is_err());
        assert!(start_controller_for_platform(&fixture.inner, false).is_none());
        fixture.inner.load_error = Some("Invalid configuration".into());
        assert!(ensure_access(Some(&fixture.inner), true).is_err());
        assert!(start_controller_for_platform(&fixture.inner, true).is_none());
        assert!(!fixture.inner.data_dir().exists());
    }
}

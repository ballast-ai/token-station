//! Experimental commands are unavailable to the ordinary desktop identity.
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use token_station_cli::semantic::{Mode, SemanticController, Status};

fn controller(app: &AppHandle) -> Result<Arc<SemanticController>, String> {
    if !crate::experimental::is_scx_experiment() {
        return Err("SCX controls are only available in the isolated experimental App.".into());
    }
    app.try_state::<Arc<SemanticController>>()
        .map(|state| Arc::clone(state.inner()))
        .ok_or_else(|| "SCX is unavailable while the application is in recovery.".into())
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

//! Local browser search preferences and a manual connection test.
use crate::AppStateManaged;
use tauri::State;
use token_station_cli::search::{SearchController, SearchResponse, SearchSettings, SearchStatus};

#[tauri::command]
pub(crate) fn get_search_status(state: State<'_, AppStateManaged>) -> Result<SearchStatus, String> {
    let inner = state.0.lock().map_err(|_| "Cannot read search settings.")?;
    Ok(SearchController::shared(&inner.data_dir()).status())
}

#[tauri::command]
pub(crate) fn save_search_settings(
    state: State<'_, AppStateManaged>,
    settings: SearchSettings,
) -> Result<SearchStatus, String> {
    let inner = state.0.lock().map_err(|_| "Cannot save search settings.")?;
    inner.ensure_editable()?;
    SearchController::shared(&inner.data_dir()).save(settings)
}

#[tauri::command]
pub(crate) async fn test_browser_search(
    state: State<'_, AppStateManaged>,
    query: String,
) -> Result<SearchResponse, String> {
    let controller = {
        let inner = state.0.lock().map_err(|_| "Cannot read search settings.")?;
        inner.ensure_editable()?;
        SearchController::shared(&inner.data_dir())
    };
    tauri::async_runtime::spawn_blocking(move || controller.search(&query, &|| false))
        .await
        .map_err(|_| "Browser search test failed.".to_owned())?
}

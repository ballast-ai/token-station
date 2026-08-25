use crate::*;

/// Minimal recovery control plane. These commands depend only on application
/// paths and the filesystem; they never require the business metrics DB to
/// open successfully.
#[tauri::command]
pub(crate) fn get_recovery_state(paths: State<'_, DesktopPaths>) -> RecoveryState {
    recovery::inspect_recovery_state(&paths.data_dir)
}

#[tauri::command]
pub(crate) fn get_recovery_diagnostics(
    paths: State<'_, DesktopPaths>,
) -> Result<DiagnosticPreview, String> {
    recovery::diagnostic_preview(&paths.config_file, &paths.data_dir)
}

#[tauri::command]
pub(crate) fn record_frontend_diagnostic(
    paths: State<'_, DesktopPaths>,
    event: FrontendDiagnosticInput,
) -> Result<FrontendDiagnosticRecord, String> {
    recovery::append_frontend_event(&recovery::diagnostic_log_path(&paths.data_dir), event)
}

#[tauri::command]
pub(crate) fn export_recovery_bundle(
    paths: State<'_, DesktopPaths>,
    confirmed: bool,
) -> Result<String, String> {
    recovery::export_bundle(&paths.config_file, &paths.data_dir, confirmed)
        .map(|path| path.display().to_string())
}

#[tauri::command]
pub(crate) fn open_recovery_folder(paths: State<'_, DesktopPaths>) -> Result<String, String> {
    std::fs::create_dir_all(&paths.data_dir)
        .map_err(|error| format!("{}: {error}", paths.data_dir.display()))?;
    tauri_plugin_opener::open_path(&paths.data_dir, None::<&str>)
        .map_err(|error| format!("打开自救目录失败：{error}"))?;
    Ok(paths.data_dir.display().to_string())
}

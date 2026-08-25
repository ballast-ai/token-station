use crate::*;

pub(crate) fn desktop_updater<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<tauri_plugin_updater::Updater, String> {
    let endpoint = official_update_manifest_endpoint()?
        .parse()
        .map_err(|error| format!("更新地址无效：{error}"))?;
    app.updater_builder()
        .pubkey(OFFICIAL_PUBLIC_KEY)
        .endpoints(vec![endpoint])
        .map_err(|error| format!("更新地址配置失败：{error}"))?
        .build()
        .map_err(|error| format!("更新器初始化失败：{error}"))
}

#[cfg(target_os = "windows")]
pub(crate) fn desktop_update_platform_unsupported_message() -> Option<&'static str> {
    Some(desktop_update::WINDOWS_FIRST_RELEASE_UNSUPPORTED_MESSAGE)
}

#[cfg(target_os = "macos")]
pub(crate) fn desktop_update_platform_unsupported_message() -> Option<&'static str> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) fn desktop_update_platform_unsupported_message() -> Option<&'static str> {
    Some(desktop_update::MACOS_ONLY_FIRST_RELEASE_UNSUPPORTED_MESSAGE)
}

/// Check the signed desktop update channel without changing the installed app.
#[tauri::command]
pub(crate) async fn check_desktop_update(
    app: AppHandle,
    operation: State<'_, DesktopUpdateOperation>,
) -> Result<DesktopUpdateView, String> {
    let current = app.package_info().version.to_string();
    let _lease = operation.try_begin()?;
    if let Some(message) = desktop_update_platform_unsupported_message() {
        return Ok(DesktopUpdateView::unsupported(&current, message));
    }

    Ok(
        desktop_update::check_with(&current, OFFICIAL_PUBLIC_KEY, || async {
            let update = desktop_updater(&app)?
                .check()
                .await
                .map_err(|error| format!("暂时无法检查更新，请稍后重试：{error}"))?;
            Ok(update.map(|update| DesktopUpdateCandidate {
                version: update.version.to_string(),
                notes: update.body,
                pub_date: update.date.map(|date| date.to_string()),
            }))
        })
        .await,
    )
}

pub(crate) async fn prepare_gateway_for_desktop_update(
    app: AppHandle,
) -> Result<bool, desktop_update::DesktopUpdatePrepareFailure<bool>> {
    let Some(state) = app.try_state::<AppStateManaged>() else {
        return Ok(false);
    };
    let was_active = {
        let inner = state.0.lock().unwrap();
        matches!(
            inner.server,
            ServerLifecycle::Starting { .. }
                | ServerLifecycle::Applying { .. }
                | ServerLifecycle::Running { .. }
        )
    };
    begin_serve_stop(app.clone(), state.inner());

    let wait_result = tauri::async_runtime::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let stopped = {
                let state = app.state::<AppStateManaged>();
                let inner = state.0.lock().unwrap();
                matches!(
                    inner.server,
                    ServerLifecycle::Stopped { .. } | ServerLifecycle::Failed { .. }
                )
            };
            if stopped {
                return Ok(was_active);
            }
            if Instant::now() >= deadline {
                return Err(desktop_update::DesktopUpdatePrepareFailure::new(
                    "update_gateway_stop_timeout: 等待本地网关安全停止超时",
                    was_active,
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    })
    .await;
    match wait_result {
        Ok(result) => result,
        Err(error) => Err(desktop_update::DesktopUpdatePrepareFailure::new(
            format!("等待本地网关停止任务失败：{error}"),
            was_active,
        )),
    }
}

pub(crate) async fn restore_gateway_after_failed_update(
    app: AppHandle,
    was_active: bool,
) -> Result<(), String> {
    restore_gateway_after_failed_update_with(app, was_active, prepare_server).await
}

pub(crate) async fn restore_gateway_after_failed_update_with<R, F>(
    app: AppHandle<R>,
    was_active: bool,
    prepare: F,
) -> Result<(), String>
where
    R: Runtime,
    F: FnOnce(ClientConfig) -> Result<PreparedServer, StartFailure> + Send + 'static,
{
    if !was_active {
        return Ok(());
    }
    if app.try_state::<AppStateManaged>().is_none() {
        return Ok(());
    }
    let wait_app = app.clone();
    let needs_start = tauri::async_runtime::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let action = {
                let state = wait_app.state::<AppStateManaged>();
                let inner = state.0.lock().unwrap();
                match inner.server {
                    ServerLifecycle::Stopped { .. } | ServerLifecycle::Failed { .. } => Some(true),
                    ServerLifecycle::Starting { .. }
                    | ServerLifecycle::Applying { .. }
                    | ServerLifecycle::Running { .. } => Some(false),
                    ServerLifecycle::Stopping { .. } => None,
                }
            };
            if let Some(needs_start) = action {
                return Ok(needs_start);
            }
            if Instant::now() >= deadline {
                return Err(
                    "update_gateway_restore_timeout: 等待本地网关停止完成后恢复超时".to_owned(),
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    })
    .await
    .map_err(|error| format!("等待本地网关恢复任务失败：{error}"))??;
    if !needs_start {
        return Ok(());
    }
    let state = app.state::<AppStateManaged>();
    let expected_generation = {
        let inner = state.0.lock().unwrap();
        inner
            .server
            .generation()
            .checked_add(1)
            .ok_or_else(|| "代理启动 generation 已耗尽，请重启 App".to_owned())?
    };
    begin_serve_start(app.clone(), state.inner(), prepare)?;

    let wait_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let outcome = {
                let state = wait_app.state::<AppStateManaged>();
                let inner = state.0.lock().unwrap();
                let actual_generation = inner.server.generation();
                if actual_generation != expected_generation {
                    Some(Err(format!(
                        "update_gateway_restore_interrupted: 恢复目标 generation {expected_generation} 已被 {actual_generation} 取代"
                    )))
                } else {
                    match &inner.server {
                        ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. } => None,
                        ServerLifecycle::Running { server, .. } => {
                            if !server.is_task_alive() {
                                Some(Err(
                                    "update_gateway_restore_start_failed: 恢复后的代理任务已退出"
                                        .to_owned(),
                                ))
                            } else if server.listener_reachable() {
                                Some(Ok(()))
                            } else {
                                None
                            }
                        }
                        ServerLifecycle::Failed { error, .. } => Some(Err(format!(
                            "update_gateway_restore_start_failed: {error}"
                        ))),
                        ServerLifecycle::Stopped { .. } | ServerLifecycle::Stopping { .. } => {
                            Some(Err(
                                "update_gateway_restore_interrupted: 恢复启动被停止".to_owned(),
                            ))
                        }
                    }
                }
            };
            if let Some(outcome) = outcome {
                return outcome;
            }
            if Instant::now() >= deadline {
                return Err(
                    "update_gateway_restore_start_timeout: 等待本地网关恢复运行超时".to_owned(),
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    })
    .await
    .map_err(|error| format!("等待本地网关恢复启动任务失败：{error}"))?
}

#[tauri::command]
pub(crate) async fn install_desktop_update_and_restart(
    app: AppHandle,
    operation: State<'_, DesktopUpdateOperation>,
    expected_version: String,
) -> Result<bool, String> {
    let _lease = operation.try_begin()?;
    if let Some(message) = desktop_update_platform_unsupported_message() {
        return Err(message.to_owned());
    }
    if OFFICIAL_PUBLIC_KEY.trim().is_empty() {
        return Err("当前构建没有内置官方更新公钥，不能在 App 内安装更新。".to_owned());
    }

    let updater = desktop_updater(&app)?;
    let progress_app = app.clone();
    let prepare_app = app.clone();
    let recover_app = app.clone();
    let installed = desktop_update::install_with(
        OFFICIAL_PUBLIC_KEY,
        &expected_version,
        || async move {
            updater
                .check()
                .await
                .map_err(|error| format!("暂时无法检查更新，请稍后重试：{error}"))
                .map(|update| update.map(|update| (update.version.to_string(), update)))
        },
        move |update| async move {
            let mut downloaded = 0_u64;
            let bytes = update
                .download(
                    move |chunk_length, total| {
                        downloaded = downloaded.saturating_add(chunk_length as u64);
                        let _ = progress_app
                            .emit(PROGRESS_EVENT, DesktopUpdateProgress { downloaded, total });
                    },
                    || {},
                )
                .await
                .map_err(|error| format!("更新包下载或签名校验失败，当前版本未被替换：{error}"))?;
            Ok((update, bytes))
        },
        move || prepare_gateway_for_desktop_update(prepare_app),
        |update, bytes, _was_active| {
            update
                .install(bytes)
                .map_err(|error| format!("更新安装失败，当前版本未被替换：{error}"))
        },
        move |was_active| restore_gateway_after_failed_update(recover_app, was_active),
    )
    .await?;
    if !installed {
        return Ok(false);
    }

    #[cfg(target_os = "windows")]
    return Ok(true);

    #[cfg(not(target_os = "windows"))]
    app.restart()
}

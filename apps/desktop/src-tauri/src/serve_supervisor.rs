use crate::*;

pub(crate) const SERVE_STATE_CHANGED_EVENT: &str = "serve-state-changed";

pub(crate) fn emit_serve_state<R: Runtime>(app: &AppHandle<R>, view: &ServeView) {
    desktop_shell::update_proxy_menu(app);
    let _ = app.emit(SERVE_STATE_CHANGED_EVENT, view.clone());
}

pub(crate) fn publish_status_menu_start_error<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppStateManaged,
    error: String,
) {
    let view = {
        let mut inner = state.0.lock().unwrap();
        if matches!(
            inner.server,
            ServerLifecycle::Stopped { .. } | ServerLifecycle::Failed { .. }
        ) {
            let generation = inner.server.generation();
            let listen = inner
                .draft
                .pointer("/server/listen")
                .and_then(Value::as_str)
                .unwrap_or("127.0.0.1:8787")
                .to_owned();
            inner.server = ServerLifecycle::Failed {
                generation,
                listen,
                error,
            };
        }
        inner.serve_view()
    };
    emit_serve_state(app, &view);
    desktop_shell::restore_main_window(app);
}

pub(crate) fn desktop_shell_applying_phase(
    task_alive: bool,
    accepting: bool,
) -> desktop_shell::ProxyMenuPhase {
    if task_alive && accepting {
        desktop_shell::ProxyMenuPhase::Applying
    } else {
        desktop_shell::ProxyMenuPhase::Switching
    }
}

pub(crate) fn desktop_shell_snapshot(inner: &AppInner) -> desktop_shell::ProxyMenuSnapshot {
    let generation = inner.server.generation();
    let (phase, listen) = match &inner.server {
        ServerLifecycle::Stopped { .. } => (
            desktop_shell::ProxyMenuPhase::Stopped,
            inner.draft["server"]["listen"]
                .as_str()
                .unwrap_or("127.0.0.1:8787"),
        ),
        ServerLifecycle::Starting { listen, .. } => {
            (desktop_shell::ProxyMenuPhase::Starting, listen.as_str())
        }
        ServerLifecycle::Applying { old, .. } => (
            desktop_shell_applying_phase(old.is_task_alive(), old.is_accepting()),
            old.listen(),
        ),
        ServerLifecycle::Stopping { listen, .. } => {
            (desktop_shell::ProxyMenuPhase::Stopping, listen.as_str())
        }
        ServerLifecycle::Running { server, .. } if server.is_task_alive() => {
            (desktop_shell::ProxyMenuPhase::Running, server.listen())
        }
        ServerLifecycle::Running { server, .. } => {
            (desktop_shell::ProxyMenuPhase::Failed, server.listen())
        }
        ServerLifecycle::Failed { listen, .. } => {
            (desktop_shell::ProxyMenuPhase::Failed, listen.as_str())
        }
    };
    desktop_shell::ProxyMenuSnapshot::new(generation, phase, listen)
}

pub(crate) fn lifecycle_proxy_action(server: &ServerLifecycle) -> desktop_shell::ProxyMenuAction {
    match server {
        ServerLifecycle::Stopped { .. } | ServerLifecycle::Failed { .. } => {
            desktop_shell::ProxyMenuAction::Start
        }
        ServerLifecycle::Running { server, .. } if server.is_task_alive() => {
            desktop_shell::ProxyMenuAction::Stop
        }
        ServerLifecycle::Running { .. } => desktop_shell::ProxyMenuAction::Start,
        ServerLifecycle::Starting { .. }
        | ServerLifecycle::Applying { .. }
        | ServerLifecycle::Stopping { .. } => desktop_shell::ProxyMenuAction::None,
    }
}

pub(crate) fn menu_action_expectation_matches(
    expected_generation: u64,
    current_generation: u64,
    requested: desktop_shell::ProxyMenuAction,
    current: desktop_shell::ProxyMenuAction,
) -> bool {
    expected_generation == current_generation && requested == current
}

pub(crate) fn complete_serve_start<R: Runtime>(
    app: &AppHandle<R>,
    generation: u64,
    result: Result<PreparedServer, StartFailure>,
    applied_pricing: PriceTable,
    metrics_db: PathBuf,
    upstream_epochs: BTreeMap<String, u64>,
) {
    // Same-port handoff must first release the old accept socket. This state
    // mutation is instant; the candidate bind/retry itself happens below,
    // outside the App mutex.
    let resume_listen = result.as_ref().ok().and_then(|prepared| {
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        match &inner.server {
            ServerLifecycle::Applying {
                generation: current,
                old,
                ..
            } if *current == generation && old.listen() == prepared.listen() => {
                old.stop_accepting();
                Some(old.listen().to_owned())
            }
            _ => None,
        }
    });
    if resume_listen.is_some() {
        // Publish the listener handoff immediately. Candidate bind retries can
        // last almost one second, so the periodic refresh alone could leave a
        // stale checked “still running” item visible for the whole outage.
        desktop_shell::update_proxy_menu(app);
    }
    let result = result.and_then(PreparedServer::bind);
    // A failed candidate bind must restore the old listener, but that retry is
    // equally forbidden under the global lock. The reserved socket is only
    // installed if the same generation is still Applying.
    let mut resume_listener = if result.is_err() {
        resume_listen.as_deref().map(PreparedServer::bind_listener)
    } else {
        None
    };
    let mut discard = None;
    let mut retire = None;
    let mut published = false;
    let mut view = {
        let state = app.state::<AppStateManaged>();
        let mut inner = state.0.lock().unwrap();
        let current = std::mem::replace(&mut inner.server, ServerLifecycle::Stopped { generation });
        inner.server = match (current, result) {
            (
                ServerLifecycle::Starting {
                    generation: current,
                    listen,
                    revision,
                },
                Ok(prepared),
            ) if current == generation => match prepared.publish(revision, upstream_epochs.clone())
            {
                Ok(server) => {
                    published = true;
                    ServerLifecycle::Running {
                        generation,
                        server,
                        apply_error: None,
                    }
                }
                Err(failure) => ServerLifecycle::Failed {
                    generation,
                    listen,
                    error: failure.public_message(),
                },
            },
            (
                ServerLifecycle::Applying {
                    generation: current,
                    revision,
                    mut old,
                    ..
                },
                Ok(prepared),
            ) if current == generation => {
                let same_listener = old.listen() == prepared.listen();
                match prepared.publish(revision, upstream_epochs.clone()) {
                    Ok(server) => {
                        published = true;
                        old.stop_accepting();
                        retire = Some(old);
                        ServerLifecycle::Running {
                            generation,
                            server,
                            apply_error: None,
                        }
                    }
                    Err(failure) => {
                        let mut message = failure.public_message();
                        if same_listener {
                            let restore = resume_listener
                                .take()
                                .unwrap_or_else(|| {
                                    Err(StartFailure::new("listen_restore", "旧 listener 未能预留"))
                                })
                                .and_then(|listener| old.resume_accepting(listener));
                            if let Err(restore) = restore {
                                message = format!(
                                    "切换失败且旧 listener 恢复失败：{message}; {}",
                                    restore.public_message()
                                );
                                let listen = old.listen().to_owned();
                                retire = Some(old);
                                ServerLifecycle::Failed {
                                    generation,
                                    listen,
                                    error: message,
                                }
                            } else {
                                ServerLifecycle::Running {
                                    generation,
                                    server: old,
                                    apply_error: Some(format!("已保存尚未应用：{message}")),
                                }
                            }
                        } else {
                            ServerLifecycle::Running {
                                generation,
                                server: old,
                                apply_error: Some(format!("已保存尚未应用：{message}")),
                            }
                        }
                    }
                }
            }
            (
                ServerLifecycle::Starting {
                    generation: current,
                    listen,
                    ..
                },
                Err(failure),
            ) if current == generation => ServerLifecycle::Failed {
                generation,
                listen,
                error: failure.public_message(),
            },
            (
                ServerLifecycle::Applying {
                    generation: current,
                    old,
                    ..
                },
                Err(failure),
            ) if current == generation => ServerLifecycle::Running {
                generation,
                server: old,
                apply_error: Some(format!("已保存尚未应用：{}", failure.public_message())),
            },
            (
                ServerLifecycle::Stopping {
                    generation: current,
                    listen,
                    draining,
                },
                Ok(prepared),
            ) if current == generation => {
                discard = Some(prepared);
                if draining {
                    ServerLifecycle::Stopping {
                        generation,
                        listen,
                        draining,
                    }
                } else {
                    ServerLifecycle::Stopped { generation }
                }
            }
            (
                ServerLifecycle::Stopping {
                    generation: current,
                    listen,
                    draining,
                },
                Err(_),
            ) if current == generation => {
                if draining {
                    ServerLifecycle::Stopping {
                        generation,
                        listen,
                        draining,
                    }
                } else {
                    ServerLifecycle::Stopped { generation }
                }
            }
            (current, Ok(prepared)) => {
                discard = Some(prepared);
                current
            }
            (current, Err(_)) => current,
        };
        Some(inner.serve_view())
    };
    if published {
        let app_state = app.state::<AppStateManaged>();
        let agents = app.state::<AgentCommandState>();
        if let Ok(runtime) = runtime_from_app(app_state.inner()) {
            if let Err(error) = agents.refresh_model_metadata(None, &runtime) {
                let mut inner = app_state.0.lock().unwrap();
                if let ServerLifecycle::Running {
                    generation: current,
                    apply_error,
                    ..
                } = &mut inner.server
                {
                    if *current == generation {
                        *apply_error = Some(format!(
                            "代理已启动，但 Agent 模型元数据刷新失败：{}",
                            error.message
                        ));
                        view = Some(inner.serve_view());
                    }
                }
            }
        }
    }
    if let Some(prepared) = discard {
        prepared.discard();
    }
    if let Some(old) = retire {
        tauri::async_runtime::spawn_blocking(move || {
            old.drain_and_shutdown();
            if let Err(error) = SqliteStore::backfill_unknown_costs(&metrics_db, &applied_pricing) {
                eprintln!("post-apply historical cost backfill failed: {error}");
            }
        });
    }
    if let Some(view) = view {
        emit_serve_state(app, &view);
    }
}

pub(crate) fn begin_serve_start_inner<R, F>(
    app: AppHandle<R>,
    state: &AppStateManaged,
    expected_stopped_generation: Option<u64>,
    prepare: F,
) -> Result<Option<StateView>, String>
where
    R: Runtime,
    F: FnOnce(ClientConfig) -> Result<PreparedServer, StartFailure> + Send + 'static,
{
    let (config, generation, snapshot, serve_view, metrics_db, upstream_epochs) = {
        let mut inner = state.0.lock().unwrap();
        if let Some(expected) = expected_stopped_generation {
            if !menu_action_expectation_matches(
                expected,
                inner.server.generation(),
                desktop_shell::ProxyMenuAction::Start,
                lifecycle_proxy_action(&inner.server),
            ) {
                return Ok(None);
            }
        }
        inner.ensure_editable()?;
        match &inner.server {
            ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. } => {
                return Err("apply_in_progress: 已有配置正在应用".to_owned());
            }
            ServerLifecycle::Stopping { .. } => {
                return Err(
                    "startup_cleanup_in_progress: 上一次代理正在停止，请稍后重试".to_string(),
                );
            }
            ServerLifecycle::Stopped { .. }
            | ServerLifecycle::Running { .. }
            | ServerLifecycle::Failed { .. } => {}
        }
        let config = inner.materialize()?;
        let revision = inner.save_draft()?;
        let metrics_db = inner.data_dir().join("metrics.sqlite");
        let generation = inner
            .server
            .generation()
            .checked_add(1)
            .ok_or_else(|| "代理启动 generation 已耗尽，请重启 App".to_string())?;
        let listen = config.server.listen.clone();
        let current = std::mem::replace(&mut inner.server, ServerLifecycle::Stopped { generation });
        inner.server = match current {
            ServerLifecycle::Running { server: old, .. } => ServerLifecycle::Applying {
                generation,
                revision,
                old,
            },
            _ => ServerLifecycle::Starting {
                generation,
                listen,
                revision,
            },
        };
        let snapshot = inner.snapshot();
        let serve_view = snapshot.serve.clone();
        (
            config,
            generation,
            snapshot,
            serve_view,
            metrics_db,
            inner.upstream_epochs.clone(),
        )
    };

    emit_serve_state(&app, &serve_view);
    let completion_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let applied_pricing = config.pricing.clone();
        let result =
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prepare(config))) {
                Ok(result) => result,
                Err(_) => Err(StartFailure::new("startup_task", "后台启动任务异常退出")),
            };
        complete_serve_start(
            &completion_app,
            generation,
            result,
            applied_pricing,
            metrics_db,
            upstream_epochs,
        );
    });
    Ok(Some(snapshot))
}

pub(crate) fn begin_serve_start<R, F>(
    app: AppHandle<R>,
    state: &AppStateManaged,
    prepare: F,
) -> Result<StateView, String>
where
    R: Runtime,
    F: FnOnce(ClientConfig) -> Result<PreparedServer, StartFailure> + Send + 'static,
{
    begin_serve_start_inner(app, state, None, prepare)
        .map(|snapshot| snapshot.expect("unconditional proxy start cannot be rejected as stale"))
}

pub(crate) fn begin_serve_start_if_generation<R, F>(
    app: AppHandle<R>,
    state: &AppStateManaged,
    expected_generation: u64,
    prepare: F,
) -> Result<bool, String>
where
    R: Runtime,
    F: FnOnce(ClientConfig) -> Result<PreparedServer, StartFailure> + Send + 'static,
{
    begin_serve_start_inner(app, state, Some(expected_generation), prepare)
        .map(|snapshot| snapshot.is_some())
}

#[tauri::command]
pub(crate) fn serve_start(
    app: AppHandle,
    state: State<'_, AppStateManaged>,
) -> Result<StateView, String> {
    begin_serve_start(app, state.inner(), prepare_server)
}

pub(crate) async fn ensure_serve_running_with<R, F>(
    app: AppHandle<R>,
    state: &AppStateManaged,
    prepare: F,
    timeout: Duration,
) -> Result<StateView, String>
where
    R: Runtime,
    F: FnOnce(ClientConfig) -> Result<PreparedServer, StartFailure> + Send + 'static,
{
    enum EnsureAction {
        Ready(Box<StateView>),
        Wait {
            generation: u64,
            fail_on_apply_error: bool,
        },
        Start {
            generation: u64,
        },
    }

    let action = {
        let inner = state.0.lock().unwrap();
        match &inner.server {
            ServerLifecycle::Running { server, .. }
                if server.is_task_alive() && server.listener_reachable() =>
            {
                EnsureAction::Ready(Box::new(inner.snapshot()))
            }
            ServerLifecycle::Running { server, .. } if !server.is_task_alive() => {
                return Err(
                    "ensure_serve_running_start_failed: 代理任务已退出，请先停止后重试".to_owned(),
                );
            }
            ServerLifecycle::Running { generation, .. }
            | ServerLifecycle::Starting { generation, .. } => EnsureAction::Wait {
                generation: *generation,
                fail_on_apply_error: false,
            },
            ServerLifecycle::Applying { generation, .. } => EnsureAction::Wait {
                generation: *generation,
                fail_on_apply_error: true,
            },
            ServerLifecycle::Stopped { generation }
            | ServerLifecycle::Failed { generation, .. } => EnsureAction::Start {
                generation: generation
                    .checked_add(1)
                    .ok_or_else(|| "代理启动 generation 已耗尽，请重启 App".to_owned())?,
            },
            ServerLifecycle::Stopping { .. } => {
                return Err("ensure_serve_running_stopping: 代理正在停止，请稍后重试".to_owned());
            }
        }
    };

    let (expected_generation, fail_on_apply_error) = match action {
        EnsureAction::Ready(view) => return Ok(*view),
        EnsureAction::Wait {
            generation,
            fail_on_apply_error,
        } => (generation, fail_on_apply_error),
        EnsureAction::Start { generation } => {
            if let Err(error) = begin_serve_start(app.clone(), state, prepare) {
                let joined_existing_start = {
                    let inner = state.0.lock().unwrap();
                    inner.server.generation() == generation
                        && matches!(
                            inner.server,
                            ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. }
                        )
                };
                if !joined_existing_start {
                    return Err(error);
                }
            }
            (generation, false)
        }
    };

    let deadline = Instant::now() + timeout;
    loop {
        enum WaitObservation {
            Complete(Box<Result<StateView, String>>),
            Probe(String),
            Pending,
        }

        let observation = {
            let inner = state.0.lock().unwrap();
            let actual_generation = inner.server.generation();
            if actual_generation != expected_generation {
                WaitObservation::Complete(Box::new(Err(format!(
                    "ensure_serve_running_interrupted: 启动目标 generation {expected_generation} 已被 {actual_generation} 取代"
                ))))
            } else {
                match &inner.server {
                    ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. } => {
                        WaitObservation::Pending
                    }
                    ServerLifecycle::Running {
                        server,
                        apply_error,
                        ..
                    } => {
                        if !server.is_task_alive() {
                            WaitObservation::Complete(Box::new(Err(
                                "ensure_serve_running_start_failed: 代理任务已退出".to_owned(),
                            )))
                        } else if fail_on_apply_error && apply_error.is_some() {
                            WaitObservation::Complete(Box::new(Err(format!(
                                "ensure_serve_running_start_failed: {}",
                                apply_error.as_deref().unwrap_or("代理应用失败")
                            ))))
                        } else {
                            WaitObservation::Probe(server.listen().to_owned())
                        }
                    }
                    ServerLifecycle::Failed { error, .. } => WaitObservation::Complete(Box::new(
                        Err(format!("ensure_serve_running_start_failed: {error}")),
                    )),
                    ServerLifecycle::Stopped { .. } => WaitObservation::Complete(Box::new(Err(
                        "ensure_serve_running_interrupted: 代理启动被停止".to_owned(),
                    ))),
                    ServerLifecycle::Stopping { .. } => WaitObservation::Complete(Box::new(Err(
                        "ensure_serve_running_stopping: 代理正在停止，请稍后重试".to_owned(),
                    ))),
                }
            }
        };

        match observation {
            WaitObservation::Complete(outcome) => return *outcome,
            WaitObservation::Probe(listen) => {
                let reachable = match listen.parse::<std::net::SocketAddr>() {
                    Ok(address) => tokio::time::timeout(
                        Duration::from_millis(200),
                        tokio::net::TcpStream::connect(address),
                    )
                    .await
                    .is_ok_and(|result| result.is_ok()),
                    Err(_) => false,
                };
                if reachable {
                    let inner = state.0.lock().unwrap();
                    if inner.server.generation() == expected_generation {
                        if let ServerLifecycle::Running {
                            server,
                            apply_error,
                            ..
                        } = &inner.server
                        {
                            if server.listen() == listen
                                && server.is_task_alive()
                                && (!fail_on_apply_error || apply_error.is_none())
                            {
                                return Ok(inner.snapshot());
                            }
                        }
                    }
                }
            }
            WaitObservation::Pending => {}
        }
        if Instant::now() >= deadline {
            return Err("ensure_serve_running_timeout: 等待代理启动并可达超时".to_owned());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tauri::command]
pub(crate) async fn ensure_serve_running(
    app: AppHandle,
    state: State<'_, AppStateManaged>,
) -> Result<StateView, String> {
    ensure_serve_running_with(app, state.inner(), prepare_server, Duration::from_secs(30)).await
}

pub(crate) fn complete_serve_stop<R: Runtime>(app: &AppHandle<R>, generation: u64) {
    let view = {
        let state = app.state::<AppStateManaged>();
        let mut inner = state.0.lock().unwrap();
        match &inner.server {
            ServerLifecycle::Stopping {
                generation: current,
                ..
            } if *current == generation => {
                inner.server = ServerLifecycle::Stopped { generation };
                Some(inner.serve_view())
            }
            _ => None,
        }
    };
    if let Some(view) = view {
        emit_serve_state(app, &view);
    }
}

pub(crate) fn begin_serve_stop_inner<R: Runtime>(
    app: AppHandle<R>,
    state: &AppStateManaged,
    expected_running_generation: Option<u64>,
) -> Option<StateView> {
    let (generation, snapshot, serve_view, running) = {
        let mut inner = state.0.lock().unwrap();
        if let Some(expected) = expected_running_generation {
            if !menu_action_expectation_matches(
                expected,
                inner.server.generation(),
                desktop_shell::ProxyMenuAction::Stop,
                lifecycle_proxy_action(&inner.server),
            ) {
                return None;
            }
        }
        let generation = inner.server.generation();
        let current = std::mem::replace(&mut inner.server, ServerLifecycle::Stopped { generation });
        let mut running = None;
        let changed = match current {
            ServerLifecycle::Running { server, .. }
            | ServerLifecycle::Applying { old: server, .. } => {
                let listen = server.listen().to_string();
                inner.server = ServerLifecycle::Stopping {
                    generation,
                    listen,
                    draining: true,
                };
                running = Some(server);
                true
            }
            ServerLifecycle::Starting { listen, .. } => {
                inner.server = ServerLifecycle::Stopping {
                    generation,
                    listen,
                    draining: false,
                };
                true
            }
            ServerLifecycle::Stopping {
                listen, draining, ..
            } => {
                inner.server = ServerLifecycle::Stopping {
                    generation,
                    listen,
                    draining,
                };
                false
            }
            ServerLifecycle::Failed { .. } => true,
            ServerLifecycle::Stopped { .. } => false,
        };
        let snapshot = inner.snapshot();
        let serve_view = changed.then(|| snapshot.serve.clone());
        (generation, snapshot, serve_view, running)
    };

    if let Some(serve_view) = serve_view {
        emit_serve_state(&app, &serve_view);
    }
    if let Some(running) = running {
        let completion_app = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ =
                tauri::async_runtime::spawn_blocking(move || running.drain_and_shutdown()).await;
            complete_serve_stop(&completion_app, generation);
        });
    }
    Some(snapshot)
}

pub(crate) fn begin_serve_stop<R: Runtime>(
    app: AppHandle<R>,
    state: &AppStateManaged,
) -> StateView {
    begin_serve_stop_inner(app, state, None)
        .expect("unconditional proxy stop cannot be rejected as stale")
}

pub(crate) fn begin_serve_stop_if_generation<R: Runtime>(
    app: AppHandle<R>,
    state: &AppStateManaged,
    expected_generation: u64,
) -> Option<StateView> {
    begin_serve_stop_inner(app, state, Some(expected_generation))
}

#[tauri::command]
pub(crate) fn serve_stop(app: AppHandle, state: State<'_, AppStateManaged>) -> StateView {
    begin_serve_stop(app, state.inner())
}

//! Token Station desktop backend.
//!
//! This crate does not rewrite routing or gateway logic. It uses
//! `token-station-cli` as a library and reuses the same `Gateway`, `ClientConfig`,
//! `server::serve`, and keychain. The GUI is a panel over that core. Its three
//! routing tiers populate tier_high, tier_mid, and tier_low pools with one
//! provider-model pair each, then heuristic bands select among them.
//!
//! Partially configured tiers are invalid under RouterConfig validation, so the
//! draft remains a serde_json::Value and materializes as ClientConfig only when
//! saving or starting. Failed validation is reported to the user without writing.

pub mod agent_integration;
mod config_state;
mod cursor_tunnel;
mod desktop_shell;
pub mod desktop_update;
mod free_provider_catalog;
mod model_catalog;
mod pricing_catalog;
mod provider_tombstones;
mod recovery;
mod serve_lifecycle;

mod config_draft;
mod desktop_update_commands;
mod dock_icon;
#[cfg(test)]
mod lib_tests;
mod model_test;
mod pricing_commands;
mod provider_commands;
mod provider_discovery;
mod recovery_commands;
mod routing_commands;
mod self_test;
mod serve_supervisor;
mod stats_commands;
mod views;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tauri_plugin_updater::UpdaterExt;
use zeroize::Zeroizing;

use token_station_cli::bodylog::{valid_request_id, BodyLog, PlaintextExchange};
use token_station_cli::budget::{AgentBudget, BudgetStatus};
use token_station_cli::cancel::{CancelReason, CancelToken};
use token_station_cli::config::{
    harness_request_models, ClientConfig, EgressConfig, PluginsConfig,
    RoutingMode as HostRoutingMode, CLAUDE_CODE_FABLE_MODEL_ID,
};
use token_station_cli::gateway::{FeatureLayer, Gateway, HealthLayer, Reply, StageStatus};
use token_station_cli::plugins::{PackageManifest, PluginRegistry, Receipts};
use token_station_cli::pricing::{ModelPrice, PriceTable};
use token_station_cli::request_context::RequestContext;
use token_station_cli::{
    secrets, stats,
    store::{ReceiptQuery, SqliteStore},
    upgrade,
};
use token_station_metrics::ReceiptView;
use token_station_protocol::{CapabilityState, ModelCapability, ProviderApi, ProviderEndpoint};
use token_station_router_core::{UpstreamModel, UpstreamRef};

use agent_integration::commands::{
    apply_agent_plan, apply_snapshot_restore, discard_agent_plan, force_forget_agent,
    get_agent_backup_directory, get_agent_drift, get_cached_agent_views, list_agent_registry,
    list_agent_snapshots, open_agent_backup_directory, plan_agent_connection,
    plan_agent_disconnect, plan_snapshot_restore, runtime_from_app, scan_agents, AgentCommandState,
};
use agent_integration::registry::AgentRegistry;
use agent_integration::types::AdmissionStatus;
use config_state::ConfigState;
use cursor_tunnel::{
    configure_cursor_provider, get_cursor_provider_status, restore_cursor_provider,
    CursorTunnelState,
};
use desktop_update::{
    official_update_manifest_endpoint, DesktopUpdateCandidate, DesktopUpdateOperation,
    DesktopUpdateProgress, DesktopUpdateView, OFFICIAL_PUBLIC_KEY, PROGRESS_EVENT,
};
use model_catalog::ModelDiscoveryView;
use pricing_catalog::{
    ModelPriceSuggestionView, PublicProviderModelsView, RequestedModelPriceSuggestion,
};
use recovery::{
    DiagnosticPreview, FrontendDiagnosticInput, FrontendDiagnosticRecord, RecoveryMode,
    RecoveryState,
};
use serve_lifecycle::{
    home_gateway_identities_match, home_referenced_upstreams, prepare_server, PreparedServer,
    RunningServer, StartFailure,
};

use config_draft::*;
use desktop_update_commands::*;
use dock_icon::*;
use model_test::*;
use pricing_commands::*;
use provider_commands::*;
use provider_discovery::*;
use recovery_commands::*;
use routing_commands::*;
pub use self_test::{run_config_compatibility_check, run_installed_self_test};
use serve_supervisor::*;
use stats_commands::*;
use views::*;

enum ServerLifecycle {
    Stopped {
        generation: u64,
    },
    Starting {
        generation: u64,
        listen: String,
        revision: u64,
    },
    Applying {
        generation: u64,
        revision: u64,
        old: RunningServer,
    },
    Stopping {
        generation: u64,
        listen: String,
        draining: bool,
    },
    Running {
        generation: u64,
        server: RunningServer,
        apply_error: Option<String>,
    },
    Failed {
        generation: u64,
        listen: String,
        error: String,
    },
}

impl ServerLifecycle {
    fn stopped() -> Self {
        Self::Stopped { generation: 0 }
    }

    fn generation(&self) -> u64 {
        match self {
            Self::Stopped { generation }
            | Self::Starting { generation, .. }
            | Self::Applying { generation, .. }
            | Self::Stopping { generation, .. }
            | Self::Running { generation, .. }
            | Self::Failed { generation, .. } => *generation,
        }
    }
}

/// Global backend state protected by one lock; commands are short transactions.
struct AppInner {
    /// Actual token-station.json configuration path.
    config_path: PathBuf,
    /// Authoritative config draft. Materialize and validate candidates before replacing current state.
    draft: Value,
    /// Preserve startup read or validation errors. Show a safe template but block
    /// writes so Save cannot silently overwrite the user's original file.
    load_error: Option<String>,
    /// Persistent identity of the editable saved config; Runtime Supervisor owns the running revision.
    config_state: ConfigState,
    /// In-process editing state for Agent-specific routes. Tiers may be empty but never enter the savable global draft.
    agent_route_drafts: BTreeMap<String, BTreeMap<String, TierView>>,
    /// In-process Harness mapping edits commit with the matching Agent tier draft.
    agent_harness_route_drafts: BTreeMap<String, BTreeMap<String, TierView>>,
    /// Authoritative proxy-service lifecycle state.
    server: ServerLifecycle,
    /// Free-provider verification sends real upstream requests; an in-memory single-flight set limits duplication and abuse.
    pending_free_providers: BTreeSet<String>,
    /// Verified but unsaved provider keys. Clear them on exit to avoid orphaned keys without config references.
    pending_provider_keys: BTreeMap<String, Zeroizing<String>>,
    /// Provider key names to remove only after the draft commits atomically.
    pending_provider_key_removals: BTreeSet<String>,
    /// In-flight model discovery is bounded and single-flight per Provider name.
    pending_provider_discoveries: BTreeSet<String>,
    /// Official provider dialects approved for South at startup or explicit plugin refresh.
    south_approved_dialects: BTreeSet<String>,
    /// Monotonic in-process identities for Provider definitions. Value snapshots
    /// alone cannot detect an A -> B -> A edit while an async operation is in flight.
    upstream_epochs: BTreeMap<String, u64>,
    /// Latest model-discovery operation for each Provider name. Provider
    /// identity can stay unchanged while two network responses finish out of order.
    discovery_generations: BTreeMap<String, u64>,
}

pub struct AppStateManaged(Mutex<AppInner>);

struct FreeProviderValidationGuard<'a> {
    inner: &'a Mutex<AppInner>,
    upstream: String,
}

struct ProviderDiscoveryGuard<'a> {
    inner: &'a Mutex<AppInner>,
    provider: String,
}

impl Drop for FreeProviderValidationGuard<'_> {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        inner.pending_free_providers.remove(&self.upstream);
    }
}

impl Drop for ProviderDiscoveryGuard<'_> {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        inner.pending_provider_discoveries.remove(&self.provider);
        if inner.draft["upstreams"].get(&self.provider).is_none() {
            inner.discovery_generations.remove(&self.provider);
        }
    }
}

/// Writable runtime locations resolved from Tauri's per-application roots.
#[derive(Clone, Debug, PartialEq, Eq)]
struct DesktopPaths {
    config_file: PathBuf,
    data_dir: PathBuf,
    plugins_dir: PathBuf,
    agent_data_root: PathBuf,
}

impl DesktopPaths {
    fn from_app_roots(config_root: PathBuf, data_root: PathBuf) -> Self {
        Self {
            config_file: config_root.join("token-station.json"),
            data_dir: data_root.join("token-station-data"),
            plugins_dir: data_root.join("plugins"),
            agent_data_root: data_root.join("agent-integration"),
        }
    }

    fn create_writable_dirs(&self) -> Result<(), std::io::Error> {
        for path in [
            self.config_file
                .parent()
                .expect("desktop config file always has a parent"),
            self.data_dir.as_path(),
            self.plugins_dir.as_path(),
            self.agent_data_root.as_path(),
        ] {
            crate::agent_integration::safe_fs::ensure_private_dir(path)?;
        }
        let config_root = self
            .config_file
            .parent()
            .expect("desktop config file always has a parent");
        for path in [
            self.config_file.clone(),
            config_root.join("token-station.state.json"),
            self.data_dir.join("virtual-key"),
            self.data_dir.join("plugin-receipts.json"),
            self.data_dir.join("model-catalog-cache.json"),
            self.data_dir.join("provider-tombstones.json"),
            self.data_dir.join("metrics.sqlite"),
            self.data_dir.join("metrics.sqlite-wal"),
            self.data_dir.join("metrics.sqlite-shm"),
            self.data_dir.join("requests.log"),
            self.data_dir.join("requests.log.1"),
            self.data_dir.join("requests.log.2"),
            self.data_dir.join("requests.log.3"),
            self.data_dir.join("diagnostics/frontend.jsonl"),
        ] {
            match std::fs::symlink_metadata(&path) {
                Ok(_) => crate::agent_integration::safe_fs::harden_private_file(&path)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

/// OS application data roots injected by Tauri for Agent snapshots and
/// ownership records.
#[derive(Clone)]
pub struct AgentIntegrationPaths {
    pub snapshot_root: PathBuf,
    pub ownership_root: PathBuf,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            desktop_shell::restore_main_window_after_second_launch(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(desktop_shell::handle_window_event)
        .setup(|app| {
            let desktop_paths = DesktopPaths::from_app_roots(
                app.path().app_config_dir()?,
                app.path().app_data_dir()?,
            );
            desktop_paths.create_writable_dirs().map_err(|error| {
                std::io::Error::other(format!(
                    "初始化桌面应用目录失败（配置：{}，数据：{}，插件：{}）：{error}",
                    desktop_paths.config_file.display(),
                    desktop_paths.data_dir.display(),
                    desktop_paths.plugins_dir.display()
                ))
            })?;

            // The recovery control plane is available before any business
            // state. In safe mode we intentionally do not manage AppState or
            // Agent command state, so normal read/write IPC cannot be invoked
            // behind the recovery shell.
            app.manage(desktop_paths.clone());
            app.manage(DesktopUpdateOperation::default());
            desktop_shell::prepare_close_fallback(app.handle());
            if recovery::inspect_recovery_state(&desktop_paths.data_dir).mode == RecoveryMode::Safe
            {
                desktop_shell::complete_install(desktop_shell::install(
                    app.handle(),
                    desktop_shell::ProxyMenuMode::RecoverySafe,
                    |_app| {
                        desktop_shell::ProxyMenuSnapshot::new(
                            0,
                            desktop_shell::ProxyMenuPhase::Stopped,
                            "不可用（本地数据格式需要更新）",
                        )
                    },
                    |_app| desktop_shell::AgentMenuSnapshot::default(),
                    |_app, _action, _expected_generation| {},
                ))?;
                return Ok(());
            }

            // Reuse existing config after complete CLI validation and defaulting.
            // Damaged config enters read-only protection and is never silently
            // replaced by an empty template. Upgrade legacy single-OpenAI inbound
            // config only in memory.
            let (draft, saved, load_error) = load_draft_state(
                &desktop_paths.config_file,
                &desktop_paths.data_dir,
                &desktop_paths.plugins_dir,
            );
            let mut inner = AppInner::new_with_saved(
                desktop_paths.config_file.clone(),
                draft,
                saved,
                load_error,
            );
            if inner.load_error.is_none() {
                if let Err(error) = recover_pending_provider_purge_on_startup(&mut inner) {
                    inner.load_error = Some(format!(
                        "Provider 永久删除的待清理状态无法安全恢复，已进入只读保护：{error}"
                    ));
                }
            }
            if inner.load_error.is_none()
                && seed_builtin_pricing(&mut inner.draft).map_err(std::io::Error::other)?
            {
                inner.observe_draft().map_err(std::io::Error::other)?;
                inner.save_draft().map_err(std::io::Error::other)?;
            }
            let pricing = draft_price_table(&inner).map_err(std::io::Error::other)?;
            if let Err(error) = SqliteStore::backfill_unknown_costs(
                &desktop_paths.data_dir.join("metrics.sqlite"),
                &pricing,
            ) {
                eprintln!("历史未知成本回填失败：{error}");
            }
            let read_only = inner.load_error.is_some();
            app.manage(AppStateManaged(Mutex::new(inner)));
            app.manage(ModelTestStreamState::default());

            // Agent command state must exist before the native menu is built so
            // its initial snapshot and every later refresh share one authority.
            let paths = AgentIntegrationPaths {
                snapshot_root: desktop_paths.agent_data_root.join("snapshots"),
                ownership_root: desktop_paths.agent_data_root.join("ownership"),
            };
            let agent_commands = AgentCommandState::new(paths.clone()).map_err(|message| {
                std::io::Error::other(format!("初始化 Agent IPC 失败：{message}"))
            })?;
            app.manage(paths);
            app.manage(agent_commands);
            app.manage(CursorTunnelState::default());
            desktop_shell::complete_install(desktop_shell::install(
                app.handle(),
                if read_only {
                    desktop_shell::ProxyMenuMode::ConfigReadOnly
                } else {
                    desktop_shell::ProxyMenuMode::Normal
                },
                |app| {
                    let state = app.state::<AppStateManaged>();
                    let inner = state.0.lock().unwrap();
                    desktop_shell_snapshot(&inner)
                },
                |app| {
                    let Some(agents) = app.try_state::<AgentCommandState>() else {
                        return desktop_shell::AgentMenuSnapshot::default();
                    };
                    desktop_shell::agent_menu_snapshot(
                        agents.managed_agent_menu_entries().into_iter().map(
                            |(agent_id, display_name, order)| {
                                desktop_shell::AgentMenuEntry::new(agent_id, display_name, order)
                            },
                        ),
                    )
                },
                |app, action, expected_generation| {
                    let Some(state) = app.try_state::<AppStateManaged>() else {
                        return;
                    };
                    match action {
                        desktop_shell::ProxyMenuAction::Start => {
                            match begin_serve_start_if_generation(
                                app.clone(),
                                state.inner(),
                                expected_generation,
                                prepare_server,
                            ) {
                                Ok(true) => {}
                                Ok(false) => desktop_shell::update_proxy_menu(app),
                                Err(error) => {
                                    publish_status_menu_start_error(app, state.inner(), error);
                                }
                            }
                        }
                        desktop_shell::ProxyMenuAction::Stop => {
                            if begin_serve_stop_if_generation(
                                app.clone(),
                                state.inner(),
                                expected_generation,
                            )
                            .is_none()
                            {
                                desktop_shell::update_proxy_menu(app);
                            }
                        }
                        desktop_shell::ProxyMenuAction::None => {}
                    }
                },
            ))?;

            // Native state must remain truthful even when the WebView is
            // hidden or its JavaScript timers are throttled. Each request is
            // resolved against the authoritative supervisor only when its
            // main-thread refresh executes.
            let status_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    if status_app.try_state::<AppStateManaged>().is_none() {
                        break;
                    }
                    desktop_shell::update_proxy_menu(&status_app);
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            set_dock_theme_icon,
            get_state,
            get_runtime_state,
            preview_provider_endpoints,
            list_free_provider_presets,
            add_free_provider,
            add_provider,
            add_provider_with_credential,
            add_managed_enterprise_route,
            set_local_routing,
            set_routing_mode,
            set_direct_route,
            set_quota_accounts,
            set_quota_plan,
            get_quota_snapshot,
            edit_provider,
            edit_provider_with_credential,
            discover_provider_models,
            verify_enterprise_route,
            test_provider,
            test_model_chat_stream,
            cancel_model_test_chat,
            set_provider_model_vision,
            set_provider_model_limits,
            update_provider_models,
            preview_provider_removal,
            remove_provider,
            restore_provider,
            purge_deleted_providers,
            set_tier,
            add_keyword,
            remove_keyword,
            set_agent_route_mode,
            set_agent_tier,
            set_agent_harness_model_route,
            save_home_route_as_profile,
            mount_agent_profile,
            delete_profile,
            save_config,
            save_agent_routes,
            restart_agent_route,
            restart_agent_harness_routes,
            apply_home_route_to_all_agents,
            serve_start,
            ensure_serve_running,
            serve_stop,
            list_agent_registry,
            scan_agents,
            get_cached_agent_views,
            get_agent_backup_directory,
            open_agent_backup_directory,
            plan_agent_connection,
            get_cursor_provider_status,
            configure_cursor_provider,
            restore_cursor_provider,
            apply_agent_plan,
            discard_agent_plan,
            plan_agent_disconnect,
            force_forget_agent,
            list_agent_snapshots,
            get_agent_drift,
            plan_snapshot_restore,
            apply_snapshot_restore,
            set_settings,
            get_egress,
            get_stats,
            get_agent_budgets,
            set_agent_budget,
            remove_agent_budget,
            get_price_table,
            list_public_provider_models,
            suggest_model_price,
            import_model_prices_for_provider,
            set_model_price,
            remove_model_price,
            get_recent_receipts,
            get_request_receipts,
            get_router_table,
            get_plugins,
            check_desktop_update,
            install_desktop_update_and_restart,
            get_recovery_state,
            get_recovery_diagnostics,
            record_frontend_diagnostic,
            export_recovery_bundle,
            open_recovery_folder,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    app.run(|app, event| {
        #[cfg(not(target_os = "macos"))]
        let _ = (app, &event);
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen {
            has_visible_windows,
            ..
        } = event
        {
            if !has_visible_windows {
                desktop_shell::restore_main_window(app);
            }
        }
    });
}

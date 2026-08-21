//! Native desktop shell policy for background residency and the status menu.

// The native status menu is installed only on macOS. Linux CI still compiles
// this module so its platform-neutral state policy remains unit tested and the
// cross-platform refresh call can stay a harmless no-op without cfg sprawl.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Emitter, Manager, Runtime, Window, WindowEvent};

use std::collections::HashSet;
use std::sync::Mutex;

pub(crate) const MAIN_WINDOW_LABEL: &str = "main";
pub(crate) const MENU_OPEN_ID: &str = "desktop-shell-open";
pub(crate) const MENU_PROXY_ID: &str = "desktop-shell-proxy";
pub(crate) const MENU_QUIT_ID: &str = "desktop-shell-quit";
pub(crate) const MENU_MANAGE_AGENTS_ID: &str = "desktop-shell-manage-agents";
pub(crate) const MENU_ADD_PROVIDER_ID: &str = "desktop-shell-add-provider";
pub(crate) const MENU_REQUEST_LOGS_ID: &str = "desktop-shell-request-logs";
pub(crate) const MENU_SETTINGS_ID: &str = "desktop-shell-settings";

const MENU_AGENT_PREFIX: &str = "desktop-shell-agent:";
const STATUS_MENU_NAVIGATE_EVENT: &str = "status-menu-navigate";

const TRAY_ICON_ID: &str = "token-station-status";
const TRAY_ICON_PNG: &[u8] = include_bytes!("../icons/tray-template.png");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProxyMenuPhase {
    Stopped,
    Starting,
    Applying,
    Switching,
    Running,
    Stopping,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProxyMenuSnapshot {
    pub(crate) generation: u64,
    pub(crate) phase: ProxyMenuPhase,
    pub(crate) listen: String,
}

impl ProxyMenuSnapshot {
    pub(crate) fn new(generation: u64, phase: ProxyMenuPhase, listen: impl Into<String>) -> Self {
        Self {
            generation,
            phase,
            listen: listen.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentMenuEntry {
    pub(crate) agent_id: String,
    pub(crate) display_name: String,
    pub(crate) order: u16,
}

impl AgentMenuEntry {
    pub(crate) fn new(
        agent_id: impl Into<String>,
        display_name: impl Into<String>,
        order: u16,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            display_name: display_name.into(),
            order,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AgentMenuSnapshot {
    pub(crate) entries: Vec<AgentMenuEntry>,
}

impl AgentMenuSnapshot {
    #[cfg(test)]
    pub(crate) fn new(entries: impl IntoIterator<Item = AgentMenuEntry>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
        }
    }
}

fn safe_agent_menu_id(agent_id: &str) -> bool {
    !agent_id.is_empty()
        && agent_id.len() <= 64
        && agent_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(crate) fn agent_menu_snapshot(
    entries: impl IntoIterator<Item = AgentMenuEntry>,
) -> AgentMenuSnapshot {
    let mut entries: Vec<_> = entries
        .into_iter()
        .filter(|entry| {
            safe_agent_menu_id(&entry.agent_id) && !entry.display_name.trim().is_empty()
        })
        .collect();
    entries.sort_by(|left, right| {
        left.order
            .cmp(&right.order)
            .then_with(|| left.agent_id.cmp(&right.agent_id))
    });
    let mut seen = HashSet::new();
    entries.retain(|entry| seen.insert(entry.agent_id.clone()));
    AgentMenuSnapshot { entries }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProxyMenuMode {
    Normal,
    ConfigReadOnly,
    RecoverySafe,
}

impl ProxyMenuMode {
    const fn is_read_only(self) -> bool {
        !matches!(self, Self::Normal)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProxyMenuView {
    pub(crate) status: String,
    pub(crate) address: String,
    pub(crate) control: String,
    pub(crate) control_checked: bool,
    pub(crate) control_enabled: bool,
}

impl ProxyMenuView {
    fn tooltip(&self) -> String {
        format!("Token Station · {}", self.status)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowRequest {
    Close,
    Reopen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProxyMenuAction {
    None,
    Start,
    Stop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DesktopShellCommand {
    Ignore,
    Open,
    Navigate(String),
    Proxy(ProxyMenuAction),
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WindowResponse {
    pub(crate) prevent_close: bool,
    pub(crate) hide: bool,
    pub(crate) show: bool,
    pub(crate) unminimize: bool,
    pub(crate) focus: bool,
}

pub(crate) fn window_response(request: WindowRequest) -> WindowResponse {
    match request {
        WindowRequest::Close => WindowResponse {
            prevent_close: true,
            hide: true,
            show: false,
            unminimize: false,
            focus: false,
        },
        WindowRequest::Reopen => WindowResponse {
            prevent_close: false,
            hide: false,
            show: true,
            unminimize: true,
            focus: true,
        },
    }
}

pub(crate) fn proxy_menu_action(phase: ProxyMenuPhase, read_only: bool) -> ProxyMenuAction {
    if read_only {
        return ProxyMenuAction::None;
    }
    match phase {
        ProxyMenuPhase::Stopped | ProxyMenuPhase::Failed => ProxyMenuAction::Start,
        ProxyMenuPhase::Running => ProxyMenuAction::Stop,
        ProxyMenuPhase::Starting
        | ProxyMenuPhase::Applying
        | ProxyMenuPhase::Switching
        | ProxyMenuPhase::Stopping => ProxyMenuAction::None,
    }
}

fn proxy_menu_checked(phase: ProxyMenuPhase, read_only: bool) -> bool {
    !read_only
        && matches!(
            phase,
            ProxyMenuPhase::Applying | ProxyMenuPhase::Running | ProxyMenuPhase::Stopping
        )
}

#[cfg(test)]
pub(crate) fn menu_command(
    menu_id: &str,
    phase: ProxyMenuPhase,
    read_only: bool,
) -> DesktopShellCommand {
    menu_command_with_agents(menu_id, phase, read_only, &AgentMenuSnapshot::default())
}

pub(crate) fn menu_command_with_agents(
    menu_id: &str,
    phase: ProxyMenuPhase,
    read_only: bool,
    agents: &AgentMenuSnapshot,
) -> DesktopShellCommand {
    match menu_id {
        MENU_OPEN_ID => DesktopShellCommand::Open,
        MENU_MANAGE_AGENTS_ID => DesktopShellCommand::Navigate("home".to_owned()),
        MENU_ADD_PROVIDER_ID => DesktopShellCommand::Navigate("add-provider".to_owned()),
        MENU_REQUEST_LOGS_ID => DesktopShellCommand::Navigate("logs".to_owned()),
        MENU_SETTINGS_ID => DesktopShellCommand::Navigate("settings".to_owned()),
        MENU_PROXY_ID => match proxy_menu_action(phase, read_only) {
            ProxyMenuAction::None => DesktopShellCommand::Ignore,
            action => DesktopShellCommand::Proxy(action),
        },
        MENU_QUIT_ID => DesktopShellCommand::Quit,
        _ => menu_id
            .strip_prefix(MENU_AGENT_PREFIX)
            .filter(|agent_id| {
                safe_agent_menu_id(agent_id)
                    && agents
                        .entries
                        .iter()
                        .any(|entry| entry.agent_id == *agent_id)
            })
            .map(|agent_id| DesktopShellCommand::Navigate(format!("agent:{agent_id}")))
            .unwrap_or(DesktopShellCommand::Ignore),
    }
}

pub(crate) fn proxy_menu_view(
    phase: ProxyMenuPhase,
    listen: &str,
    mode: ProxyMenuMode,
) -> ProxyMenuView {
    let (status, control, phase_enabled) = match phase {
        ProxyMenuPhase::Stopped => ("代理已停止", "运行本地代理", true),
        ProxyMenuPhase::Starting => ("代理正在启动…", "正在启动本地代理…", false),
        ProxyMenuPhase::Applying => (
            "正在更新代理 · 当前代理仍在运行",
            "正在更新本地代理…",
            false,
        ),
        ProxyMenuPhase::Switching => ("正在切换代理 · 暂时不可用", "正在切换本地代理…", false),
        ProxyMenuPhase::Running => ("代理运行中", "运行本地代理", true),
        ProxyMenuPhase::Stopping => ("代理正在停止…", "正在停止本地代理…", false),
        ProxyMenuPhase::Failed => ("代理启动失败", "重试启动本地代理", true),
    };
    let status = match mode {
        ProxyMenuMode::Normal => status.to_owned(),
        ProxyMenuMode::ConfigReadOnly => format!("只读保护 · {status}"),
        ProxyMenuMode::RecoverySafe => format!("安全模式 · {status}"),
    };
    let address = match mode {
        ProxyMenuMode::Normal => format!("地址 · {listen}"),
        ProxyMenuMode::ConfigReadOnly => "地址不可用 · 配置损坏".to_owned(),
        ProxyMenuMode::RecoverySafe => "地址不可用 · 安全模式".to_owned(),
    };
    let control = match mode {
        ProxyMenuMode::Normal => control.to_owned(),
        ProxyMenuMode::ConfigReadOnly => "本地代理不可用 · 只读保护".to_owned(),
        ProxyMenuMode::RecoverySafe => "本地代理不可用 · 安全模式".to_owned(),
    };
    ProxyMenuView {
        status,
        address,
        control,
        control_checked: proxy_menu_checked(phase, mode.is_read_only()),
        control_enabled: phase_enabled && !mode.is_read_only(),
    }
}

type ProxyStateReader<R> = dyn Fn(&AppHandle<R>) -> ProxyMenuSnapshot + Send + Sync + 'static;
type AgentStateReader<R> = dyn Fn(&AppHandle<R>) -> AgentMenuSnapshot + Send + Sync + 'static;

struct DesktopShellState<R: Runtime> {
    tray: TrayIcon<R>,
    address_item: MenuItem<R>,
    proxy_item: CheckMenuItem<R>,
    agent_menu: Submenu<R>,
    agent_snapshot: Mutex<AgentMenuSnapshot>,
    mode: ProxyMenuMode,
    read_proxy_state: Box<ProxyStateReader<R>>,
    read_agent_state: Box<AgentStateReader<R>>,
}

/// Installs the single native status-menu surface. The callback receives only
/// validated proxy actions; state changes still run through the app's existing
/// lifecycle coordinator in the caller.
pub(crate) fn install<R, S, A, F>(
    app: &AppHandle<R>,
    mode: ProxyMenuMode,
    read_proxy_state: S,
    read_agent_state: A,
    on_proxy_action: F,
) -> tauri::Result<()>
where
    R: Runtime,
    S: Fn(&AppHandle<R>) -> ProxyMenuSnapshot + Send + Sync + 'static,
    A: Fn(&AppHandle<R>) -> AgentMenuSnapshot + Send + Sync + 'static,
    F: Fn(&AppHandle<R>, ProxyMenuAction, u64) + Send + Sync + 'static,
{
    let initial = read_proxy_state(app);
    let initial_agents = agent_menu_snapshot(read_agent_state(app).entries);
    let view = proxy_menu_view(initial.phase, &initial.listen, mode);
    let address_item = MenuItem::with_id(
        app,
        "desktop-shell-address",
        &view.address,
        false,
        None::<&str>,
    )?;
    let open_item = MenuItem::with_id(app, MENU_OPEN_ID, "打开 Token Station", true, None::<&str>)?;
    let proxy_item = CheckMenuItem::with_id(
        app,
        MENU_PROXY_ID,
        &view.control,
        view.control_enabled,
        view.control_checked,
        None::<&str>,
    )?;
    let agent_menu = Submenu::new(
        app,
        format!("已接入 Agent · {}", initial_agents.entries.len()),
        true,
    )?;
    populate_agent_menu(app, &agent_menu, &initial_agents)?;
    let quick_actions = Submenu::new(app, "快捷操作", true)?;
    let add_provider_item =
        MenuItem::with_id(app, MENU_ADD_PROVIDER_ID, "添加供应商…", true, None::<&str>)?;
    let request_logs_item = MenuItem::with_id(
        app,
        MENU_REQUEST_LOGS_ID,
        "查看请求日志",
        true,
        None::<&str>,
    )?;
    let settings_item = MenuItem::with_id(app, MENU_SETTINGS_ID, "打开设置", true, None::<&str>)?;
    quick_actions.append_items(&[&add_provider_item, &request_logs_item, &settings_item])?;
    let separator_before_agents = PredefinedMenuItem::separator(app)?;
    let separator_before_proxy = PredefinedMenuItem::separator(app)?;
    let separator_before_actions = PredefinedMenuItem::separator(app)?;
    let separator_before_quit = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, MENU_QUIT_ID, "退出 Token Station", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open_item,
            &separator_before_agents,
            &agent_menu,
            &separator_before_proxy,
            &proxy_item,
            &address_item,
            &separator_before_actions,
            &quick_actions,
            &separator_before_quit,
            &quit_item,
        ],
    )?;

    let tray = TrayIconBuilder::with_id(TRAY_ICON_ID)
        .icon(Image::from_bytes(TRAY_ICON_PNG)?)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip(view.tooltip())
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| {
            let (snapshot, agents, read_only) = {
                let Some(shell) = app.try_state::<DesktopShellState<R>>() else {
                    return;
                };
                let snapshot = if event.id().as_ref() == MENU_PROXY_ID {
                    // macOS toggles a native check item before dispatching its
                    // event. Re-read and render the authoritative backend state on
                    // this same main-thread callback before deciding the command,
                    // so the optimistic check never becomes an executable fact.
                    match refresh_proxy_menu(app, &shell) {
                        Ok(snapshot) => snapshot,
                        Err(error) => {
                            eprintln!("status menu action refresh failed: {error}");
                            let _ = shell.proxy_item.set_enabled(false);
                            return;
                        }
                    }
                } else {
                    initial.clone()
                };
                let agents = shell
                    .agent_snapshot
                    .lock()
                    .map(|snapshot| snapshot.clone())
                    .unwrap_or_default();
                (snapshot, agents, shell.mode.is_read_only())
            };
            match menu_command_with_agents(event.id().as_ref(), snapshot.phase, read_only, &agents)
            {
                DesktopShellCommand::Ignore => {}
                DesktopShellCommand::Open => restore_main_window(app),
                DesktopShellCommand::Navigate(destination) => {
                    restore_main_window(app);
                    let _ = app.emit(STATUS_MENU_NAVIGATE_EVENT, destination);
                }
                DesktopShellCommand::Proxy(action) => {
                    on_proxy_action(app, action, snapshot.generation)
                }
                // A direct process exit does not emit CloseRequested, so the
                // close-to-hide policy cannot turn an explicit quit into hide.
                DesktopShellCommand::Quit => app.exit(0),
            }
        })
        .build(app)?;

    app.manage(DesktopShellState {
        tray,
        address_item,
        proxy_item,
        agent_menu,
        agent_snapshot: Mutex::new(initial_agents),
        mode,
        read_proxy_state: Box::new(read_proxy_state),
        read_agent_state: Box::new(read_agent_state),
    });
    Ok(())
}

/// Requests a native-menu refresh. The queued main-thread callback reads the
/// authoritative backend state at execution time, so concurrent lifecycle
/// notifications cannot publish an older snapshot after a newer one.
pub(crate) fn update_proxy_menu<R: Runtime>(app: &AppHandle<R>) {
    let menu_app = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        let Some(shell) = menu_app.try_state::<DesktopShellState<R>>() else {
            return;
        };
        if let Err(error) = refresh_proxy_menu(&menu_app, &shell) {
            eprintln!("status menu update failed: {error}");
            let _ = shell.proxy_item.set_enabled(false);
        }
    }) {
        eprintln!("status menu refresh scheduling failed: {error}");
    }
}

/// Refreshes the durable Agent ownership projection after a scan or ownership
/// transaction. It is deliberately separate from the one-second proxy poll so
/// the status menu does not reread ownership files while nothing has changed.
pub(crate) fn update_agent_menu<R: Runtime>(app: &AppHandle<R>) {
    let menu_app = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        let Some(shell) = menu_app.try_state::<DesktopShellState<R>>() else {
            return;
        };
        if let Err(error) = refresh_agent_menu(&menu_app, &shell) {
            eprintln!("status menu Agent update failed: {error}");
        }
    }) {
        eprintln!("status menu Agent refresh scheduling failed: {error}");
    }
}

fn populate_agent_menu<R: Runtime>(
    app: &AppHandle<R>,
    menu: &Submenu<R>,
    snapshot: &AgentMenuSnapshot,
) -> tauri::Result<()> {
    if snapshot.entries.is_empty() {
        let empty = MenuItem::with_id(
            app,
            "desktop-shell-agent-empty",
            "暂无已接入 Agent",
            false,
            None::<&str>,
        )?;
        menu.append(&empty)?;
    } else {
        for entry in &snapshot.entries {
            let item = MenuItem::with_id(
                app,
                format!("{MENU_AGENT_PREFIX}{}", entry.agent_id),
                &entry.display_name,
                true,
                None::<&str>,
            )?;
            menu.append(&item)?;
        }
    }
    let separator = PredefinedMenuItem::separator(app)?;
    let manage = MenuItem::with_id(
        app,
        MENU_MANAGE_AGENTS_ID,
        "管理 Agent…",
        true,
        None::<&str>,
    )?;
    menu.append_items(&[&separator, &manage])
}

fn refresh_agent_menu<R: Runtime>(
    app: &AppHandle<R>,
    shell: &DesktopShellState<R>,
) -> Result<(), String> {
    let next = agent_menu_snapshot((shell.read_agent_state)(app).entries);
    let mut current = shell
        .agent_snapshot
        .lock()
        .map_err(|_| "Agent menu snapshot lock poisoned".to_owned())?;
    if *current == next {
        return Ok(());
    }
    for item in shell
        .agent_menu
        .items()
        .map_err(|error| error.to_string())?
    {
        shell
            .agent_menu
            .remove(&item)
            .map_err(|error| error.to_string())?;
    }
    populate_agent_menu(app, &shell.agent_menu, &next).map_err(|error| error.to_string())?;
    shell
        .agent_menu
        .set_text(format!("已接入 Agent · {}", next.entries.len()))
        .map_err(|error| error.to_string())?;
    *current = next;
    Ok(())
}

fn refresh_proxy_menu<R: Runtime>(
    app: &AppHandle<R>,
    shell: &DesktopShellState<R>,
) -> Result<ProxyMenuSnapshot, String> {
    let snapshot = (shell.read_proxy_state)(app);
    let view = proxy_menu_view(snapshot.phase, &snapshot.listen, shell.mode);
    apply_proxy_menu_view(shell, &view)?;
    Ok(snapshot)
}

fn apply_proxy_menu_view<R: Runtime>(
    shell: &DesktopShellState<R>,
    view: &ProxyMenuView,
) -> Result<(), String> {
    shell
        .address_item
        .set_text(&view.address)
        .map_err(|error| error.to_string())?;
    shell
        .proxy_item
        .set_text(&view.control)
        .map_err(|error| error.to_string())?;
    shell
        .proxy_item
        .set_checked(view.control_checked)
        .map_err(|error| error.to_string())?;
    shell
        .proxy_item
        .set_enabled(view.control_enabled)
        .map_err(|error| error.to_string())?;
    shell
        .tray
        .set_tooltip(Some(view.tooltip()))
        .map_err(|error| error.to_string())
}

pub(crate) fn handle_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    #[cfg(not(target_os = "macos"))]
    let _ = (window, event);
    #[cfg(target_os = "macos")]
    {
        if window.label() != MAIN_WINDOW_LABEL {
            return;
        }
        if let WindowEvent::CloseRequested { api, .. } = event {
            let response = window_response(WindowRequest::Close);
            if response.prevent_close {
                api.prevent_close();
            }
            if response.hide {
                let _ = window.hide();
            }
        }
    }
}

/// Restores the existing main window for both the status menu and Dock reopen.
pub(crate) fn restore_main_window<R: Runtime>(app: &AppHandle<R>) {
    let response = window_response(WindowRequest::Reopen);
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return;
    };
    if response.show {
        let _ = window.show();
    }
    if response.unminimize {
        let _ = window.unminimize();
    }
    if response.focus {
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_menu_template_uses_the_station_bar_mark() {
        let image = Image::from_bytes(TRAY_ICON_PNG).expect("status menu icon must decode");
        assert_eq!((image.width(), image.height()), (36, 36));

        let rgba = image.rgba();
        let pixel_at = |x: u32, y: u32| {
            let offset = ((y * image.width() + x) * 4) as usize;
            &rgba[offset..offset + 4]
        };
        for (x, y) in [(0, 0), (35, 0), (0, 35), (35, 35)] {
            assert_eq!(
                pixel_at(x, y)[3],
                0,
                "status menu icon corner must be transparent"
            );
        }

        let center_run = (0..image.width())
            .filter(|&x| pixel_at(x, 18)[3] > 200)
            .count();
        assert!(
            center_run >= 24,
            "status menu icon must contain the wide station bar"
        );

        for pixel in rgba.as_chunks::<4>().0.iter().filter(|pixel| pixel[3] > 0) {
            assert_eq!(
                pixel[0], pixel[1],
                "template red and green channels must match"
            );
            assert_eq!(
                pixel[1], pixel[2],
                "template green and blue channels must match"
            );
        }
    }

    #[test]
    fn proxy_menu_exposes_a_native_toggle_and_compact_address_for_every_phase() {
        let cases = [
            (
                ProxyMenuPhase::Stopped,
                ProxyMenuView {
                    status: "代理已停止".to_owned(),
                    address: "地址 · 127.0.0.1:8787".to_owned(),
                    control: "运行本地代理".to_owned(),
                    control_checked: false,
                    control_enabled: true,
                },
            ),
            (
                ProxyMenuPhase::Starting,
                ProxyMenuView {
                    status: "代理正在启动…".to_owned(),
                    address: "地址 · 127.0.0.1:8787".to_owned(),
                    control: "正在启动本地代理…".to_owned(),
                    control_checked: false,
                    control_enabled: false,
                },
            ),
            (
                ProxyMenuPhase::Applying,
                ProxyMenuView {
                    status: "正在更新代理 · 当前代理仍在运行".to_owned(),
                    address: "地址 · 127.0.0.1:8787".to_owned(),
                    control: "正在更新本地代理…".to_owned(),
                    control_checked: true,
                    control_enabled: false,
                },
            ),
            (
                ProxyMenuPhase::Switching,
                ProxyMenuView {
                    status: "正在切换代理 · 暂时不可用".to_owned(),
                    address: "地址 · 127.0.0.1:8787".to_owned(),
                    control: "正在切换本地代理…".to_owned(),
                    control_checked: false,
                    control_enabled: false,
                },
            ),
            (
                ProxyMenuPhase::Running,
                ProxyMenuView {
                    status: "代理运行中".to_owned(),
                    address: "地址 · 127.0.0.1:8787".to_owned(),
                    control: "运行本地代理".to_owned(),
                    control_checked: true,
                    control_enabled: true,
                },
            ),
            (
                ProxyMenuPhase::Stopping,
                ProxyMenuView {
                    status: "代理正在停止…".to_owned(),
                    address: "地址 · 127.0.0.1:8787".to_owned(),
                    control: "正在停止本地代理…".to_owned(),
                    control_checked: true,
                    control_enabled: false,
                },
            ),
            (
                ProxyMenuPhase::Failed,
                ProxyMenuView {
                    status: "代理启动失败".to_owned(),
                    address: "地址 · 127.0.0.1:8787".to_owned(),
                    control: "重试启动本地代理".to_owned(),
                    control_checked: false,
                    control_enabled: true,
                },
            ),
        ];

        for (phase, expected) in cases {
            assert_eq!(
                proxy_menu_view(phase, "127.0.0.1:8787", ProxyMenuMode::Normal),
                expected
            );
        }
    }

    #[test]
    fn recovery_mode_disables_proxy_control_without_hiding_runtime_facts() {
        assert_eq!(
            proxy_menu_view(
                ProxyMenuPhase::Stopped,
                "ignored in recovery safe mode",
                ProxyMenuMode::RecoverySafe,
            ),
            ProxyMenuView {
                status: "安全模式 · 代理已停止".to_owned(),
                address: "地址不可用 · 安全模式".to_owned(),
                control: "本地代理不可用 · 安全模式".to_owned(),
                control_checked: false,
                control_enabled: false,
            }
        );
    }

    #[test]
    fn damaged_config_read_only_never_presents_the_display_template_as_a_real_listener() {
        assert_eq!(
            proxy_menu_view(
                ProxyMenuPhase::Stopped,
                "127.0.0.1:8787",
                ProxyMenuMode::ConfigReadOnly,
            ),
            ProxyMenuView {
                status: "只读保护 · 代理已停止".to_owned(),
                address: "地址不可用 · 配置损坏".to_owned(),
                control: "本地代理不可用 · 只读保护".to_owned(),
                control_checked: false,
                control_enabled: false,
            }
        );
    }

    #[test]
    fn protected_modes_never_expose_an_actionable_or_checked_proxy_control() {
        for mode in [ProxyMenuMode::ConfigReadOnly, ProxyMenuMode::RecoverySafe] {
            for phase in [
                ProxyMenuPhase::Stopped,
                ProxyMenuPhase::Starting,
                ProxyMenuPhase::Applying,
                ProxyMenuPhase::Switching,
                ProxyMenuPhase::Running,
                ProxyMenuPhase::Stopping,
                ProxyMenuPhase::Failed,
            ] {
                let view = proxy_menu_view(phase, "must not be shown", mode);
                assert!(!view.control_checked, "mode={mode:?}, phase={phase:?}");
                assert!(!view.control_enabled, "mode={mode:?}, phase={phase:?}");
                assert!(!view.address.contains("must not be shown"));
            }
        }
    }

    #[test]
    fn closing_hides_while_dock_and_menu_reopen_share_one_restore_policy() {
        assert_eq!(
            window_response(WindowRequest::Close),
            WindowResponse {
                prevent_close: true,
                hide: true,
                show: false,
                unminimize: false,
                focus: false,
            }
        );
        assert_eq!(
            window_response(WindowRequest::Reopen),
            WindowResponse {
                prevent_close: false,
                hide: false,
                show: true,
                unminimize: true,
                focus: true,
            }
        );
    }

    #[test]
    fn proxy_control_dispatches_only_stable_writable_states() {
        assert_eq!(
            proxy_menu_action(ProxyMenuPhase::Stopped, false),
            ProxyMenuAction::Start
        );
        assert_eq!(
            proxy_menu_action(ProxyMenuPhase::Failed, false),
            ProxyMenuAction::Start
        );
        assert_eq!(
            proxy_menu_action(ProxyMenuPhase::Running, false),
            ProxyMenuAction::Stop
        );
        for phase in [
            ProxyMenuPhase::Starting,
            ProxyMenuPhase::Applying,
            ProxyMenuPhase::Switching,
            ProxyMenuPhase::Stopping,
        ] {
            assert_eq!(proxy_menu_action(phase, false), ProxyMenuAction::None);
        }
        for phase in [
            ProxyMenuPhase::Stopped,
            ProxyMenuPhase::Starting,
            ProxyMenuPhase::Applying,
            ProxyMenuPhase::Switching,
            ProxyMenuPhase::Running,
            ProxyMenuPhase::Stopping,
            ProxyMenuPhase::Failed,
        ] {
            assert_eq!(proxy_menu_action(phase, true), ProxyMenuAction::None);
        }
    }

    #[test]
    fn menu_commands_keep_open_proxy_control_and_explicit_quit_distinct() {
        assert_eq!(
            menu_command(MENU_OPEN_ID, ProxyMenuPhase::Running, false),
            DesktopShellCommand::Open
        );
        assert_eq!(
            menu_command(MENU_PROXY_ID, ProxyMenuPhase::Running, false),
            DesktopShellCommand::Proxy(ProxyMenuAction::Stop)
        );
        assert_eq!(
            menu_command(MENU_PROXY_ID, ProxyMenuPhase::Starting, false),
            DesktopShellCommand::Ignore
        );
        assert_eq!(
            menu_command(MENU_QUIT_ID, ProxyMenuPhase::Running, false),
            DesktopShellCommand::Quit
        );
        assert_eq!(
            menu_command("unknown", ProxyMenuPhase::Running, false),
            DesktopShellCommand::Ignore
        );
    }

    #[test]
    fn agent_menu_snapshot_sorts_deduplicates_and_rejects_unsafe_ids() {
        assert_eq!(
            agent_menu_snapshot([
                AgentMenuEntry::new("codex", "Codex", 30),
                AgentMenuEntry::new("claude-code", "Claude Code", 10),
                AgentMenuEntry::new("codex", "Duplicate Codex", 20),
                AgentMenuEntry::new("../settings", "Unsafe", 0),
            ]),
            AgentMenuSnapshot::new([
                AgentMenuEntry::new("claude-code", "Claude Code", 10),
                AgentMenuEntry::new("codex", "Duplicate Codex", 20),
            ])
        );
    }

    #[test]
    fn agent_and_workspace_menu_commands_are_navigation_only() {
        let agents = AgentMenuSnapshot::new([
            AgentMenuEntry::new("claude-code", "Claude Code", 10),
            AgentMenuEntry::new("codex", "Codex", 20),
        ]);
        assert_eq!(
            menu_command_with_agents(
                "desktop-shell-agent:codex",
                ProxyMenuPhase::Running,
                false,
                &agents,
            ),
            DesktopShellCommand::Navigate("agent:codex".to_owned())
        );
        assert_eq!(
            menu_command_with_agents(
                "desktop-shell-agent:unknown",
                ProxyMenuPhase::Running,
                false,
                &agents,
            ),
            DesktopShellCommand::Ignore
        );
        for (menu_id, destination) in [
            (MENU_MANAGE_AGENTS_ID, "home"),
            (MENU_ADD_PROVIDER_ID, "add-provider"),
            (MENU_REQUEST_LOGS_ID, "logs"),
            (MENU_SETTINGS_ID, "settings"),
        ] {
            assert_eq!(
                menu_command_with_agents(menu_id, ProxyMenuPhase::Running, false, &agents,),
                DesktopShellCommand::Navigate(destination.to_owned())
            );
        }
    }
}

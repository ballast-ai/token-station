//! Native desktop shell policy for background residency and the status menu.

// The native status menu is installed only on macOS. Linux CI still compiles
// this module so its platform-neutral state policy remains unit tested and the
// cross-platform refresh call can stay a harmless no-op without cfg sprawl.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, Runtime, Window, WindowEvent};

pub(crate) const MAIN_WINDOW_LABEL: &str = "main";
pub(crate) const MENU_OPEN_ID: &str = "desktop-shell-open";
pub(crate) const MENU_PROXY_ID: &str = "desktop-shell-proxy";
pub(crate) const MENU_QUIT_ID: &str = "desktop-shell-quit";

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DesktopShellCommand {
    Ignore,
    Open,
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

pub(crate) fn menu_command(
    menu_id: &str,
    phase: ProxyMenuPhase,
    read_only: bool,
) -> DesktopShellCommand {
    match menu_id {
        MENU_OPEN_ID => DesktopShellCommand::Open,
        MENU_PROXY_ID => match proxy_menu_action(phase, read_only) {
            ProxyMenuAction::None => DesktopShellCommand::Ignore,
            action => DesktopShellCommand::Proxy(action),
        },
        MENU_QUIT_ID => DesktopShellCommand::Quit,
        _ => DesktopShellCommand::Ignore,
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

struct DesktopShellState<R: Runtime> {
    tray: TrayIcon<R>,
    address_item: MenuItem<R>,
    proxy_item: CheckMenuItem<R>,
    mode: ProxyMenuMode,
    read_proxy_state: Box<ProxyStateReader<R>>,
}

/// Installs the single native status-menu surface. The callback receives only
/// validated proxy actions; state changes still run through the app's existing
/// lifecycle coordinator in the caller.
pub(crate) fn install<R, S, F>(
    app: &AppHandle<R>,
    mode: ProxyMenuMode,
    read_proxy_state: S,
    on_proxy_action: F,
) -> tauri::Result<()>
where
    R: Runtime,
    S: Fn(&AppHandle<R>) -> ProxyMenuSnapshot + Send + Sync + 'static,
    F: Fn(&AppHandle<R>, ProxyMenuAction, u64) + Send + Sync + 'static,
{
    let initial = read_proxy_state(app);
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
    let separator_before_proxy = PredefinedMenuItem::separator(app)?;
    let separator_before_quit = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, MENU_QUIT_ID, "退出 Token Station", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open_item,
            &separator_before_proxy,
            &proxy_item,
            &address_item,
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
            let (snapshot, read_only) = {
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
                (snapshot, shell.mode.is_read_only())
            };
            match menu_command(event.id().as_ref(), snapshot.phase, read_only) {
                DesktopShellCommand::Ignore => {}
                DesktopShellCommand::Open => restore_main_window(app),
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
        mode,
        read_proxy_state: Box::new(read_proxy_state),
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
}

// ---- Tauri commands --------------------------------------------------------

pub(crate) fn dock_icon_bytes(theme: &str) -> Result<&'static [u8], String> {
    match theme {
        "light" => Ok(include_bytes!("../icons/icon-light.png")),
        "dark" => Ok(include_bytes!("../icons/icon-dark.png")),
        _ => Err(format!("unsupported Dock icon theme: {theme}")),
    }
}

#[tauri::command]
pub(crate) async fn set_dock_theme_icon(
    app: tauri::AppHandle,
    theme: String,
) -> Result<(), String> {
    let icon_bytes = dock_icon_bytes(&theme)?;

    #[cfg(target_os = "macos")]
    {
        // Imported here rather than at file scope: everything else in this
        // module is fully qualified, so with no file-level import there is
        // nothing that can go unused under `cfg(not(target_os = "macos"))`.
        // A `use crate::*` used to supply this, and on Linux it supplied
        // nothing at all — which `-D warnings` correctly refused.
        use std::time::Duration;

        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        app.run_on_main_thread(move || {
            let _ = result_tx.send(apply_macos_dock_icon(icon_bytes));
        })
        .map_err(|error| format!("failed to schedule Dock icon update: {error}"))?;

        let apply_result = tauri::async_runtime::spawn_blocking(move || {
            result_rx.recv_timeout(Duration::from_secs(2))
        })
        .await
        .map_err(|error| format!("failed to join Dock icon update: {error}"))?
        .map_err(|error| format!("timed out waiting for Dock icon update: {error}"))?;
        apply_result?;
    }

    #[cfg(not(target_os = "macos"))]
    let _ = (app, icon_bytes);

    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn apply_macos_dock_icon(icon_bytes: &'static [u8]) -> Result<(), String> {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApp, NSImage};
    use objc2_foundation::NSData;

    let main_thread = MainThreadMarker::new()
        .ok_or_else(|| "Dock icon update did not run on the AppKit main thread".to_string())?;
    let data = NSData::with_bytes(icon_bytes);
    let image = NSImage::initWithData(NSImage::alloc(), &data)
        .ok_or_else(|| "failed to decode the embedded Dock icon".to_string())?;
    if !image.isValid() {
        return Err("decoded Dock icon is not a valid AppKit image".to_string());
    }
    let application = NSApp(main_thread);

    // AppKit requires application icon updates on the main thread.
    unsafe { application.setApplicationIconImage(Some(&image)) };
    let applied_image = application
        .applicationIconImage()
        .ok_or_else(|| "AppKit did not retain the Dock icon".to_string())?;
    if !applied_image.isValid() {
        return Err("AppKit did not apply the requested Dock icon".to_string());
    }

    Ok(())
}

#[cfg(test)]
mod dock_icon_tests {
    use super::dock_icon_bytes;

    #[test]
    fn accepts_supported_dock_icon_themes() {
        for theme in ["light", "dark"] {
            assert!(dock_icon_bytes(theme).is_ok());
        }
    }

    #[test]
    fn rejects_unknown_dock_icon_theme() {
        assert!(dock_icon_bytes("system").is_err());
    }

    #[test]
    fn embeds_png_dock_icons() {
        for theme in ["light", "dark"] {
            assert!(dock_icon_bytes(theme)
                .unwrap()
                .starts_with(b"\x89PNG\r\n\x1a\n"));
        }
    }

    #[test]
    fn embeds_distinct_light_and_dark_dock_icons() {
        assert_ne!(
            dock_icon_bytes("light").unwrap(),
            dock_icon_bytes("dark").unwrap()
        );
    }
}

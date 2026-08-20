use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use token_station_desktop_lib::desktop_update::{
    check_with, install_with, update_manifest_endpoint, DesktopUpdateOperation,
    DesktopUpdatePrepareFailure, DesktopUpdateStatus, DesktopUpdateView, STABLE_LATEST_JSON_URL,
    WINDOWS_FIRST_RELEASE_UNSUPPORTED_MESSAGE,
};

#[test]
fn updater_manifest_endpoint_accepts_only_an_https_override() {
    let preview = "https://github.com/ballast-ai/token-station/releases/download/updater-preview/latest.json";

    assert_eq!(update_manifest_endpoint(None), Ok(STABLE_LATEST_JSON_URL));
    assert_eq!(update_manifest_endpoint(Some("  ")), Ok(STABLE_LATEST_JSON_URL));
    assert_eq!(update_manifest_endpoint(Some(preview)), Ok(preview));
    assert!(update_manifest_endpoint(Some("http://example.test/latest.json"))
        .unwrap_err()
        .contains("HTTPS"));
}

#[test]
fn concurrent_update_operations_are_rejected_until_the_first_finishes() {
    let operation = DesktopUpdateOperation::default();
    let first = operation.try_begin().expect("first operation starts");
    assert_eq!(
        operation.try_begin().err().as_deref(),
        Some("update_in_progress: 已有更新任务正在进行")
    );
    drop(first);
    assert!(operation.try_begin().is_ok());
}

#[test]
fn unsupported_platform_view_keeps_release_page_available() {
    let view = DesktopUpdateView::unsupported("1.1.2", WINDOWS_FIRST_RELEASE_UNSUPPORTED_MESSAGE);

    assert_eq!(view.status, DesktopUpdateStatus::Unsupported);
    assert_eq!(view.current_version, "1.1.2");
    assert_eq!(view.version, None);
    assert!(view
        .message
        .as_deref()
        .unwrap_or_default()
        .contains("Windows"));
    assert!(view.release_url.ends_with("/releases"));
}

#[tokio::test]
async fn a_source_build_without_the_official_public_key_never_contacts_the_update_endpoint() {
    let contacted = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&contacted);

    let view = check_with("1.1.2", "", move || async move {
        observed.store(true, Ordering::SeqCst);
        Ok(None)
    })
    .await;

    assert_eq!(view.status, DesktopUpdateStatus::Unsupported);
    assert_eq!(view.current_version, "1.1.2");
    assert_eq!(view.version, None);
    assert!(view
        .message
        .as_deref()
        .unwrap_or_default()
        .contains("官方更新公钥"));
    assert!(!contacted.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_download_or_signature_failure_never_stops_the_gateway_or_installs() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));

    let check_events = Arc::clone(&events);
    let download_events = Arc::clone(&events);
    let prepare_events = Arc::clone(&events);
    let install_events = Arc::clone(&events);
    let result = install_with(
        "official-public-key",
        "1.1.3",
        move || async move {
            check_events.lock().unwrap().push("check");
            Ok(Some(("1.1.3".to_owned(), "signed-package")))
        },
        move |package| async move {
            download_events.lock().unwrap().push("download");
            assert_eq!(package, "signed-package");
            Err("更新包签名校验失败".to_owned())
        },
        move || async move {
            prepare_events.lock().unwrap().push("prepare");
            Ok(())
        },
        move |_package, _bytes, _prepared| {
            install_events.lock().unwrap().push("install");
            Ok(())
        },
        |_prepared| async { Ok(()) },
    )
    .await;

    assert_eq!(result.unwrap_err(), "更新包签名校验失败");
    assert_eq!(*events.lock().unwrap(), vec!["check", "download"]);
}

#[tokio::test]
async fn a_changed_latest_version_is_rejected_before_download_or_gateway_stop() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));

    let check_events = Arc::clone(&events);
    let download_events = Arc::clone(&events);
    let prepare_events = Arc::clone(&events);
    let install_events = Arc::clone(&events);
    let result = install_with(
        "official-public-key",
        "1.1.3",
        move || async move {
            check_events.lock().unwrap().push("check");
            Ok(Some(("1.1.4".to_owned(), "signed-package")))
        },
        move |package| async move {
            download_events.lock().unwrap().push("download");
            Ok((package, b"verified-bytes".to_vec()))
        },
        move || async move {
            prepare_events.lock().unwrap().push("prepare-gateway");
            Ok(())
        },
        move |_package, _bytes, _prepared| {
            install_events.lock().unwrap().push("install");
            Ok(())
        },
        |_prepared| async { Ok(()) },
    )
    .await;

    assert_eq!(
        result.unwrap_err(),
        "update_version_changed: 已确认更新到 1.1.3，但当前可用版本已变为 1.1.4；请重新检查并确认"
    );
    assert_eq!(*events.lock().unwrap(), vec!["check"]);
}

#[tokio::test]
async fn a_missing_expected_version_is_rejected_before_checking_again() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let check_events = Arc::clone(&events);

    let result = install_with(
        "official-public-key",
        "  ",
        move || async move {
            check_events.lock().unwrap().push("check");
            Ok(Some(("1.1.3".to_owned(), "signed-package")))
        },
        |package| async move { Ok((package, b"verified-bytes".to_vec())) },
        || async { Ok(()) },
        |_package, _bytes, _prepared| Ok(()),
        |_prepared| async { Ok(()) },
    )
    .await;

    assert_eq!(
        result.unwrap_err(),
        "update_expected_version_missing: 缺少已确认的更新版本，请重新检查"
    );
    assert!(events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_verified_update_stops_the_gateway_only_before_installing() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));

    let check_events = Arc::clone(&events);
    let download_events = Arc::clone(&events);
    let prepare_events = Arc::clone(&events);
    let install_events = Arc::clone(&events);
    let result = install_with(
        "official-public-key",
        "1.1.3",
        move || async move {
            check_events.lock().unwrap().push("check");
            Ok(Some(("1.1.3".to_owned(), "signed-package")))
        },
        move |package| async move {
            download_events.lock().unwrap().push("download-and-verify");
            Ok((package, b"verified-bytes".to_vec()))
        },
        move || async move {
            prepare_events.lock().unwrap().push("prepare-gateway");
            Ok("gateway-was-running")
        },
        move |package, bytes, prepared| {
            install_events.lock().unwrap().push("install");
            assert_eq!(package, "signed-package");
            assert_eq!(bytes, b"verified-bytes");
            assert_eq!(*prepared, "gateway-was-running");
            Ok(())
        },
        |_prepared| async { Ok(()) },
    )
    .await;

    assert_eq!(result, Ok(true));
    assert_eq!(
        *events.lock().unwrap(),
        vec!["check", "download-and-verify", "prepare-gateway", "install"]
    );
}

#[tokio::test]
async fn a_failed_install_recovers_a_previously_running_gateway() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let prepare_events = Arc::clone(&events);
    let install_events = Arc::clone(&events);
    let recover_events = Arc::clone(&events);

    let result = install_with(
        "official-public-key",
        "1.1.3",
        || async { Ok(Some(("1.1.3".to_owned(), "signed-package"))) },
        |package| async move { Ok((package, b"verified-bytes".to_vec())) },
        move || async move {
            prepare_events.lock().unwrap().push("prepare-gateway");
            Ok(true)
        },
        move |_package, _bytes, was_active| {
            install_events.lock().unwrap().push("install-failed");
            assert!(*was_active);
            Err("安装器拒绝替换".to_owned())
        },
        move |was_active| async move {
            assert!(was_active);
            recover_events.lock().unwrap().push("restore-gateway");
            Ok(())
        },
    )
    .await;

    assert_eq!(result.unwrap_err(), "安装器拒绝替换");
    assert_eq!(
        *events.lock().unwrap(),
        vec!["prepare-gateway", "install-failed", "restore-gateway"]
    );
}

#[tokio::test]
async fn a_failed_gateway_prepare_recovers_before_returning_the_error() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));

    let prepare_events = Arc::clone(&events);
    let install_events = Arc::clone(&events);
    let recover_events = Arc::clone(&events);
    let result = install_with(
        "official-public-key",
        "1.1.3",
        || async { Ok(Some(("1.1.3".to_owned(), "signed-package"))) },
        |package| async move { Ok((package, b"verified-bytes".to_vec())) },
        move || async move {
            prepare_events.lock().unwrap().push("prepare-failed");
            Err(DesktopUpdatePrepareFailure::new(
                "update_gateway_stop_timeout: 等待本地网关安全停止超时",
                true,
            ))
        },
        move |_package, _bytes, _was_active| {
            install_events.lock().unwrap().push("install");
            Ok(())
        },
        move |was_active| async move {
            assert!(was_active);
            recover_events.lock().unwrap().push("restore-gateway");
            Ok(())
        },
    )
    .await;

    assert_eq!(
        result.unwrap_err(),
        "update_gateway_stop_timeout: 等待本地网关安全停止超时"
    );
    assert_eq!(
        *events.lock().unwrap(),
        vec!["prepare-failed", "restore-gateway"]
    );
}

#[tokio::test]
async fn a_failed_gateway_prepare_preserves_both_prepare_and_recovery_errors() {
    let result = install_with(
        "official-public-key",
        "1.1.3",
        || async { Ok(Some(("1.1.3".to_owned(), "signed-package"))) },
        |package| async move { Ok((package, b"verified-bytes".to_vec())) },
        || async {
            Err(DesktopUpdatePrepareFailure::new(
                "update_gateway_stop_timeout: 等待本地网关安全停止超时",
                true,
            ))
        },
        |_package, _bytes, _was_active| Ok(()),
        |was_active| async move {
            assert!(was_active);
            Err("startup_cleanup_in_progress: 网关仍在停止".to_owned())
        },
    )
    .await;

    assert_eq!(
        result.unwrap_err(),
        "update_gateway_stop_timeout: 等待本地网关安全停止超时; update_gateway_restore_failed: startup_cleanup_in_progress: 网关仍在停止"
    );
}
